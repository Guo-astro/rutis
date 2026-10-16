//! The tagged values beyond JSON data (#308, #355, #360): points in time,
//! local dates and times, binary data, maps, sets, big integers, decimals
//! and UUIDs. Each crosses a session under its own tag, as text in the
//! formats below, and only to a far end that declared the capability
//! `value-<tag>`; their types here are small and dependency-free, with
//! conversions to what common crates use (Unix time, `SystemTime`, year /
//! month / day fields, decimal text, 16 bytes).
//!
//! | tag | type | text on the wire |
//! | --- | --- | --- |
//! | `instant` | [`Instant`] | RFC 3339 in UTC: `2026-10-15T03:22:15.123456789Z` |
//! | `datetime` | [`DateTime`] | `2026-10-15T14:30:05.25`, no time zone |
//! | `date` | [`Date`] | `2026-10-15` |
//! | `time` | [`TimeOfDay`] | `14:30:05.25`, 24-hour, no time zone |
//! | `bytes` | `Vec<u8>` | base64 (RFC 4648 §4, padded) |
//! | `map` | `Vec<(Value, Value)>` | `[[key, value], …]`, in the sender's order |
//! | `set` | `Vec<Value>` | `[element, …]`, in the sender's order |
//! | `bigint` | [`BigInt`] | decimal integer, no leading zeros, at most 4300 digits |
//! | `decimal` | [`Decimal`] | decimal, no exponent, trailing zeros kept, at most 4300 digits |
//! | `uuid` | [`Uuid`] | lowercase `8-4-4-4-12` hex |
//!
//! Years are 0001–9999; fractions of a second have 0–9 digits. Design:
//! `docs/design-value-types-2026-10-15.md`; the normative wire text is in
//! `docs/design-protocol-plugins-2026-09-25.md` §6.1.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::session::rpc::Value;
use crate::session::Error;

/// The tags of the values beyond JSON data; each is declared as the
/// capability `value-<tag>`.
pub const VALUE_TAGS: [&str; 10] = [
    "instant", "datetime", "date", "time", "bytes", "map", "set", "bigint", "decimal", "uuid",
];

/// The most digits a [`BigInt`] or [`Decimal`] may have: Python's default
/// limit on integer text.
pub const MAX_DIGITS: usize = 4300;

/// What a value error shows of a text: at most 40 characters.
fn shortened(text: &str) -> &str {
    match text.char_indices().nth(40) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// A text a value error quotes, shortened.
fn quoted(text: &str) -> String {
    let short = shortened(text);
    if short.len() < text.len() {
        format!("{short:?}…")
    } else {
        format!("{text:?}")
    }
}

fn invalid(what: &str, text: &str, expected: &str) -> Error {
    Error::Value(format!(
        "invalid {what} {}: expected {expected}",
        quoted(text)
    ))
}

// ── Calendar arithmetic ──────────────────────────────────────────────

const SECONDS_PER_DAY: i64 = 86_400;

/// Days from 1970-01-01 to `year-month-day` in the proleptic Gregorian
/// calendar (H. Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let month_from_march = (i64::from(month) + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date `days` after 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let days = days + 719_468;
    let era = (if days >= 0 { days } else { days - 146_096 }) / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u8;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    } as u8;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// ── Reading the text formats ─────────────────────────────────────────

/// A cursor over the ASCII text of a value.
struct Text<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Text<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            at: 0,
        }
    }
    /// Exactly `count` decimal digits.
    fn digits(&mut self, count: usize) -> Option<u32> {
        let digits = self.bytes.get(self.at..self.at + count)?;
        let mut value = 0u32;
        for digit in digits {
            if !digit.is_ascii_digit() {
                return None;
            }
            value = value * 10 + u32::from(digit - b'0');
        }
        self.at += count;
        Some(value)
    }
    fn byte(&mut self, byte: u8) -> Option<()> {
        (self.bytes.get(self.at) == Some(&byte)).then(|| self.at += 1)
    }
    /// An optional fraction of a second, `.` and 1–9 digits, in
    /// nanoseconds.
    fn fraction(&mut self) -> Option<u32> {
        if self.byte(b'.').is_none() {
            return Some(0);
        }
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        let digits = &self.bytes[start..self.at];
        if digits.is_empty() || digits.len() > 9 {
            return None;
        }
        let mut nanos = 0u32;
        for index in 0..9 {
            nanos = nanos * 10 + digits.get(index).map_or(0, |digit| u32::from(digit - b'0'));
        }
        Some(nanos)
    }
    fn end(&self) -> Option<()> {
        (self.at == self.bytes.len()).then_some(())
    }
    fn date(&mut self) -> Option<Date> {
        let year = self.digits(4)?;
        self.byte(b'-')?;
        let month = self.digits(2)?;
        self.byte(b'-')?;
        let day = self.digits(2)?;
        Date::new(year as u16, month as u8, day as u8).ok()
    }
    fn time(&mut self) -> Option<TimeOfDay> {
        let hour = self.digits(2)?;
        self.byte(b':')?;
        let minute = self.digits(2)?;
        self.byte(b':')?;
        let second = self.digits(2)?;
        let nanosecond = self.fraction()?;
        TimeOfDay::new(hour as u8, minute as u8, second as u8, nanosecond).ok()
    }
    fn datetime(&mut self) -> Option<DateTime> {
        let date = self.date()?;
        self.byte(b'T')?;
        Some(DateTime::new(date, self.time()?))
    }
}

