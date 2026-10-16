// Package values holds the tagged values beyond JSON data (#308, #355,
// #360) and their text on the wire: points in time, local dates and
// times, binary data, big integers, decimals and UUIDs. The rutis package
// exports the types (rutis.DateTime, rutis.Date, rutis.TimeOfDay,
// rutis.Decimal, rutis.UUID).
//
//	| tag      | Go                | text on the wire                       |
//	| instant  | time.Time (UTC)   | 2026-10-15T03:22:15.123456789Z         |
//	| datetime | DateTime          | 2026-10-15T14:30:05.25, no time zone   |
//	| date     | Date              | 2026-10-15                             |
//	| time     | TimeOfDay         | 14:30:05.25, 24-hour, no time zone     |
//	| bytes    | []byte            | base64 (RFC 4648 §4, padded)           |
//	| bigint   | *big.Int          | decimal integer, at most 4300 digits   |
//	| decimal  | Decimal           | decimal, no exponent, zeros kept       |
//	| uuid     | UUID              | lowercase 8-4-4-4-12 hex               |
//
// Years are 0001–9999; fractions of a second have 0–9 digits. Design:
// docs/design-value-types-2026-10-15.md; the normative wire text is in
// docs/design-protocol-plugins-2026-09-25.md §6.1.
package values

import (
	"encoding/base64"
	"fmt"
	"math/big"
	"strings"
	"time"
	"unicode/utf8"
)

// Tags are the tags of the values beyond JSON data; each is declared as
// the capability `value-<tag>`.
var Tags = []string{"instant", "datetime", "date", "time", "bytes", "map", "set", "bigint", "decimal", "uuid"}

// MaxDigits is the most digits a bigint or decimal may have: Python's
// default limit on integer text.
const MaxDigits = 4300

// quoted is a text an error quotes, at most 40 characters.
func quoted(text string) string {
	if utf8.RuneCountInString(text) <= 40 {
		return fmt.Sprintf("%q", text)
	}
	runes := []rune(text)
	return fmt.Sprintf("%q…", string(runes[:40]))
}

func invalid(what, text, expected string) error {
	return fmt.Errorf("invalid %s %s: expected %s", what, quoted(text), expected)
}

// ── Reading the text formats ─────────────────────────────────────────

// cursor reads the ASCII text of a value.
type cursor struct {
	text string
	at   int
}

// digits reads exactly `count` decimal digits.
func (c *cursor) digits(count int) (int, bool) {
	if c.at+count > len(c.text) {
		return 0, false
	}
	value := 0
	for _, digit := range []byte(c.text[c.at : c.at+count]) {
		if digit < '0' || digit > '9' {
			return 0, false
		}
		value = value*10 + int(digit-'0')
	}
	c.at += count
	return value, true
}

func (c *cursor) byte(b byte) bool {
	if c.at < len(c.text) && c.text[c.at] == b {
		c.at++
		return true
	}
	return false
}

// fraction reads an optional `.` and 1–9 digits, in nanoseconds.
func (c *cursor) fraction() (int, bool) {
	if !c.byte('.') {
		return 0, true
	}
	start := c.at
	for c.at < len(c.text) && c.text[c.at] >= '0' && c.text[c.at] <= '9' {
		c.at++
	}
	digits := c.text[start:c.at]
	if len(digits) == 0 || len(digits) > 9 {
		return 0, false
	}
	nanos := 0
	for i := 0; i < 9; i++ {
		nanos *= 10
		if i < len(digits) {
			nanos += int(digits[i] - '0')
		}
	}
	return nanos, true
}

func (c *cursor) end() bool { return c.at == len(c.text) }

func (c *cursor) date() (Date, bool) {
	year, ok1 := c.digits(4)
	ok2 := c.byte('-')
	month, ok3 := c.digits(2)
	ok4 := c.byte('-')
	day, ok5 := c.digits(2)
	date := Date{Year: year, Month: time.Month(month), Day: day}
	return date, ok1 && ok2 && ok3 && ok4 && ok5 && date.IsValid()
}

func (c *cursor) time() (TimeOfDay, bool) {
	hour, ok1 := c.digits(2)
	ok2 := c.byte(':')
	minute, ok3 := c.digits(2)
	ok4 := c.byte(':')
	second, ok5 := c.digits(2)
	if !(ok1 && ok2 && ok3 && ok4 && ok5) {
		return TimeOfDay{}, false
	}
	nanosecond, ok := c.fraction()
	t := TimeOfDay{Hour: hour, Minute: minute, Second: second, Nanosecond: nanosecond}
	return t, ok && t.IsValid()
}

