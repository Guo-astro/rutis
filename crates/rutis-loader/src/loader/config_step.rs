//! The loader's config step (config schema design §2.1): the one place a
//! row's config goes through the kernel's [`rutis::config::check`] (filled
//! with defaults, validated against its plugin's schema, its extra keys
//! worked out). Every place that checks a config (the check before a copy
//! loads, `commit`'s dry run, `EntryFactory`'s `validate_config` / `build`,
//! and `check`) calls
//! [`Inner::config_step`]; none has a copy of its own.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rutis::host::ErrorKind;
use rutis::{ConfigError, ConfigIssue};
use serde_json::Value;

use crate::resolver::Resolved;
use crate::LoaderError;

use super::Inner;

/// Where the loader writes a row's extra config keys: the row's id and the
/// problem. By default one line on standard error, `<row>: config: <problem>`.
pub type ConfigWarnings = Arc<dyn Fn(&str, &ConfigIssue) + Send + Sync>;

/// The default [`ConfigWarnings`].
pub(super) fn standard_error() -> ConfigWarnings {
    Arc::new(|row: &str, issue: &ConfigIssue| eprintln!("{row}: config: {}", issue.message))
}

/// What the loader warned about: each row (and instance, for a runtime
/// that dials in), config value and warning once, shared by dry runs and
/// loads, so restarts do not repeat a warning (a schema that changes what
/// is extra warns again).
#[derive(Default)]
pub(super) struct Warned {
    seen: Mutex<HashSet<WarnedKey>>,
}

/// A row, its instance, its config value and the warnings given.
type WarnedKey = (String, Option<String>, String, Vec<String>);

impl Warned {
    /// Whether this row, instance, value and warnings were not warned about
    /// yet (and now are).
    fn first(
        &self,
        row: &str,
        instance: Option<&str>,
        config: &Value,
        warnings: &[ConfigIssue],
    ) -> bool {
        self.seen.lock().unwrap().insert((
            row.to_owned(),
            instance.map(str::to_owned),
            config.to_string(),
            warnings.iter().map(|w| w.message.clone()).collect(),
        ))
    }
}

/// Why the config step runs.
pub(super) enum Purpose<'a> {
    /// `check`: extra keys are collected into `warnings`.
    Check { warnings: &'a mut Vec<ConfigIssue> },
    /// A copy of row `row` loads, is updated or is dry-run: extra keys are
    /// warned about once per row, `instance` and value.
    Run {
        row: &'a str,
        instance: Option<&'a str>,
    },
}

/// What can be checked of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Checkable {
    /// Its plugin has a schema.
    Yes,
    /// No schema: nothing to check.
    NoSchema,
    /// Its schema is known only when it runs (a remote runtime not
    /// connected, a peer that is not there).
    NotYet,
    /// A Cordis row: Cordis validates it with its own `Config`, not by the
    /// schema converted from it (config schema design §2.3).
    Cordis,
}

/// What the config step needs of a row's plugin.
pub(crate) struct Subject<'a> {
    schema: Option<&'a Value>,
    checkable: Checkable,
}

impl<'a> Subject<'a> {
    /// The rows of `resolved`.
    pub(crate) fn of(resolved: &'a Resolved) -> Self {
        Subject {
            schema: resolved.schema.as_ref(),
            checkable: checkable(resolved),
        }
    }
}

/// What can be checked of `resolved`'s rows.
pub(super) fn checkable(resolved: &Resolved) -> Checkable {
    if resolved.meta.get("cordis") == Some(&Value::Bool(true)) {
        return Checkable::Cordis;
    }
    match (&resolved.schema, resolved.meta.get("schema")) {
        (Some(_), _) => Checkable::Yes,
        (None, Some(Value::String(_))) => Checkable::NotYet,
        (None, _) => Checkable::NoSchema,
    }
}

impl Inner {
    /// The config step for a copy of a row of `subject`, its config
    /// evaluated (the raw config as the row has it, never a filled copy):
    /// fill a copy with defaults, validate it, work out the extra keys. On
    /// a config that does not match, the marked error
    /// ([`rutis::ConfigError`]; [`rejected`] passes it on as the row's
    /// error). Otherwise the filled copy: what the plugin gets, whichever
    /// runtime it runs in. A row with nothing to check (no schema, a Cordis
    /// row) gets `config` as it is.
    pub(super) fn config_step(
        &self,
        subject: &Subject<'_>,
        config: &Value,
        purpose: Purpose<'_>,
    ) -> Result<Value, ConfigError> {
        let (Checkable::Yes, Some(schema)) = (subject.checkable, subject.schema) else {
            return Ok(config.clone());
        };
        let checked = rutis::config::check(schema, Some(config));
        match purpose {
            Purpose::Check { warnings } => {
                for warning in &checked.warnings {
                    if !warnings.contains(warning) {
                        warnings.push(warning.clone());
                    }
                }
            }
            Purpose::Run { row, instance } => {
                if !checked.warnings.is_empty()
                    && self.warned.first(row, instance, config, &checked.warnings)
                {
                    let sink = self.config_warnings.lock().unwrap().clone();
                    for warning in &checked.warnings {
                        sink(row, warning);
                    }
                }
            }
        }
        match checked.error() {
            Some(error) => Err(error),
            None => Ok(checked.config),
        }
    }
}

/// Row `row` failing with the config error `error`, passed as its
/// plugin's error.
pub(super) fn rejected(row: &str, error: ConfigError) -> LoaderError {
    LoaderError::Rejected {
        id: row.to_owned(),
        error: Arc::new(ErrorKind::PluginFailed(Box::new(error))),
    }
}