/// `.` and the fraction's digits without trailing zeros; nothing for 0.
struct Fraction(u32);
impl fmt::Display for Fraction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 0 {
            return Ok(());
        }
        let digits = format!("{:09}", self.0);
        write!(f, ".{}", digits.trim_end_matches('0'))
    }
}

// ── Date, time of day, local date and time ───────────────────────────

/// A calendar date without a time zone (tag `date`), years 0001–9999.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}

impl Date {
    /// The date `year-month-day`, if it exists and the year is 1–9999.
    pub fn new(year: u16, month: u8, day: u8) -> Result<Self, Error> {
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day == 0
            || day > days_in_month(year, month)
        {
            return Err(Error::Value(format!(
                "invalid date {year:04}-{month:02}-{day:02}: no such day in years 0001–9999"
            )));
        }
        Ok(Self { year, month, day })
    }
    pub fn year(&self) -> u16 {
        self.year
    }
    /// 1–12.
    pub fn month(&self) -> u8 {
        self.month
    }
    /// 1–31.
    pub fn day(&self) -> u8 {
        self.day
    }
    /// Days since 1970-01-01, negative before it.
    pub fn days_since_unix_epoch(&self) -> i64 {
        days_from_civil(i64::from(self.year), self.month, self.day)
    }
    /// The date `days` after 1970-01-01, if its year is 1–9999.
    pub fn from_days_since_unix_epoch(days: i64) -> Result<Self, Error> {
        // 0001-01-01 and 9999-12-31: checked first, so the arithmetic
        // below never overflows.
        if !(-719_162..=2_932_896).contains(&days) {
            return Err(Error::Value(format!(
                "{days} days from 1970 leave years 0001–9999"
            )));
        }
        let (year, month, day) = civil_from_days(days);
        u16::try_from(year)
            .map_err(|_| Error::Value(format!("{days} days from 1970 leave years 0001–9999")))
            .and_then(|year| Self::new(year, month, day))
    }
}

impl FromStr for Date {
    type Err = Error;
    /// `YYYY-MM-DD`.
    fn from_str(text: &str) -> Result<Self, Error> {
        let mut cursor = Text::new(text);
        cursor
            .date()
            .filter(|_| cursor.end().is_some())
            .ok_or_else(|| invalid("date", text, "YYYY-MM-DD, years 0001–9999"))
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// A time of day without a time zone (tag `time`), to the nanosecond.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeOfDay {
    hour: u8,
    minute: u8,
    second: u8,
    nanosecond: u32,
}

impl TimeOfDay {
    /// `hour:minute:second` and `nanosecond` (below 10^9); 24-hour, no
    /// leap second.
    pub fn new(hour: u8, minute: u8, second: u8, nanosecond: u32) -> Result<Self, Error> {
        if hour > 23 || minute > 59 || second > 59 || nanosecond >= 1_000_000_000 {
            return Err(Error::Value(format!(
                "invalid time {hour:02}:{minute:02}:{second:02} (+{nanosecond} ns)"
            )));
        }
        Ok(Self {
            hour,
            minute,
            second,
            nanosecond,
        })
    }
    pub fn hour(&self) -> u8 {
        self.hour
    }
    pub fn minute(&self) -> u8 {
        self.minute
    }
    pub fn second(&self) -> u8 {
        self.second
    }
    pub fn nanosecond(&self) -> u32 {
        self.nanosecond
    }
    fn seconds_of_day(&self) -> i64 {
        i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }
}

impl FromStr for TimeOfDay {
    type Err = Error;
    /// `HH:MM:SS`, with an optional fraction of 1–9 digits.
    fn from_str(text: &str) -> Result<Self, Error> {
        let mut cursor = Text::new(text);
        cursor
            .time()
            .filter(|_| cursor.end().is_some())
            .ok_or_else(|| {
                invalid(
                    "time",
                    text,
                    "HH:MM:SS[.fraction], 24-hour, 0–9 fraction digits",
                )
            })
    }
}

impl fmt::Display for TimeOfDay {
    /// The shortest text: no fraction for a whole second, and no trailing
    /// zeros in one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}{}",
            self.hour,
            self.minute,
            self.second,
            Fraction(self.nanosecond)
        )
    }
}

/// A local date and time without a time zone (tag `datetime`): passed as
/// it is, never read as a point in time in some zone (design V5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime {
    date: Date,
    time: TimeOfDay,
}

impl DateTime {
    pub fn new(date: Date, time: TimeOfDay) -> Self {
        Self { date, time }
    }
    pub fn date(&self) -> Date {
        self.date
    }
    pub fn time(&self) -> TimeOfDay {
        self.time
    }
}