func (c *cursor) datetime() (DateTime, bool) {
	date, ok := c.date()
	if !ok || !c.byte('T') {
		return DateTime{}, false
	}
	t, ok := c.time()
	return DateTime{Date: date, Time: t}, ok
}

// fraction is `.` and the digits of `nanos` without trailing zeros; ""
// for 0.
func fraction(nanos int) string {
	if nanos == 0 {
		return ""
	}
	return "." + strings.TrimRight(fmt.Sprintf("%09d", nanos), "0")
}

// ── Date, time of day, local date and time ───────────────────────────

func daysIn(year int, month time.Month) int {
	switch month {
	case time.February:
		if year%4 == 0 && (year%100 != 0 || year%400 == 0) {
			return 29
		}
		return 28
	case time.April, time.June, time.September, time.November:
		return 30
	}
	return 31
}

// Date is a calendar date without a time zone (tag `date`), years
// 0001–9999.
type Date struct {
	Year  int
	Month time.Month // 1–12
	Day   int        // 1–31
}

// DateOf is the date of t in t's location.
func DateOf(t time.Time) Date {
	year, month, day := t.Date()
	return Date{Year: year, Month: month, Day: day}
}

// ParseDate reads `YYYY-MM-DD`.
func ParseDate(text string) (Date, error) {
	c := cursor{text: text}
	date, ok := c.date()
	if !ok || !c.end() {
		return Date{}, invalid("date", text, "YYYY-MM-DD, years 0001–9999")
	}
	return date, nil
}

// IsValid reports whether the date exists, in years 0001–9999.
func (d Date) IsValid() bool {
	return d.Year >= 1 && d.Year <= 9999 && d.Month >= 1 && d.Month <= 12 && d.Day >= 1 && d.Day <= daysIn(d.Year, d.Month)
}

// String is `YYYY-MM-DD`.
func (d Date) String() string { return fmt.Sprintf("%04d-%02d-%02d", d.Year, int(d.Month), d.Day) }

// In is midnight at the start of the date in `loc`.
func (d Date) In(loc *time.Location) time.Time {
	return time.Date(d.Year, d.Month, d.Day, 0, 0, 0, 0, loc)
}

func (d Date) MarshalText() ([]byte, error) {
	if !d.IsValid() {
		return nil, d.check()
	}
	return []byte(d.String()), nil
}

func (d *Date) UnmarshalText(text []byte) error {
	parsed, err := ParseDate(string(text))
	if err == nil {
		*d = parsed
	}
	return err
}

func (d Date) check() error {
	if d.IsValid() {
		return nil
	}
	return fmt.Errorf("invalid date %s: no such day in years 0001–9999", d)
}

// TimeOfDay is a time of day without a time zone (tag `time`), to the
// nanosecond: 24-hour, no leap second.
type TimeOfDay struct {
	Hour       int // 0–23
	Minute     int // 0–59
	Second     int // 0–59
	Nanosecond int // 0–999999999
}

// TimeOfDayOf is the time of day of t in t's location.
func TimeOfDayOf(t time.Time) TimeOfDay {
	return TimeOfDay{Hour: t.Hour(), Minute: t.Minute(), Second: t.Second(), Nanosecond: t.Nanosecond()}
}

// ParseTimeOfDay reads `HH:MM:SS`, with an optional fraction of 1–9
// digits.
func ParseTimeOfDay(text string) (TimeOfDay, error) {
	c := cursor{text: text}
	t, ok := c.time()
	if !ok || !c.end() {
		return TimeOfDay{}, invalid("time", text, "HH:MM:SS[.fraction], 24-hour, 0–9 fraction digits")
	}
	return t, nil
}

// IsValid reports whether every field is in its range.
func (t TimeOfDay) IsValid() bool {
	return t.Hour >= 0 && t.Hour <= 23 && t.Minute >= 0 && t.Minute <= 59 && t.Second >= 0 && t.Second <= 59 &&
		t.Nanosecond >= 0 && t.Nanosecond < 1e9
}

// String is `HH:MM:SS` and the fraction without trailing zeros, if any.
func (t TimeOfDay) String() string {
	return fmt.Sprintf("%02d:%02d:%02d%s", t.Hour, t.Minute, t.Second, fraction(t.Nanosecond))
}

