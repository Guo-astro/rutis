use std::sync::Arc;

use rutis::host::ErrorKind;
use rutis::Error;

/// A row that ended up `Failed` or `Unresolved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub id: String,
    pub error: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.id, self.error)
    }
}

fn list(failures: &[Failure]) -> String {
    failures
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// `cannot load "<name>": <message>`, or the message alone when it says
/// what cannot be loaded.
fn resolve_text(name: &str, message: &str) -> String {
    if message.starts_with("cannot load ") {
        message.to_owned()
    } else {
        format!("cannot load {name:?}: {message}")
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum LoaderError {
    /// No resolver knows this module name.
    #[error("no plugin named {name:?}")]
    NotFound { name: String },
    /// A resolver knows the name but could not load it. A runtime that
    /// says itself what it cannot load (`cannot load src/util.ts: …`) is
    /// not repeated.
    #[error("{}", resolve_text(name, message))]
    Resolve { name: String, message: String },
    /// The dry run (config validation, build, instance validation) failed.
    /// A config that does not match its schema shows as itself
    /// (`config: …`): where it is printed, the row is named.
    #[error("{}", rejected_text(id, error))]
    Rejected { id: String, error: Arc<ErrorKind> },
    #[error("no row {0}")]
    UnknownEntry(String),
    /// `create` with the id of a row there is.
    #[error("a row {0} already exists")]
    RowExists(String),
    #[error("invalid entry: {0}")]
    InvalidEntry(String),
    /// The edit cannot be expressed as a patch in the editable layer.
    #[error("entry {id:?} is not owned by the editable layer: {reason}")]
    NotOwned { id: String, reason: String },
    /// A layer above the editable one replaces the same field.
    #[error("{field} of {id:?} is overridden by layer {layer:?}")]
    OverriddenByLayer {
        id: String,
        field: String,
        layer: String,
    },
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Rows can be added or removed only in an editable layer.
    #[error("this host keeps no editable configuration, so rows cannot be added or removed")]
    NoEditableLayer,
    /// The edit validated but rows failed to start; it was rolled back:
    /// each row and its error.
    #[error("{}", list(.failures))]
    ApplyFailed { failures: Vec<Failure> },
    /// The rollback after `ApplyFailed` could not restore the old rows either.
    #[error(
        "the change failed ({}), and the rows as they were did not start again either ({})",
        list(.apply),
        list(.rollback)
    )]
    RollbackFailed {
        apply: Vec<Failure>,
        rollback: Vec<Failure>,
    },
    /// The stored layer kept changing under concurrent writers.
    #[error("persisted layer changed concurrently")]
    Conflict,
    /// The edit is live but could not be persisted; it stays queued.
    #[error("edit applied but not persisted: {0}")]
    PersistFailed(String),
    #[error("expression failed: {0}")]
    Expression(String),
    /// Service names the catalog does not know.
    #[error("unknown services: {}", .0.join(", "))]
    UnknownService(Vec<String>),
    /// A service inside instances of `group`, used where no such instance
    /// encloses the row.
    #[error(
        "{name:?} is a service inside {group:?} instances: put the row in the {group:?} group"
    )]
    OutsideInstance { name: String, group: String },
    /// An expression read a service that is not registered as readable.
    #[error("service {0:?} is not readable from expressions")]
    NotReadable(String),
    #[error("loader is closed")]
    Closed,
}

/// Why one copy of a row cannot run in its instance while copies in other
/// instances can: what the instance brings lacks something the row needs,
/// such as a runtime connection without the row's plugin. A scoped factory
/// ([`crate::ScopedFactory`]) returns it in [`ErrorKind::PluginFailed`](rutis::host::ErrorKind::PluginFailed).
/// The copy fails with it in that instance alone: an edit is neither
/// refused by the dry run nor rolled back because of it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unavailable(pub String);

impl Unavailable {
    /// `self` as the error a scoped factory returns.
    pub fn into_error(self) -> Error {
        rutis::Error::from(ErrorKind::PluginFailed(Box::new(self)))
    }

    /// Whether `error` says a copy is [`Unavailable`] in its instance.
    pub fn is(error: &LoaderError) -> bool {
        match error {
            LoaderError::Rejected { error, .. } => matches!(
                &**error,
                ErrorKind::PluginFailed(source) if source.is::<Unavailable>()
            ),
            _ => false,
        }
    }
}

fn rejected_text(id: &str, error: &ErrorKind) -> String {
    match config_error(error) {
        Some(config) => config.to_string(),
        None => format!("entry {id:?} rejected: {error}"),
    }
}

/// The config error a row failed with: its config does not match its
/// plugin's schema.
fn config_error(error: &ErrorKind) -> Option<&rutis::ConfigError> {
    rutis::ConfigError::of(error)
}

impl LoaderError {
    /// The row's config does not match its plugin's schema: the problems.
    pub fn config(&self) -> Option<&rutis::ConfigError> {
        match self {
            LoaderError::Rejected { error, .. } => config_error(error),
            _ => None,
        }
    }
}

/// What a [`crate::Persist`] implementation reports.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PersistError {
    /// The stored version is not the expected one.
    #[error("version conflict")]
    Conflict,
    #[error("{0}")]
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A runtime's own `cannot load …` is not said twice (#677).
    #[test]
    fn a_load_error_is_not_repeated() {
        let resolve = |message: &str| LoaderError::Resolve {
            name: "file:///p/broken.mjs".into(),
            message: message.into(),
        };
        assert_eq!(
            resolve("cannot load broken.mjs: Error: x (broken.mjs:1:7)").to_string(),
            "cannot load broken.mjs: Error: x (broken.mjs:1:7)"
        );
        assert_eq!(
            resolve("Error: x").to_string(),
            "cannot load \"file:///p/broken.mjs\": Error: x"
        );
    }

    /// A change that failed and could not be undone says so in plain
    /// words, naming the rows (#680).
    #[test]
    fn a_failed_rollback_names_the_rows() {
        let failure = |id: &str, error: &str| Failure {
            id: id.into(),
            error: error.into(),
        };
        let error = LoaderError::RollbackFailed {
            apply: vec![failure("weather", "SyntaxError: x")],
            rollback: vec![failure("weather", "Error: boom")],
        };
        assert_eq!(
            error.to_string(),
            "the change failed (weather: SyntaxError: x), and the rows as they were did not start again either (weather: Error: boom)"
        );
    }
}