impl FromStr for DateTime {
    type Err = Error;
    /// `YYYY-MM-DDTHH:MM:SS`, with an optional fraction; no zone.
    fn from_str(text: &str) -> Result<Self, Error> {
        let mut cursor = Text::new(text);
        cursor
            .datetime()
            .filter(|_| cursor.end().is_some())
            .ok_or_else(|| {
                invalid(
                    "datetime",
                    text,
                    "YYYY-MM-DDTHH:MM:SS[.fraction] without a time zone, years 0001–9999",
                )
            })
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{}", self.date, self.time)
    }
}

// ── Instant ──────────────────────────────────────────────────────────

/// A point in time (tag `instant`), to the nanosecond, from
/// 0001-01-01T00:00:00Z to 9999-12-31T23:59:59.999999999Z. A far end
/// with a coarser clock (JS milliseconds, Python microseconds) truncates
/// it toward negative infinity when it receives it (design V7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    /// Seconds since 1970-01-01T00:00:00Z.
    seconds: i64,
    nanosecond: u32,
}

impl Instant {
    const MIN_SECONDS: i64 = -62_135_596_800; // 0001-01-01T00:00:00Z
    const MAX_SECONDS: i64 = 253_402_300_799; // 9999-12-31T23:59:59Z

    /// `seconds` since 1970-01-01T00:00:00Z (negative before) plus
    /// `nanosecond` (below 10^9), if within years 0001–9999.
    pub fn from_unix(seconds: i64, nanosecond: u32) -> Result<Self, Error> {
        if !(Self::MIN_SECONDS..=Self::MAX_SECONDS).contains(&seconds)
            || nanosecond >= 1_000_000_000
        {
            return Err(Error::Value(format!(
                "instant {seconds} s + {nanosecond} ns since 1970 is outside years 0001–9999"
            )));
        }
        Ok(Self {
            seconds,
            nanosecond,
        })
    }
    /// Nanoseconds since 1970-01-01T00:00:00Z.
    pub fn from_unix_nanos(nanos: i128) -> Result<Self, Error> {
        let seconds = i64::try_from(nanos.div_euclid(1_000_000_000))
            .map_err(|_| Error::Value(format!("instant {nanos} ns is outside years 0001–9999")))?;
        Self::from_unix(seconds, nanos.rem_euclid(1_000_000_000) as u32)
    }
    /// Whole seconds since 1970-01-01T00:00:00Z, rounded toward negative
    /// infinity.
    pub fn unix_seconds(&self) -> i64 {
        self.seconds
    }
    /// The nanoseconds past [`Instant::unix_seconds`].
    pub fn subsec_nanos(&self) -> u32 {
        self.nanosecond
    }
    pub fn unix_nanos(&self) -> i128 {
        i128::from(self.seconds) * 1_000_000_000 + i128::from(self.nanosecond)
    }
    /// This instant's date and time of day in UTC.
    pub fn to_utc(&self) -> DateTime {
        let days = self.seconds.div_euclid(SECONDS_PER_DAY);
        let second = self.seconds.rem_euclid(SECONDS_PER_DAY);
        let date = Date::from_days_since_unix_epoch(days).expect("an instant is within range");
        let time = TimeOfDay::new(
            (second / 3600) as u8,
            (second % 3600 / 60) as u8,
            (second % 60) as u8,
            self.nanosecond,
        )
        .expect("a second of the day");
        DateTime::new(date, time)
    }
    /// The instant at which it is `utc` in UTC.
    pub fn from_utc(utc: DateTime) -> Self {
        Self {
            seconds: utc.date.days_since_unix_epoch() * SECONDS_PER_DAY + utc.time.seconds_of_day(),
            nanosecond: utc.time.nanosecond,
        }
    }
    /// The same instant as a `SystemTime`, where the platform's
    /// `SystemTime` reaches it.
    pub fn to_system_time(&self) -> Option<SystemTime> {
        if self.seconds >= 0 {
            UNIX_EPOCH.checked_add(Duration::new(self.seconds as u64, self.nanosecond))
        } else {
            UNIX_EPOCH
                .checked_sub(Duration::from_secs(self.seconds.unsigned_abs()))?
                .checked_add(Duration::from_nanos(u64::from(self.nanosecond)))
        }
    }
}

impl TryFrom<SystemTime> for Instant {
    type Error = Error;
    fn try_from(time: SystemTime) -> Result<Self, Error> {
        let out_of_range = || Error::Value("the time is outside years 0001–9999".into());
        let nanos = match time.duration_since(UNIX_EPOCH) {
            Ok(after) => i128::try_from(after.as_nanos()).map_err(|_| out_of_range())?,
            Err(before) => {
                -i128::try_from(before.duration().as_nanos()).map_err(|_| out_of_range())?
            }
        };
        Self::from_unix_nanos(nanos)
    }
}

