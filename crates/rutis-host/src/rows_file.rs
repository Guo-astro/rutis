//! `run`'s `rutis.json` as the loader's editable layer: rows plugins add
//! or remove through the service `loader` are written back into its
//! `rows`, its other fields and the order of its keys as they were
//! (`docs/design-host-writes-plugins-2026-10-27.md` §五).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::host::BoxFuture;
use rutis_loader::edit::{apply_edit, Edit, Editable, Persist, PersistError, Version};
use rutis_loader::patch::apply_patches;
use rutis_loader::{Layer, LoaderError, Patch};
use serde_json::Value;

use crate::config::{read_text, HostConfig};
use crate::install::Target;

/// The name of the layer the host runs its rows as.
pub const LAYER: &str = "rutis.json";

/// The file's content the host runs (or refused), as last read or
/// written: what `--watch-config` does not take as a change.
pub type Seen = Arc<Mutex<Option<String>>>;

/// The configuration file whose rows plugins change.
pub struct RowsFile {
    path: PathBuf,
    seen: Seen,
}

impl RowsFile {
    /// `path`, its content as last read or written kept in `seen`.
    pub fn new(path: &Path, seen: Seen) -> Arc<Self> {
        Arc::new(Self {
            path: path.to_owned(),
            seen,
        })
    }

    /// Read it: the configuration in it, its content now seen.
    pub fn read(&self) -> Result<HostConfig, String> {
        let text = read_text(&self.path)?;
        *self.seen.lock().unwrap() = Some(text.clone());
        HostConfig::parse(&text, &self.path)
    }

    /// The layer the host runs its rows as, editable, at the content seen.
    pub fn editable(&self) -> Editable {
        let seen = self.seen.lock().unwrap().clone().unwrap_or_default();
        Editable::new(LAYER, Version(seen))
    }

    /// Its name, as lines say it (`rutis.json`).
    fn name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// The edits made to its rows as they are written in it: the row each
    /// adds as given, not as the host runs it (a plugin file's path, a
    /// link's identity).
    fn write(&self, expected: &Version, edits: &[Edit]) -> Result<Version, PersistError> {
        let failed = |why: String| {
            PersistError::Failed(format!("{} could not be written: {why}", self.name()))
        };
        let text = read_text(&self.path).map_err(failed)?;
        if text != expected.0 {
            return Err(PersistError::Conflict);
        }
        let mut target = Target::parse(self.path.clone(), &text).map_err(failed)?;
        let rows = edited(target.rows(), edits).map_err(|error| failed(error.to_string()))?;
        target.set("rows", Value::Array(rows), &[]);
        let written = target.store().map_err(|error| failed(error.to_string()))?;
        *self.seen.lock().unwrap() = Some(written.clone());
        Ok(Version(written))
    }
}

/// `rows` with `edits` made to them.
pub fn edited(rows: Vec<Value>, edits: &[Edit]) -> Result<Vec<Value>, LoaderError> {
    let mut layer = rows_layer(rows);
    for edit in edits {
        layer.patches = apply_edit(std::slice::from_ref(&layer), 0, edit)?;
    }
    Ok(apply_patches(&[layer]).rows)
}

/// `rows` as the one layer the host runs.
pub fn rows_layer(rows: Vec<Value>) -> Layer {
    Layer::new(
        LAYER,
        vec![Patch {
            insert: Some(rows),
            ..Patch::default()
        }],
    )
}

impl Persist for RowsFile {
    /// The rows in the file now, as the host runs them.
    fn load<'a>(
        &'a self,
        _layer: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<Patch>, Version), LoaderError>> {
        Box::pin(async move {
            let text = read_text(&self.path).map_err(LoaderError::PersistFailed)?;
            let config =
                HostConfig::parse(&text, &self.path).map_err(LoaderError::PersistFailed)?;
            Ok((rows_layer(config.rows()).patches, Version(text)))
        })
    }

    fn save<'a>(
        &'a self,
        _layer: &'a str,
        expected: &'a Version,
        edits: &'a [Edit],
        _patches: &'a [Patch],
    ) -> BoxFuture<'a, Result<Version, PersistError>> {
        Box::pin(async move { self.write(expected, edits) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Rows as written come back the same, groups and all, and an edit
    /// changes only its row.
    #[test]
    fn rows_are_written_back_as_they_were() {
        let rows = vec![
            json!({"id": "a", "name": "./a.mjs", "config": {"x": 1}}),
            json!({"id": "g", "group": true, "config": [{"id": "b", "name": "py:b"}]}),
        ];
        assert_eq!(edited(rows.clone(), &[]).unwrap(), rows);
        let create = Edit::Create {
            entry: json!({"id": "c", "name": "py:c"}),
            parent: None,
            position: None,
        };
        let mut added = rows.clone();
        added.push(json!({"id": "c", "name": "py:c"}));
        assert_eq!(
            edited(rows.clone(), std::slice::from_ref(&create)).unwrap(),
            added
        );
        let remove = Edit::Remove { id: "c".into() };
        assert_eq!(edited(rows.clone(), &[create, remove]).unwrap(), rows);
    }
}