func (t TimeOfDay) MarshalText() ([]byte, error) {
	if err := t.check(); err != nil {
		return nil, err
	}
	return []byte(t.String()), nil
}

func (t *TimeOfDay) UnmarshalText(text []byte) error {
	parsed, err := ParseTimeOfDay(string(text))
	if err == nil {
		*t = parsed
	}
	return err
}

func (t TimeOfDay) check() error {
	if t.IsValid() {
		return nil
	}
	return fmt.Errorf("invalid time %02d:%02d:%02d (+%d ns)", t.Hour, t.Minute, t.Second, t.Nanosecond)
}

// DateTime is a local date and time without a time zone (tag
// `datetime`): passed as it is, never read as a point in time in some
// zone (design V5).
type DateTime struct {
	Date Date
	Time TimeOfDay
}

// DateTimeOf is the date and time of day of t in t's location.
func DateTimeOf(t time.Time) DateTime { return DateTime{Date: DateOf(t), Time: TimeOfDayOf(t)} }

// ParseDateTime reads `YYYY-MM-DDTHH:MM:SS`, with an optional fraction and
// no time zone.
func ParseDateTime(text string) (DateTime, error) {
	c := cursor{text: text}
	dt, ok := c.datetime()
	if !ok || !c.end() {
		return DateTime{}, invalid("datetime", text, "YYYY-MM-DDTHH:MM:SS[.fraction] without a time zone, years 0001–9999")
	}
	return dt, nil
}

// IsValid reports whether both the date and the time are valid.
func (dt DateTime) IsValid() bool { return dt.Date.IsValid() && dt.Time.IsValid() }

// String is `YYYY-MM-DDTHH:MM:SS[.fraction]`.
func (dt DateTime) String() string { return dt.Date.String() + "T" + dt.Time.String() }

// In is the time at which it is dt in `loc`.
func (dt DateTime) In(loc *time.Location) time.Time {
	return time.Date(dt.Date.Year, dt.Date.Month, dt.Date.Day, dt.Time.Hour, dt.Time.Minute, dt.Time.Second, dt.Time.Nanosecond, loc)
}

func (dt DateTime) MarshalText() ([]byte, error) {
	if err := dt.check(); err != nil {
		return nil, err
	}
	return []byte(dt.String()), nil
}

func (dt *DateTime) UnmarshalText(text []byte) error {
	parsed, err := ParseDateTime(string(text))
	if err == nil {
		*dt = parsed
	}
	return err
}

func (dt DateTime) check() error {
	if err := dt.Date.check(); err != nil {
		return err
	}
	return dt.Time.check()
}

// Check reports why a DateTime, Date or TimeOfDay cannot be sent, if it
// cannot.
func Check(v any) error {
	switch v := v.(type) {
	case DateTime:
		return v.check()
	case Date:
		return v.check()
	case TimeOfDay:
		return v.check()
	}
	return nil
}

// ── Instants ─────────────────────────────────────────────────────────

// FormatInstant is t in UTC as RFC 3339, the fraction to the nanosecond
// without trailing zeros, if t is within years 0001–9999.
func FormatInstant(t time.Time) (string, error) {
	t = t.UTC()
	if year := t.Year(); year < 1 || year > 9999 {
		return "", fmt.Errorf("the time %s is outside years 0001–9999", t.Format(time.RFC3339Nano))
	}
	return DateTimeOf(t).String() + "Z", nil
}

// ParseInstant reads RFC 3339 in UTC: `YYYY-MM-DDTHH:MM:SS`, an optional
// fraction of 1–9 digits, and `Z`. The time is in UTC.
func ParseInstant(text string) (time.Time, error) {
	c := cursor{text: text}
	dt, ok := c.datetime()
	if !ok || !c.byte('Z') || !c.end() {
		return time.Time{}, invalid("instant", text, "RFC 3339 in UTC, YYYY-MM-DDTHH:MM:SS[.fraction]Z, years 0001–9999")
	}
	return dt.In(time.UTC), nil
}

// ── Bytes ────────────────────────────────────────────────────────────

// FormatBytes is `b` in padded base64.
func FormatBytes(b []byte) string { return base64.StdEncoding.EncodeToString(b) }