impl FromStr for Instant {
    type Err = Error;
    /// RFC 3339 in UTC: `YYYY-MM-DDTHH:MM:SS`, an optional fraction of
    /// 1–9 digits, and `Z`.
    fn from_str(text: &str) -> Result<Self, Error> {
        let mut cursor = Text::new(text);
        cursor
            .datetime()
            .filter(|_| cursor.byte(b'Z').is_some() && cursor.end().is_some())
            .map(Self::from_utc)
            .ok_or_else(|| {
                invalid(
                    "instant",
                    text,
                    "RFC 3339 in UTC, YYYY-MM-DDTHH:MM:SS[.fraction]Z, years 0001–9999",
                )
            })
    }
}

impl fmt::Display for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}Z", self.to_utc())
    }
}

// ── Big integers and decimals ────────────────────────────────────────

/// An integer of any size up to [`MAX_DIGITS`] digits (tag `bigint`),
/// kept as its decimal text. A `BigInt` always crosses as one, whatever
/// its size. Rust integers need none: one beyond ±(2^53−1) crosses as a
/// bigint by itself (design V6). A `BigInt` holds an integer past 128
/// bits, or one received that must go on as a bigint.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BigInt(String);

impl BigInt {
    /// The decimal text: an optional `-`, no leading zeros.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_negative(&self) -> bool {
        self.0.starts_with('-')
    }
    /// The value, if it fits an `i128`.
    pub fn to_i128(&self) -> Option<i128> {
        self.0.parse().ok()
    }
    /// The value, if it fits a `u128`.
    pub fn to_u128(&self) -> Option<u128> {
        self.0.parse().ok()
    }
    /// The value as a JSON number, if it fits an `i64` or a `u64`.
    pub(crate) fn to_json_number(&self) -> Option<serde_json::Number> {
        match self.0.parse::<i64>() {
            Ok(value) => Some(value.into()),
            Err(_) => self.0.parse::<u64>().ok().map(Into::into),
        }
    }
}

/// `-`, then digits with no leading zero; `digits` counts them.
fn integer_text(text: &str) -> Option<(bool, usize)> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let valid = !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    valid.then_some((negative, digits.len()))
}

impl FromStr for BigInt {
    type Err = Error;
    /// Decimal digits with an optional `-`; no leading zeros, no `-0`, at
    /// most [`MAX_DIGITS`] digits.
    fn from_str(text: &str) -> Result<Self, Error> {
        match integer_text(text) {
            Some((negative, digits)) if digits <= MAX_DIGITS && !(negative && text == "-0") => {
                Ok(Self(text.to_owned()))
            }
            _ => Err(invalid(
                "bigint",
                text,
                "a decimal integer without leading zeros, at most 4300 digits",
            )),
        }
    }
}

impl fmt::Display for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

macro_rules! integers {
    ($($int:ty),*) => {$(
        impl From<$int> for BigInt {
            fn from(value: $int) -> Self {
                Self(value.to_string())
            }
        }
        impl From<$int> for Decimal {
            fn from(value: $int) -> Self {
                Self(value.to_string())
            }
        }
    )*};
}
integers!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

/// A decimal number (tag `decimal`) kept as its text: no exponent, its
/// trailing zeros kept (`1.10` is not `1.1`), at most [`MAX_DIGITS`]
/// digits. Convert with the decimal crate of your choice through
/// [`Decimal::as_str`] and [`FromStr`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Decimal(String);

impl Decimal {
    /// The decimal text: an optional `-`, an integer part without leading
    /// zeros, an optional fraction.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Decimal {
    type Err = Error;
    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, at most [`MAX_DIGITS`] digits.
    /// NaN, infinities and exponents are refused.
    fn from_str(text: &str) -> Result<Self, Error> {
        let (integer, fraction) = match text.split_once('.') {
            Some((integer, fraction)) => (integer, Some(fraction)),
            None => (text, None),
        };
        let valid = integer_text(integer).is_some_and(|(_, digits)| {
            let fraction = fraction.map_or(Some(0), |fraction| {
                (!fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit()))
                    .then_some(fraction.len())
            });
            fraction.is_some_and(|fraction| digits + fraction <= MAX_DIGITS)
        });
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(invalid(
                "decimal",
                text,
                "decimal digits with an optional fraction, no exponent, at most 4300 digits",
            ))
        }
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ── UUID ─────────────────────────────────────────────────────────────

/// A UUID (tag `uuid`), any version; written in lowercase `8-4-4-4-12`
/// hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid([u8; 16]);

impl Uuid {
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
    pub const fn from_u128(value: u128) -> Self {
        Self(value.to_be_bytes())
    }
    pub const fn as_u128(&self) -> u128 {
        u128::from_be_bytes(self.0)
    }
}

impl FromStr for Uuid {
    type Err = Error;
    /// Lowercase `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
    fn from_str(text: &str) -> Result<Self, Error> {
        let refused = || invalid("uuid", text, "lowercase hex 8-4-4-4-12");
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return Err(refused());
        }
        let mut value = [0u8; 16];
        let mut nibbles = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                if byte != b'-' {
                    return Err(refused());
                }
                continue;
            }
            let nibble = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                _ => return Err(refused()),
            };
            value[nibbles / 2] |= nibble << if nibbles % 2 == 0 { 4 } else { 0 };
            nibbles += 1;
        }
        Ok(Self(value))
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

