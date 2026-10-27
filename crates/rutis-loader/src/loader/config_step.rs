//! The loader's config step (config schema design §2.1): the one place a
//! row's config is filled with defaults, validated against its plugin's
//! schema and its extra keys worked out. Every place that checks a config
//! (the check before a copy loads, `commit`'s dry run, `EntryFactory`'s
//! `validate_config` / `build`, the factories of runtimes that dial in, and
//! `check`) calls [`Inner::config_step`]; none has a copy of its own.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rutis::host::ErrorKind;
use rutis::{ConfigError, ConfigIssue};
use serde_json::Value;

use crate::config::{self, Options};
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
    /// `check`: every row with a schema is validated, whichever runtime it
    /// belongs to (not Cordis rows: Cordis checks those); extra keys are
    /// collected into `warnings`.
    Check { warnings: &'a mut Vec<ConfigIssue> },
    /// A copy of row `row` loads, is updated or is dry-run: only rows the
    /// loader fills are validated (other runtimes' SDKs do theirs); extra
    /// keys of every row with a schema are warned about once per row,
    /// `instance` and value.
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
    /// A Cordis row: Cordis validates it with its own `Config`.
    Cordis,
}

/// What the config step needs of a row's plugin.
pub(crate) struct Subject<'a> {
    schema: Option<&'a Value>,
    checkable: Checkable,
    /// The loader fills and validates it (a Rust row: `Builtins`, a dylib,
    /// an embedder's resolver); other runtimes' rows their SDKs do.
    fills: bool,
    options: Options,
}

impl<'a> Subject<'a> {
    /// The rows of `resolved`.
    pub(crate) fn of(resolved: &'a Resolved) -> Self {
        Subject {
            schema: resolved.schema.as_ref(),
            checkable: checkable(resolved),
            fills: !resolved.foreign_scope,
            options: options(&resolved.meta),
        }
    }

    /// A row of a runtime that dialed in, its plugin described on its
    /// connection with `schema`.
    #[cfg_attr(not(feature = "runtimes"), allow(dead_code))]
    pub(crate) fn connection(schema: Option<&'a Value>, cordis: bool) -> Self {
        Subject {
            schema,
            checkable: match (cordis, schema) {
                (true, _) => Checkable::Cordis,
                (false, Some(_)) => Checkable::Yes,
                (false, None) => Checkable::NoSchema,
            },
            fills: false,
            options: Options::default(),
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

/// How `resolved`'s rows are checked: a Go binary built with an SDK older
/// than 0.18 lists every field without `omitempty` as required in its
/// manifest, so `required` is not checked there (design §7).
fn options(meta: &Value) -> Options {
    let old_go = meta.get("source").and_then(Value::as_str) == Some("go")
        && meta
            .get("sdk")
            .and_then(Value::as_str)
            .is_some_and(older_than_0_18);
    match old_go {
        true => Options::default().without_required(),
        false => Options::default(),
    }
}

/// `version` (`major.minor[.patch][-pre]`) is older than 0.18.
fn older_than_0_18(version: &str) -> bool {
    let mut parts = version.split(['.', '-', '+']);
    let major: Option<u64> = parts.next().and_then(|p| p.parse().ok());
    let minor: Option<u64> = parts.next().and_then(|p| p.parse().ok());
    match (major, minor) {
        (Some(0), Some(minor)) => minor < 18,
        _ => false,
    }
}

impl Inner {
    /// The config step for a copy of a row of `subject`, its config
    /// evaluated (the raw config as the row has it, never a filled copy):
    /// fill a copy with defaults, validate it, work out the extra keys. On
    /// a config that does not match, the marked error
    /// ([`rutis::ConfigError`]; [`rejected`] passes it on as the row's
    /// error). Otherwise the filled copy for the rows the loader fills, and
    /// `config` as it is for the others.
    pub(super) fn config_step(
        &self,
        subject: &Subject<'_>,
        config: &Value,
        purpose: Purpose<'_>,
    ) -> Result<Value, ConfigError> {
        let (Checkable::Yes, Some(schema)) = (subject.checkable, subject.schema) else {
            return Ok(config.clone());
        };
        let checked = config::check(schema, Some(config), subject.options);
        let validate = match purpose {
            Purpose::Check { warnings } => {
                for warning in &checked.warnings {
                    if !warnings.contains(warning) {
                        warnings.push(warning.clone());
                    }
                }
                true
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
                subject.fills
            }
        };
        match (validate, checked.error()) {
            (true, Some(error)) => Err(error),
            (true, None) if subject.fills => Ok(checked.config),
            _ => Ok(config.clone()),
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

#[cfg(test)]
mod tests {
    use super::{older_than_0_18, options};
    use crate::config::Options;
    use serde_json::json;

    #[test]
    fn old_go_manifests_do_not_check_required() {
        let go = |sdk: &str| json!({ "source": "go", "sdk": sdk });
        assert_eq!(
            options(&go("0.17.2")),
            Options::default().without_required()
        );
        assert_eq!(options(&go("0.18.0")), Options::default());
        let swift = json!({ "source": "swift", "sdk": "0.17.2" });
        assert_eq!(options(&swift), Options::default());
    }

    #[test]
    fn versions_older_than_0_18() {
        for (version, older) in [
            ("0.17.2", true),
            ("0.13.0", true),
            ("0.18.0", false),
            ("0.18.0-rc.1", false),
            ("0.19", false),
            ("1.0.0", false),
            ("dev", false),
        ] {
            assert_eq!(older_than_0_18(version), older, "{version}");
        }
    }
}
