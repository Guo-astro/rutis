//! Bounded proofs of the session's pure date and time-of-day logic
//! (#631; `crates/rutis-bridge/src/session/values.rs`). Each proof is
//! checked over every value of its inputs, not a sample.
//!
//! Run from this directory, with Kani installed (`cargo kani setup`):
//!
//!     cargo kani
//!
//! Not verified yet: Kani 0.68.0 does not finish these harnesses here
//! (each was still checking after 90 minutes, and after 7 minutes again on
//! 2026-11-17). `fuzz/KANI-MIRI.md` records the runs; a harness whose inputs
//! are held to a smaller range says so.
//!
//! Why, minimized to no rutis code: `fuzz/kani/limits` — `format!` with one
//! symbolic argument and `str::from_utf8` over one symbolic byte, both hanging
//! next to the controls that finish in a second. Every path of these
//! constructors formats its error that way, and the error branch is still
//! explored when `kani::assume` makes it unreachable: its condition is
//! symbolic. So no shape at the call site avoids it; `fuzz/KANI-MIRI.md`'s
//! last section says what would.
//!
//! guarantee: bridge/value-tags, bridge/value-roundtrip
#![cfg(kani)]

use rutis_bridge::session::{Date, Instant, TimeOfDay};

/// The days `Date::from_days_since_unix_epoch` spans: 0001-01-01 to
/// 9999-12-31.
const MIN_DAYS: i64 = -719_162;
const MAX_DAYS: i64 = 2_932_896;

/// A date the constructor accepts keeps the fields it was given and stays
/// in the range the `date` tag can carry (years 0001–9999, months 1–12,
/// days 1–31): no normalization, no wrap.
#[kani::proof]
fn date_new_keeps_valid_fields() {
    let year: u16 = kani::any();
    let month: u8 = kani::any();
    let day: u8 = kani::any();
    if let Ok(date) = Date::new(year, month, day) {
        assert!((1..=9999).contains(&year));
        assert!((1..=12).contains(&month));
        assert!((1..=31).contains(&day));
        assert_eq!(date.year(), year);
        assert_eq!(date.month(), month);
        assert_eq!(date.day(), day);
    }
}

/// No date has month 0 or day 0, whatever the other fields are.
#[kani::proof]
fn date_new_rejects_month_and_day_zero() {
    let year: u16 = kani::any();
    let month: u8 = kani::any();
    let day: u8 = kani::any();
    if month == 0 || day == 0 {
        assert!(Date::new(year, month, day).is_err());
    }
}

/// Every day in years 0001–9999, written as the number of days from
/// 1970-01-01, reads back as the same date and the same number. The
/// bounds are `Date::from_days_since_unix_epoch`'s own (0001-01-01 is
/// −719162 before 1970-01-01; 9999-12-31 is 2932896 after it).
#[kani::proof]
fn days_from_epoch_roundtrip() {
    let days: i64 = kani::any();
    kani::assume((MIN_DAYS..=MAX_DAYS).contains(&days));
    let date =
        Date::from_days_since_unix_epoch(days).expect("a day in 0001–9999 is a date in 0001–9999");
    assert_eq!(date.days_since_unix_epoch(), days);
}

/// A time of day the constructor accepts keeps every field: hour 0–23,
/// minute and second 0–59, nanosecond below 10^9, no leap second and no
/// rounding.
#[kani::proof]
fn time_of_day_keeps_valid_fields() {
    let hour: u8 = kani::any();
    let minute: u8 = kani::any();
    let second: u8 = kani::any();
    let nanosecond: u32 = kani::any();
    if let Ok(time) = TimeOfDay::new(hour, minute, second, nanosecond) {
        assert!(hour <= 23);
        assert!(minute <= 59);
        assert!(second <= 59);
        assert!(nanosecond < 1_000_000_000);
        assert_eq!(time.hour(), hour);
        assert_eq!(time.minute(), minute);
        assert_eq!(time.second(), second);
        assert_eq!(time.nanosecond(), nanosecond);
    }
}

/// An instant, split into a UTC date and time of day and built back from
/// them, is the same instant, with no nanosecond lost and the sign of a
/// time before 1970 kept.
#[kani::proof]
fn instant_reads_back_through_utc() {
    let seconds: i64 = kani::any();
    let nanosecond: u32 = kani::any();
    kani::assume(nanosecond < 1_000_000_000);
    // A year's worth of seconds around 1970: the property does not depend
    // on the year, and the full range takes longer than it is worth here.
    kani::assume((-31_536_000..=31_536_000).contains(&seconds));
    let instant = Instant::from_unix(seconds, nanosecond)
        .expect("a second within a year of 1970 is inside 0001–9999");
    let utc = instant.to_utc();
    assert_eq!(Instant::from_utc(utc), instant);
    assert_eq!(
        utc.date().days_since_unix_epoch() * 86_400
            + i64::from(utc.time().hour()) * 3_600
            + i64::from(utc.time().minute()) * 60
            + i64::from(utc.time().second()),
        seconds
    );
    assert_eq!(utc.time().nanosecond(), nanosecond);
}

/// Nanoseconds since 1970-01-01 within years 0001–9999, split into whole
/// seconds (rounded toward negative infinity, so that a time before 1970
/// keeps its nanosecond) and the nanoseconds past them, read back as the
/// same number.
#[kani::proof]
fn instant_nanoseconds_roundtrip() {
    let nanos: i128 = kani::any();
    // One day either side of 1970: the split at 10^9 does not depend on the
    // magnitude, and symbolic 128-bit division over the full range is slow.
    kani::assume((-86_400_000_000_000..=86_400_000_000_000 + 999_999_999).contains(&nanos));
    let instant = Instant::from_unix_nanos(nanos).expect("a day either side of 1970 is in range");
    assert_eq!(instant.unix_nanos(), nanos);
}