// ── Base64 (RFC 4648 §4, padded) ─────────────────────────────────────

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `bytes` in base64 with padding.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                text.push(ALPHABET[(group >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// The bytes of padded base64 text. Only the canonical text is taken:
/// no whitespace, padding only at the end, unused bits zero.
pub(crate) fn base64_decode(text: &str) -> Result<Vec<u8>, Error> {
    let refused = || {
        Error::Value(format!(
            "invalid bytes ({} characters): expected base64 (RFC 4648 §4) with padding",
            text.len()
        ))
    };
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(refused());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let groups = bytes.len() / 4;
    for (index, group) in bytes.chunks(4).enumerate() {
        let padding = group.iter().rev().take_while(|&&byte| byte == b'=').count();
        if padding > 2 || (padding > 0 && index + 1 != groups) {
            return Err(refused());
        }
        let mut bits = 0u32;
        for &byte in &group[..4 - padding] {
            let sextet = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return Err(refused()),
            };
            bits = bits << 6 | u32::from(sextet);
        }
        bits <<= 6 * padding as u32;
        let decoded = [(bits >> 16) as u8, (bits >> 8) as u8, bits as u8];
        let kept = 3 - padding;
        if decoded[kept..].iter().any(|&byte| byte != 0) {
            return Err(refused());
        }
        out.extend_from_slice(&decoded[..kept]);
    }
    Ok(out)
}

// ── Map keys and set elements (design §5) ────────────────────────────

/// A map key or set element as the languages compare it: numbers by value
/// (`1` and `1.0` are one key, as are `0` and `-0`).
#[derive(PartialEq, Eq, Hash)]
enum Key {
    Text(String),
    Integer(i128),
    Float(u64),
    Bool(bool),
    BigInt(String),
    DateTime(DateTime),
    Date(Date),
    Time(TimeOfDay),
    Decimal(String),
    Uuid(Uuid),
}

/// What `value` is, for errors: "null", "an instant", ...
pub(crate) fn describe(value: &Value) -> &'static str {
    use serde_json::Value as Json;
    match value {
        Value::Undefined => "undefined",
        Value::Data(Json::Null) => "null",
        Value::Data(Json::Bool(_)) => "a boolean",
        Value::Data(Json::Number(_)) => "a number",
        Value::Data(Json::String(_)) => "a string",
        Value::Data(Json::Array(_)) | Value::List(_) => "an array",
        Value::Data(Json::Object(_)) | Value::Record(_) => "an object",
        Value::Reference(_) => "a reference",
        Value::Signal => "a signal",
        Value::Instant(_) => "an instant",
        Value::DateTime(_) => "a datetime",
        Value::Date(_) => "a date",
        Value::Time(_) => "a time",
        Value::Bytes(_) => "bytes",
        Value::Map(_) => "a map",
        Value::Set(_) => "a set",
        Value::BigInt(_) => "a bigint",
        Value::Decimal(_) => "a decimal",
        Value::Uuid(_) => "a uuid",
    }
}

/// The largest integer every language holds exactly, 2^53−1.
pub(crate) const SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Whether `number` is an integer beyond ±(2^53−1), which crosses as a
/// bigint (design V6).
pub(crate) fn beyond_safe(number: &serde_json::Number) -> bool {
    match (number.as_i64(), number.as_u64()) {
        (Some(integer), _) => integer.unsigned_abs() > SAFE_INTEGER,
        (None, Some(integer)) => integer > SAFE_INTEGER,
        (None, None) => false,
    }
}

/// Whether JSON data holds an integer beyond ±(2^53−1).
pub(crate) fn holds_beyond_safe(json: &serde_json::Value) -> bool {
    use serde_json::Value as Json;
    match json {
        Json::Number(number) => beyond_safe(number),
        Json::Array(items) => items.iter().any(holds_beyond_safe),
        Json::Object(fields) => fields.values().any(holds_beyond_safe),
        _ => false,
    }
}

fn key(value: &Value, what: &str) -> Result<Key, Error> {
    use serde_json::Value as Json;
    Ok(match value {
        Value::Data(Json::String(text)) => Key::Text(text.clone()),
        Value::Data(Json::Bool(flag)) => Key::Bool(*flag),
        // An integer beyond ±(2^53−1) crosses as a bigint (design V6): it
        // is the key that bigint is.
        Value::Data(Json::Number(number)) if beyond_safe(number) => Key::BigInt(number.to_string()),
        Value::Data(Json::Number(number)) => match (number.as_i64(), number.as_u64()) {
            (Some(integer), _) => Key::Integer(integer.into()),
            (None, Some(integer)) => Key::Integer(integer.into()),
            _ => {
                let float = number.as_f64().unwrap_or(f64::NAN);
                if float.fract() != 0.0 {
                    Key::Float(float.to_bits())
                } else if float.abs() <= SAFE_INTEGER as f64 {
                    Key::Integer(float as i128)
                } else {
                    // Not exact: in JavaScript 2^53 and 2^53+1 are one
                    // number, so two such keys may be one there.
                    return Err(Error::Value(format!(
                        "a {what} cannot be the float {number}: a whole number beyond \
                         ±(2^53-1) written as a float is not exact in every language"
                    )));
                }
            }
        },
        Value::BigInt(value) => Key::BigInt(value.0.clone()),
        Value::DateTime(value) => Key::DateTime(*value),
        Value::Date(value) => Key::Date(*value),
        Value::Time(value) => Key::Time(*value),
        Value::Decimal(value) => Key::Decimal(value.0.clone()),
        Value::Uuid(value) => Key::Uuid(*value),
        other => {
            return Err(Error::Value(format!(
                "a {what} cannot be {}: only strings, finite numbers, booleans, bigint, \
                 datetime, date, time, decimal and uuid values can (an instant, bytes, null, \
                 an array, an object, a map, a set or a reference cannot)",
                describe(other)
            )))
        }
    })
}

/// Check the keys of a map (`what` "map key") or the elements of a set
/// ("set element"): each of a kind every language takes as a key, and no
/// two equal, since a language that merges them would lose an entry.
pub(crate) fn check_keys<'a>(
    keys: impl IntoIterator<Item = &'a Value>,
    what: &str,
) -> Result<(), Error> {
    let mut seen = HashSet::new();
    for value in keys {
        if !seen.insert(key(value, what)?) {
            return Err(Error::Value(format!(
                "duplicate {what} {}: a map or set whose keys collide would lose entries",
                shown(value)
            )));
        }
    }
    Ok(())
}

