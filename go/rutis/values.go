package rutis

import (
	"time"

	"github.com/arcships/rutis/go/rutis/internal/values"
)

// Values beyond JSON data cross a session as tagged values (#308), each
// to a far end that takes it (one that does not fails only that call):
//
//	time.Time          a point in time (instant), sent in UTC, to the nanosecond
//	DateTime           a local date and time without a time zone (datetime)
//	Date               a calendar date (date)
//	TimeOfDay          a time of day without a time zone (time)
//	[]byte, [N]byte    binary data (bytes)
//	map[K]V            a map, when K is not a string type (map)
//	map[K]struct{}     a set (set); a set also decodes into a slice
//	*big.Int, big.Int  an integer of any size (bigint); so is any integer
//	                   beyond ±(2^53-1) (int64, uint64, ...): a bigint
//	                   received decodes into the declared integer type
//	                   when it fits, and into `any` as *big.Int
//	Decimal            a decimal number, its text kept (decimal)
//	UUID               a UUID (uuid)
//
// They cross wherever they are: in arguments and results, struct fields,
// slices and maps. Map keys and set elements are strings, numbers,
// booleans, bigints, DateTime, Date, TimeOfDay, Decimal and UUID values,
// no two equal.

// DateTime is a local date and time without a time zone (tag
// `datetime`): it crosses as it is and is never read as a point in time
// in some zone (Python's datetime without tzinfo, Kotlin's LocalDateTime).
// DateTimeOf and DateTime.In convert from and to time.Time. Its text is
// `2026-10-15T14:30:05.25`.
//
// Deprecated: moved in 0.19: use advanced.DateTime.
type DateTime = values.DateTime

// Date is a calendar date (tag `date`), years 0001–9999: `2026-10-15`.
//
// Deprecated: moved in 0.19: use advanced.Date.
type Date = values.Date

// TimeOfDay is a time of day without a time zone (tag `time`), 24-hour,
// to the nanosecond: `14:30:05.25`.
//
// Deprecated: moved in 0.19: use advanced.TimeOfDay.
type TimeOfDay = values.TimeOfDay

// Decimal is a decimal number kept as its text (tag `decimal`): an
// optional `-`, digits, an optional fraction; no exponent; its trailing
// zeros kept (`1.10` is not `1.1`). A Decimal that is not such text fails
// the call that sends it; ParseDecimal checks it first.
//
// Deprecated: moved in 0.19: use advanced.Decimal.
type Decimal = values.Decimal

// UUID is a UUID (tag `uuid`) in lowercase `8-4-4-4-12` hex. A UUID that
// is not such text fails the call that sends it; ParseUUID checks it
// first and lowercases it.
//
// Deprecated: moved in 0.19: use advanced.UUID.
type UUID = values.UUID

// DateOf is the date of t in t's location.
//
// Deprecated: moved in 0.19: use advanced.DateOf.
func DateOf(t time.Time) Date { return values.DateOf(t) }

// TimeOfDayOf is the time of day of t in t's location.
//
// Deprecated: moved in 0.19: use advanced.TimeOfDayOf.
func TimeOfDayOf(t time.Time) TimeOfDay { return values.TimeOfDayOf(t) }

// DateTimeOf is the date and time of day of t in t's location.
//
// Deprecated: moved in 0.19: use advanced.DateTimeOf.
func DateTimeOf(t time.Time) DateTime { return values.DateTimeOf(t) }

// ParseDate reads `YYYY-MM-DD`.
//
// Deprecated: moved in 0.19: use advanced.ParseDate.
func ParseDate(text string) (Date, error) { return values.ParseDate(text) }

// ParseTimeOfDay reads `HH:MM:SS` with an optional fraction of 1–9
// digits.
//
// Deprecated: moved in 0.19: use advanced.ParseTimeOfDay.
func ParseTimeOfDay(text string) (TimeOfDay, error) { return values.ParseTimeOfDay(text) }

// ParseDateTime reads `YYYY-MM-DDTHH:MM:SS` with an optional fraction of
// 1–9 digits and no time zone.
//
// Deprecated: moved in 0.19: use advanced.ParseDateTime.
func ParseDateTime(text string) (DateTime, error) { return values.ParseDateTime(text) }

// ParseDecimal checks decimal text: `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, at
// most 4300 digits.
//
// Deprecated: moved in 0.19: use advanced.ParseDecimal.
func ParseDecimal(text string) (Decimal, error) { return values.ParseDecimal(text) }

// ParseUUID reads `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` in either case
// and returns it in lowercase.
//
// Deprecated: moved in 0.19: use advanced.ParseUUID.
func ParseUUID(text string) (UUID, error) { return values.ParseUUID(text) }
