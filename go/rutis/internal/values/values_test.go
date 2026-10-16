package values

import (
	"bytes"
	"math/big"
	"strings"
	"testing"
	"time"
)

func TestInstantsReadAndWriteRFC3339InUTC(t *testing.T) {
	for _, c := range []struct {
		text    string
		seconds int64
		nanos   int
	}{
		{"1970-01-01T00:00:00Z", 0, 0},
		{"2026-10-15T03:22:15.123456789Z", 1_792_034_535, 123_456_789},
		{"2026-10-15T03:22:15.1Z", 1_792_034_535, 100_000_000},
		{"1969-12-31T23:59:59.999999999Z", -1, 999_999_999},
		{"0001-01-01T00:00:00Z", -62_135_596_800, 0},
		{"9999-12-31T23:59:59.999999999Z", 253_402_300_799, 999_999_999},
		{"2000-02-29T12:00:00Z", 951_825_600, 0},
	} {
		parsed, err := ParseInstant(c.text)
		if err != nil {
			t.Fatalf("%s: %v", c.text, err)
		}
		if parsed.Unix() != c.seconds || parsed.Nanosecond() != c.nanos || parsed.Location() != time.UTC {
			t.Errorf("%s: %v", c.text, parsed)
		}
		text, err := FormatInstant(time.Unix(c.seconds, int64(c.nanos)))
		if err != nil || text != c.text {
			t.Errorf("%s: formatted %s, %v", c.text, text, err)
		}
	}
	// Any zone is written in UTC; trailing zeros of a fraction are not kept.
	oslo := time.FixedZone("Oslo", 2*3600)
	if text, _ := FormatInstant(time.Date(2026, 10, 15, 5, 22, 15, 500_000_000, oslo)); text != "2026-10-15T03:22:15.5Z" {
		t.Errorf("%s", text)
	}
	for _, out := range []time.Time{time.Date(0, 12, 31, 23, 0, 0, 0, time.UTC), time.Date(10000, 1, 1, 0, 0, 0, 0, time.UTC)} {
		if _, err := FormatInstant(out); err == nil || !strings.Contains(err.Error(), "outside years 0001–9999") {
			t.Errorf("%v: %v", out, err)
		}
	}
	for _, text := range []string{
		"2026-10-15T03:22:15",             // no zone
		"2026-10-15T03:22:15+00:00",       // an offset, not Z
		"2026-10-15T03:22:15z",            // lowercase z
		"2026-10-15t03:22:15Z",            // lowercase t
		"2026-10-15 03:22:15Z",            // a space
		"2026-10-15T03:22:15.Z",           // an empty fraction
		"2026-10-15T03:22:15.1234567891Z", // ten digits
		"2026-10-15T03:22:60Z",            // a leap second
		"2026-10-15T24:00:00Z",            // hour 24
		"2026-02-29T00:00:00Z",            // not a leap year
		"0000-12-31T23:59:59Z",            // year 0
		"10000-01-01T00:00:00Z",           // year 10000
		"+2026-10-15T03:22:15Z",           // a sign
		"2026-1-15T03:22:15Z",             // one-digit month
		"",
	} {
		if _, err := ParseInstant(text); err == nil {
			t.Errorf("%q was taken", text)
		}
	}
}

func TestLocalDatesAndTimesKeepTheirTextWithoutAZone(t *testing.T) {
	for _, text := range []string{"2026-10-15", "0001-01-01", "9999-12-31", "2000-02-29"} {
		if d, err := ParseDate(text); err != nil || d.String() != text {
			t.Errorf("%s: %v %v", text, d, err)
		}
	}
	for _, text := range []string{"2026-13-01", "2026-00-01", "2026-04-31", "1900-02-29", "0000-01-01", "2026-10-15T00:00:00", "20261015"} {
		if _, err := ParseDate(text); err == nil {
			t.Errorf("%q was taken", text)
		}
	}
	for _, text := range []string{"14:30:05", "14:30:05.25", "00:00:00.000000001", "23:59:59.999999999"} {
		if v, err := ParseTimeOfDay(text); err != nil || v.String() != text {
			t.Errorf("%s: %v %v", text, v, err)
		}
	}
	if v, _ := ParseTimeOfDay("14:30:05.250"); v.String() != "14:30:05.25" {
		t.Errorf("%v", v)
	}
	for _, text := range []string{"14:30", "24:00:00", "14:60:00", "14:30:05Z", "14:30:05+01:00", "14:30:05.", "1:30:05"} {
		if _, err := ParseTimeOfDay(text); err == nil {
			t.Errorf("%q was taken", text)
		}
	}
	local, err := ParseDateTime("2026-10-15T14:30:05.25")
	if err != nil || local.String() != "2026-10-15T14:30:05.25" || local.Date != (Date{2026, 10, 15}) || local.Time.Nanosecond != 250_000_000 {
		t.Errorf("%v %v", local, err)
	}
	for _, text := range []string{"2026-10-15T14:30:05Z", "2026-10-15T14:30:05+02:00", "2026-10-15", "2026-10-15T14:30"} {
		if _, err := ParseDateTime(text); err == nil {
			t.Errorf("%q was taken", text)
		}
	}
	// Not a point in time: In reads it in a zone, DateTimeOf back.
	oslo := time.FixedZone("Oslo", 2*3600)
	if back := DateTimeOf(local.In(oslo)); back != local {
		t.Errorf("%v", back)
	}
	if err := Check(Date{2026, 2, 30}); err == nil || !strings.Contains(err.Error(), "invalid date 2026-02-30") {
		t.Errorf("%v", err)
	}
	if err := Check(TimeOfDay{Hour: 24}); err == nil {
		t.Error("hour 24 is not a time")
	}
	var d Date
	if err := d.UnmarshalText([]byte("2026-10-15")); err != nil || d != (Date{2026, 10, 15}) {
		t.Errorf("%v %v", d, err)
	}
}

