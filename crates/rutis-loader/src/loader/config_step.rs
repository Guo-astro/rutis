//! The loader's config step (config schema design §2.1): the one place a
//! row's config is filled with defaults, validated against its plugin's
//! schema and its extra keys worked out. Every place that checks a config
//! calls [`Inner::config_step`]; none has a copy of its own.

use std::sync::Arc;

use rutis::host::ErrorKind;
use rutis::{ConfigError, ConfigIssue};
use serde_json::Value;

use crate::config::{self, Options};
use crate::resolver::Resolved;
use crate::LoaderError;

use super::Inner;

/// Why the config step runs.
pub(super) enum Purpose<'a> {
    /// `check`: every row with a schema is validated, whichever runtime it
    /// belongs to (not Cordis rows: Cordis checks those); extra keys are
    /// collected into `warnings`.
    Check { warnings: &'a mut Vec<ConfigIssue> },
}

/// What [`Inner::config_step`] could check of a row.
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
    /// The config step for a copy of row `row` of `resolved`, its config
    /// evaluated: fill a copy with defaults, validate it, work out the
    /// extra keys. On a config that does not match, the marked error
    /// ([`rutis::ConfigError`]; [`rejected`] passes it on as the row's
    /// error). Otherwise the filled copy, or `config` as it is where
    /// nothing is checked.
    pub(super) fn config_step(
        &self,
        resolved: &Resolved,
        config: &Value,
        purpose: Purpose<'_>,
    ) -> Result<Value, ConfigError> {
        let Purpose::Check { warnings } = purpose;
        let (Checkable::Yes, Some(schema)) = (checkable(resolved), &resolved.schema) else {
            return Ok(config.clone());
        };
        let checked = config::check(schema, Some(config), options(&resolved.meta));
        for warning in checked.warnings.iter() {
            if !warnings.contains(warning) {
                warnings.push(warning.clone());
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
