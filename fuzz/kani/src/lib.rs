//! Bounded proofs of the session's pure date and time-of-day logic
//! (#631; `crates/rutis-bridge/src/session/values.rs`) and of the plugin boot
//! blob's const ABI (#1271; `fuzz/kani/src/boot.rs`). Each proof is checked
//! over every value of its inputs, not a sample.
//!
//! Run from this directory, with Kani installed (`cargo kani setup`):
//!
//!     cargo kani
//!
//! The constructors call the predicates these prove: `Date::try_new`,
//! `TimeOfDay::try_new`, `Date::try_from_days_since_unix_epoch`,
//! `Instant::try_from_unix` and `Instant::try_from_unix_nanos`, which check
//! the fields and return `None` where `new`/`from_unix` build an error
//! message. Kani 0.68.0 does not finish `format!` over a symbolic number
//! (`fuzz/kani/limits`), so the message is off the proved path;
//! `constructors_accept_what_the_predicates_accept` in `values.rs` checks
//! that each constructor accepts exactly what its predicate accepts and
//! rejects with the message it had.
//!
//! A harness whose inputs are held to a smaller range says so, and
//! `fuzz/KANI-MIRI.md` records every run: the toolchain, the command and how
//! long it took.
//!
//! guarantee: bridge/value-tags, bridge/value-roundtrip
#![cfg(kani)]

use rutis_bridge::session::{Date, Instant, TimeOfDay};

mod boot;

/// The days `Date::from_days_since_unix_epoch` spans: 0001-01-01 to
/// 9999-12-31.
const MIN_DAYS: i64 = -719_162;
const MAX_DAYS: i64 = 2_932_896;

/// The range the nanosecond round trip is proved over: 10 000 seconds either
/// side of 1970 (2h46m40s). Symbolic 128-bit division is what costs here — a
/// day either side had no verdict after 15 minutes on 2026-11-18 — so this is
/// the range proved, not all of `i128`.
const MIN_NANOS: i128 = -10_000_000_000_000;
const MAX_NANOS: i128 = 10_000_000_000_000 + 999_999_999;

/// A date the constructor accepts keeps the fields it was given and stays
/// in the range the `date` tag can carry (years 0001–9999, months 1–12,
/// days 1–31): no normalization, no wrap.
#[kani::proof]
fn date_new_keeps_valid_fields() {
    let year: u16 = kani::any();
    let month: u8 = kani::any();
    let day: u8 = kani::any();
    if let Some(date) = Date::try_new(year, month, day) {
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
        assert!(Date::try_new(year, month, day).is_none());
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
    let date = Date::try_from_days_since_unix_epoch(days);
    assert!(date.is_some());
    assert_eq!(date.unwrap().days_since_unix_epoch(), days);
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
    if let Some(time) = TimeOfDay::try_new(hour, minute, second, nanosecond) {
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
    let instant = Instant::try_from_unix(seconds, nanosecond);
    assert!(instant.is_some());
    let instant = instant.unwrap();
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

/// Nanoseconds since 1970-01-01, split into whole seconds (rounded toward
/// negative infinity, so that a time before 1970 keeps its nanosecond) and
/// the nanoseconds past them, read back as the same number. The range is
/// [`MIN_NANOS`]–[`MAX_NANOS`], not all of years 0001–9999: see there.
#[kani::proof]
fn instant_nanoseconds_roundtrip() {
    let nanos: i128 = kani::any();
    kani::assume((MIN_NANOS..=MAX_NANOS).contains(&nanos));
    let instant = Instant::try_from_unix_nanos(nanos);
    assert!(instant.is_some());
    assert_eq!(instant.unwrap().unix_nanos(), nanos);
}
