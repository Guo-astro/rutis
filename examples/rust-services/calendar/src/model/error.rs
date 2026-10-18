use serde::Serialize;

/// Why a calendar call failed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum CalendarError {
    /// No entry has this id.
    NotFound { id: u64 },
    /// The entry is not acceptable.
    Invalid(String),
    /// The calendar holds as many entries as it can.
    Full,
}

impl std::fmt::Display for CalendarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { id } => write!(f, "no entry {id}"),
            Self::Invalid(why) => write!(f, "invalid entry: {why}"),
            Self::Full => write!(f, "the calendar is full"),
        }
    }
}

impl std::error::Error for CalendarError {}