/// A key as an error shows it.
fn shown(value: &Value) -> String {
    let text = match value {
        Value::Data(json) => json.to_string(),
        Value::BigInt(value) => value.to_string(),
        Value::Decimal(value) => value.to_string(),
        _ => String::new(),
    };
    if !text.is_empty() {
        let short = shortened(&text);
        return if short.len() < text.len() {
            format!("{short}…")
        } else {
            text
        };
    }
    match value {
        Value::DateTime(value) => value.to_string(),
        Value::Date(value) => value.to_string(),
        Value::Time(value) => value.to_string(),
        Value::Uuid(value) => value.to_string(),
        other => describe(other).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn instants_read_and_write_rfc_3339_in_utc() {
        for (text, seconds, nanos) in [
            ("1970-01-01T00:00:00Z", 0, 0),
            ("2026-10-15T03:22:15.123456789Z", 1_792_034_535, 123_456_789),
            ("2026-10-15T03:22:15.1Z", 1_792_034_535, 100_000_000),
            ("1969-12-31T23:59:59.999999999Z", -1, 999_999_999),
            ("0001-01-01T00:00:00Z", Instant::MIN_SECONDS, 0),
            (
                "9999-12-31T23:59:59.999999999Z",
                Instant::MAX_SECONDS,
                999_999_999,
            ),
            ("2000-02-29T12:00:00Z", 951_825_600, 0),
        ] {
            let instant: Instant = text.parse().unwrap();
            assert_eq!(
                (instant.unix_seconds(), instant.subsec_nanos()),
                (seconds, nanos),
                "{text}"
            );
            assert_eq!(instant.to_string(), text);
            assert_eq!(Instant::from_unix(seconds, nanos).unwrap(), instant);
        }
        // Trailing zeros of a fraction are not kept: the same instant.
        let padded: Instant = "2026-10-15T03:22:15.500000000Z".parse().unwrap();
        assert_eq!(padded.to_string(), "2026-10-15T03:22:15.5Z");
        for text in [
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
        ] {
            assert!(text.parse::<Instant>().is_err(), "{text:?} was taken");
        }
    }

    #[test]
    fn instants_convert_to_unix_time_and_system_time() {
        assert!(Instant::from_unix(Instant::MIN_SECONDS - 1, 0).is_err());
        assert!(Instant::from_unix(Instant::MAX_SECONDS + 1, 0).is_err());
        assert!(Instant::from_unix(0, 1_000_000_000).is_err());
        let instant = Instant::from_unix_nanos(-1).unwrap();
        assert_eq!(
            (instant.unix_seconds(), instant.subsec_nanos()),
            (-1, 999_999_999)
        );
        assert_eq!(instant.unix_nanos(), -1);
        let now = SystemTime::now();
        let instant = Instant::try_from(now).unwrap();
        assert_eq!(instant.to_system_time(), Some(now));
        // A whole number of 100 ns: Windows keeps `SystemTime` in 100 ns
        // units, so a finer time would already be rounded when built.
        let before = UNIX_EPOCH - Duration::new(5, 300);
        let instant = Instant::try_from(before).unwrap();
        assert_eq!(instant.to_string(), "1969-12-31T23:59:54.9999997Z");
        assert_eq!(instant.to_system_time(), Some(before));
        let utc = instant.to_utc();
        assert_eq!(Instant::from_utc(utc), instant);
    }

    #[test]
    fn local_dates_and_times_keep_their_text_without_a_zone() {
        for text in ["2026-10-15", "0001-01-01", "9999-12-31", "2000-02-29"] {
            assert_eq!(text.parse::<Date>().unwrap().to_string(), text);
        }
        for text in [
            "2026-13-01",
            "2026-00-01",
            "2026-04-31",
            "1900-02-29",
            "0000-01-01",
            "2026-10-15T00:00:00",
            "20261015",
        ] {
            assert!(text.parse::<Date>().is_err(), "{text}");
        }
        for text in [
            "14:30:05",
            "14:30:05.25",
            "00:00:00.000000001",
            "23:59:59.999999999",
        ] {
            assert_eq!(text.parse::<TimeOfDay>().unwrap().to_string(), text);
        }
        assert_eq!(
            "14:30:05.250".parse::<TimeOfDay>().unwrap().to_string(),
            "14:30:05.25"
        );
        for text in [
            "14:30",
            "24:00:00",
            "14:60:00",
            "14:30:05Z",
            "14:30:05+01:00",
            "14:30:05.",
            "1:30:05",
        ] {
            assert!(text.parse::<TimeOfDay>().is_err(), "{text}");
        }
        let local: DateTime = "2026-10-15T14:30:05.25".parse().unwrap();
        assert_eq!(local.to_string(), "2026-10-15T14:30:05.25");
        assert_eq!(local.date(), Date::new(2026, 10, 15).unwrap());
        assert_eq!(local.time().nanosecond(), 250_000_000);
        for text in [
            "2026-10-15T14:30:05Z",
            "2026-10-15T14:30:05+02:00",
            "2026-10-15",
            "2026-10-15T14:30",
        ] {
            assert!(text.parse::<DateTime>().is_err(), "{text}");
        }
        let first = Date::new(1, 1, 1).unwrap().days_since_unix_epoch();
        assert_eq!(
            Date::from_days_since_unix_epoch(first).unwrap().to_string(),
            "0001-01-01"
        );
        assert!(Date::from_days_since_unix_epoch(first - 1).is_err());
        assert_eq!(
            Date::from_days_since_unix_epoch(0).unwrap().to_string(),
            "1970-01-01"
        );
    }

    #[test]
    fn big_integers_keep_every_digit_up_to_the_limit() {
        for text in [
            "0",
            "-1",
            "123456789012345678901234567890",
            "-170141183460469231731687303715884105729",
        ] {
            assert_eq!(text.parse::<BigInt>().unwrap().as_str(), text);
        }
        let most = "9".repeat(MAX_DIGITS);
        assert!(most.parse::<BigInt>().is_ok());
        assert!(format!("-{most}").parse::<BigInt>().is_ok());
        assert!(format!("{most}9").parse::<BigInt>().is_err());
        for text in ["", "-", "-0", "01", "+1", "1.0", "1e3", " 1", "0x10", "١"] {
            assert!(text.parse::<BigInt>().is_err(), "{text:?}");
        }
        assert_eq!(BigInt::from(i128::MIN).to_i128(), Some(i128::MIN));
        assert_eq!(BigInt::from(u128::MAX).to_u128(), Some(u128::MAX));
        assert_eq!(BigInt::from(u128::MAX).to_i128(), None);
        assert!(BigInt::from(-5).is_negative());
    }

    #[test]
    fn decimals_keep_their_text_and_trailing_zeros() {
        for text in [
            "0",
            "-0",
            "-0.0",
            "1.10",
            "1.1",
            "-12.3400",
            "0.000001",
            "123456789012345678901234567890.5",
        ] {
            assert_eq!(text.parse::<Decimal>().unwrap().as_str(), text);
        }
        assert_ne!("1.10".parse::<Decimal>().unwrap(), "1.1".parse().unwrap());
        let most = format!("{}.{}", "1".repeat(MAX_DIGITS - 10), "0".repeat(10));
        assert!(most.parse::<Decimal>().is_ok());
        assert!(format!("{most}0").parse::<Decimal>().is_err());
        for text in [
            "", ".5", "5.", "1e3", "1E-3", "NaN", "Infinity", "-inf", "00.1", "+1", "1.2.3", "1,5",
        ] {
            assert!(text.parse::<Decimal>().is_err(), "{text:?}");
        }
        assert_eq!(Decimal::from(-7i64).as_str(), "-7");
    }

    #[test]
    fn uuids_are_lowercase_8_4_4_4_12() {
        let text = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let uuid: Uuid = text.parse().unwrap();
        assert_eq!(uuid.to_string(), text);
        assert_eq!(Uuid::from_u128(uuid.as_u128()), uuid);
        assert_eq!(uuid.as_bytes()[0], 0x0f);
        for text in [
            "0F8FAD5B-D9CB-469F-A165-70867728950E",
            "0f8fad5bd9cb469fa16570867728950e",
            "{0f8fad5b-d9cb-469f-a165-70867728950e}",
            "0f8fad5b-d9cb-469f-a165-70867728950",
            "0f8fad5b-d9cb-469f-a165-70867728950g",
            "0f8fad5b+d9cb-469f-a165-70867728950e",
        ] {
            assert!(text.parse::<Uuid>().is_err(), "{text}");
        }
    }

    #[test]
    fn bytes_use_padded_canonical_base64() {
        for (bytes, text) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"hello", "aGVsbG8="),
            (&[0xff, 0xfe, 0x00][..], "//4A"),
        ] {
            assert_eq!(base64_encode(bytes), text);
            assert_eq!(base64_decode(text).unwrap(), bytes);
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&all)).unwrap(), all);
        for text in [
            "Zg", "Zg=", "Zh==", "Zg==Zg==", "Z===", "Zm9v\n", "Zm-v", "Zm_v", "====",
        ] {
            assert!(base64_decode(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn map_keys_are_values_in_every_language_and_never_collide() {
        let ok = [
            Value::Data(json!("a")),
            Value::Data(json!(1)),
            Value::Data(json!(true)),
            Value::BigInt(1.into()),
            Value::Decimal("1.0".parse().unwrap()),
            Value::Date("2026-10-15".parse().unwrap()),
            Value::Time("14:30:00".parse().unwrap()),
            Value::DateTime("2026-10-15T14:30:00".parse().unwrap()),
            Value::Uuid(Uuid::from_u128(1)),
        ];
        check_keys(&ok, "map key").unwrap();
        for refused in [
            Value::Instant(Instant::from_unix(0, 0).unwrap()),
            Value::Bytes(vec![]),
            Value::Data(json!(null)),
            Value::Undefined,
            Value::Data(json!([])),
            Value::Data(json!({})),
            Value::List(vec![]),
            Value::Map(vec![]),
            Value::Set(vec![]),
            Value::Signal,
        ] {
            let error = check_keys([&refused], "map key").unwrap_err();
            assert!(error.to_string().contains("a map key cannot be"), "{error}");
        }
        // Equal as the languages compare them.
        for (a, b) in [
            (json!(1), json!(1.0)),
            (json!(0.0), json!(-0.0)),
            (json!("x"), json!("x")),
            (json!(1.5), json!(1.5)),
        ] {
            let error = check_keys(&[Value::Data(a), Value::Data(b)], "set element").unwrap_err();
            assert!(
                error.to_string().contains("duplicate set element"),
                "{error}"
            );
        }
        // Different kinds, or different text, are different keys.
        check_keys(
            &[
                Value::Data(json!(1)),
                Value::Data(json!("1")),
                Value::BigInt(1.into()),
                Value::Decimal("1".parse().unwrap()),
                Value::Decimal("1.0".parse().unwrap()),
                Value::Data(json!(SAFE_INTEGER)),
                Value::Data(json!(-(SAFE_INTEGER as i64))),
                Value::Data(json!(SAFE_INTEGER - 1)),
            ],
            "map key",
        )
        .unwrap();
        // Integers beyond ±(2^53−1) are keys as the bigints they cross as
        // (design V6): equal to the same bigint, and the float nearest
        // them, not exact, is refused.
        for beyond in [
            json!(SAFE_INTEGER + 1),
            json!(SAFE_INTEGER + 2),
            json!(-(SAFE_INTEGER as i64) - 1),
            json!(u64::MAX),
            json!(i64::MIN),
        ] {
            check_keys(
                &[
                    Value::Data(beyond.clone()),
                    Value::Data(json!(SAFE_INTEGER)),
                ],
                "map key",
            )
            .unwrap();
            let same = Value::BigInt(beyond.to_string().parse().unwrap());
            let error = check_keys(&[Value::Data(beyond), same], "set element").unwrap_err();
            assert!(
                error.to_string().contains("duplicate set element"),
                "{error}"
            );
        }
        check_keys(
            &[
                Value::Data(json!(SAFE_INTEGER + 1)),
                Value::Data(json!(SAFE_INTEGER + 2)),
                Value::Data(json!(SAFE_INTEGER as f64)),
                Value::Data(json!(-(SAFE_INTEGER as f64))),
            ],
            "map key",
        )
        .unwrap();
        for float in [9007199254740993.0, 9007199254740992.0, -1e300, 1e20] {
            let error = check_keys(&[Value::Data(json!(float))], "map key").unwrap_err();
            assert!(
                error.to_string().contains(&format!(
                    "a map key cannot be the float {}: a whole number beyond ±(2^53-1)",
                    json!(float)
                )),
                "{error}"
            );
        }
    }

    #[test]
    fn days_far_outside_the_calendar_are_errors_not_overflows() {
        for days in [i64::MAX, i64::MIN, i64::MAX - 719_468, 2_932_897, -719_163] {
            assert!(Date::from_days_since_unix_epoch(days).is_err(), "{days}");
        }
        assert_eq!(
            Date::from_days_since_unix_epoch(2_932_896)
                .unwrap()
                .to_string(),
            "9999-12-31"
        );
        assert_eq!(
            Date::from_days_since_unix_epoch(-719_162)
                .unwrap()
                .to_string(),
            "0001-01-01"
        );
    }
}