// ParseBytes reads padded base64. Only the canonical text is taken: no
// whitespace, padding only at the end, unused bits zero.
func ParseBytes(text string) ([]byte, error) {
	// The standard library's decoder skips line breaks, even when strict.
	if strings.ContainsAny(text, "\r\n") || len(text)%4 != 0 {
		return nil, errBytes(text)
	}
	out, err := base64.StdEncoding.Strict().DecodeString(text)
	if err != nil {
		return nil, errBytes(text)
	}
	return out, nil
}

func errBytes(text string) error {
	return fmt.Errorf("invalid bytes (%d characters): expected base64 (RFC 4648 §4) with padding", len(text))
}

// ── Big integers and decimals ────────────────────────────────────────

// integerText reports whether `text` is `-`, then digits with no leading
// zero, and how many digits.
func integerText(text string) (negative bool, digits int, ok bool) {
	rest, negative := strings.CutPrefix(text, "-")
	if rest == "" || (len(rest) > 1 && rest[0] == '0') {
		return negative, 0, false
	}
	for i := 0; i < len(rest); i++ {
		if rest[i] < '0' || rest[i] > '9' {
			return negative, 0, false
		}
	}
	return negative, len(rest), true
}

// FormatBigInt is n in decimal, if it has at most MaxDigits digits.
func FormatBigInt(n *big.Int) (string, error) {
	text := n.String()
	if digits := len(strings.TrimPrefix(text, "-")); digits > MaxDigits {
		return "", fmt.Errorf("a bigint of %d digits is beyond %d, the most that cross", digits, MaxDigits)
	}
	return text, nil
}

// ParseBigInt reads decimal digits with an optional `-`: no leading
// zeros, no `-0`, at most MaxDigits digits.
func ParseBigInt(text string) (*big.Int, error) {
	_, digits, ok := integerText(text)
	if !ok || digits > MaxDigits || text == "-0" {
		return nil, invalid("bigint", text, "a decimal integer without leading zeros, at most 4300 digits")
	}
	n, _ := new(big.Int).SetString(text, 10)
	return n, nil
}

// Decimal is a decimal number (tag `decimal`) kept as its text: an
// optional `-`, an integer part without leading zeros, an optional
// fraction; no exponent; its trailing zeros kept ("1.10" is not "1.1");
// at most MaxDigits digits.
type Decimal string

// ParseDecimal checks `text` and returns it as a Decimal.
func ParseDecimal(text string) (Decimal, error) {
	integer, fraction, hasFraction := strings.Cut(text, ".")
	_, digits, ok := integerText(integer)
	if ok && hasFraction {
		ok = fraction != ""
		for i := 0; ok && i < len(fraction); i++ {
			ok = fraction[i] >= '0' && fraction[i] <= '9'
		}
		digits += len(fraction)
	}
	if !ok || digits > MaxDigits {
		return "", invalid("decimal", text, "decimal digits with an optional fraction, no exponent, at most 4300 digits")
	}
	return Decimal(text), nil
}

// IsValid reports whether d is decimal text as ParseDecimal takes it.
func (d Decimal) IsValid() bool {
	_, err := ParseDecimal(string(d))
	return err == nil
}

// String is the decimal text.
func (d Decimal) String() string { return string(d) }

// UUID is a UUID (tag `uuid`), any version, in lowercase `8-4-4-4-12`
// hex.
type UUID string

// ParseUUID reads `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` in either case
// and returns it in lowercase.
func ParseUUID(text string) (UUID, error) {
	lower := strings.ToLower(text)
	if !uuidText(lower) {
		return "", invalid("uuid", text, "hex 8-4-4-4-12")
	}
	return UUID(lower), nil
}

func uuidText(text string) bool {
	if len(text) != 36 {
		return false
	}
	for i := 0; i < len(text); i++ {
		c := text[i]
		switch i {
		case 8, 13, 18, 23:
			if c != '-' {
				return false
			}
		default:
			if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
				return false
			}
		}
	}
	return true
}

// IsValid reports whether u is lowercase `8-4-4-4-12` hex, as it crosses.
func (u UUID) IsValid() bool { return uuidText(string(u)) }

// String is the UUID's text.
func (u UUID) String() string { return string(u) }

// CheckUUID reports why `text` is not a UUID as it crosses (lowercase).
func CheckUUID(text string) error {
	if !uuidText(text) {
		return invalid("uuid", text, "lowercase hex 8-4-4-4-12")
	}
	return nil
}

// CheckDecimal reports why `text` is not a decimal as it crosses.
func CheckDecimal(text string) error {
	_, err := ParseDecimal(text)
	return err
}