func TestBigIntegersKeepEveryDigitUpToTheLimit(t *testing.T) {
	for _, text := range []string{"0", "-1", "123456789012345678901234567890", "-170141183460469231731687303715884105729"} {
		n, err := ParseBigInt(text)
		if err != nil || n.String() != text {
			t.Errorf("%s: %v %v", text, n, err)
		}
	}
	most := strings.Repeat("9", MaxDigits)
	if _, err := ParseBigInt(most); err != nil {
		t.Error(err)
	}
	if _, err := ParseBigInt("-" + most); err != nil {
		t.Error(err)
	}
	if _, err := ParseBigInt(most + "9"); err == nil {
		t.Error("4301 digits were taken")
	}
	beyond, _ := new(big.Int).SetString(most+"9", 10)
	if _, err := FormatBigInt(beyond); err == nil {
		t.Error("4301 digits were sent")
	}
	for _, text := range []string{"", "-", "-0", "01", "+1", "1.0", "1e3", " 1", "0x10", "١", "1_000"} {
		if _, err := ParseBigInt(text); err == nil {
			t.Errorf("%q was taken", text)
		}
	}
}

func TestDecimalsKeepTheirTextAndTrailingZeros(t *testing.T) {
	for _, text := range []string{"0", "-0", "-0.0", "1.10", "1.1", "-12.3400", "0.000001", "123456789012345678901234567890.5"} {
		if d, err := ParseDecimal(text); err != nil || string(d) != text {
			t.Errorf("%s: %v", text, err)
		}
	}
	most := strings.Repeat("1", MaxDigits-10) + "." + strings.Repeat("0", 10)
	if _, err := ParseDecimal(most); err != nil {
		t.Error(err)
	}
	if _, err := ParseDecimal(most + "0"); err == nil {
		t.Error("4301 digits were taken")
	}
	for _, text := range []string{"", ".5", "5.", "1e3", "1E-3", "NaN", "Infinity", "-inf", "00.1", "+1", "1.2.3", "1,5"} {
		if _, err := ParseDecimal(text); err == nil {
			t.Errorf("%q was taken", text)
		}
	}
}

func TestUUIDsAreLowercase8_4_4_4_12(t *testing.T) {
	text := "0f8fad5b-d9cb-469f-a165-70867728950e"
	if err := CheckUUID(text); err != nil {
		t.Fatal(err)
	}
	if u, err := ParseUUID(strings.ToUpper(text)); err != nil || string(u) != text {
		t.Errorf("ParseUUID lowercases: %v %v", u, err)
	}
	for _, bad := range []string{
		"0F8FAD5B-D9CB-469F-A165-70867728950E",
		"0f8fad5bd9cb469fa16570867728950e",
		"{0f8fad5b-d9cb-469f-a165-70867728950e}",
		"0f8fad5b-d9cb-469f-a165-70867728950",
		"0f8fad5b-d9cb-469f-a165-70867728950g",
		"0f8fad5b+d9cb-469f-a165-70867728950e",
	} {
		if CheckUUID(bad) == nil {
			t.Errorf("%q was taken", bad)
		}
	}
}

func TestBytesUsePaddedCanonicalBase64(t *testing.T) {
	for _, c := range []struct {
		bytes []byte
		text  string
	}{
		{[]byte{}, ""}, {[]byte("f"), "Zg=="}, {[]byte("fo"), "Zm8="}, {[]byte("foo"), "Zm9v"},
		{[]byte("hello"), "aGVsbG8="}, {[]byte{0xff, 0xfe, 0x00}, "//4A"},
	} {
		if text := FormatBytes(c.bytes); text != c.text {
			t.Errorf("%v: %s", c.bytes, text)
		}
		if decoded, err := ParseBytes(c.text); err != nil || !bytes.Equal(decoded, c.bytes) {
			t.Errorf("%s: %v %v", c.text, decoded, err)
		}
	}
	for _, text := range []string{"Zg", "Zg=", "Zh==", "Zg==Zg==", "Z===", "Zm9v\n", "Zm9\nv", "Zm-v", "Zm_v", "===="} {
		if _, err := ParseBytes(text); err == nil {
			t.Errorf("%q was taken", text)
		}
	}
}
