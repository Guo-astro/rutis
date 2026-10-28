//! dsh user layers (`<profile>/cordis.patch.yml`) written for earlier
//! releases, read by this one (Q5.5.3, Q6.10.4).
//!
//! The user layer is Cordis's configuration format, which has no version:
//! rutis adds none (docs/design-quality-step2-2026-10-11.md §七), so its
//! compatibility rests on these samples alone.
//! `tests/fixtures/persisted/<version>/cordis.patch.yml` is a user layer of
//! release `<version>`, using as much of the format as that release had;
//! `cordis.patch.expected.json` beside it is the patches that release read
//! from it (as rutis-loader's `Patch` serializes; plugin files' `file:` URLs
//! with the sample's directory as `<dir>`). Every sample must still be read
//! as it was then.
//!
//! Saving a release's sample (docs/release.md): copy the previous
//! release's directory to `<version>/`, and write its expectation with
//! `RUTIS_PERSISTED_WRITE=<version> cargo test -p rutis-dsh --test persisted`
//! (only that directory's file is written; review the diff).

use std::path::{Path, PathBuf};

use rutis_dsh::profile::layers::parse_patch_list;
use rutis_dsh::profile::{ProfileContext, UserLayerStore};
use rutis_loader::edit::Persist;
use serde_json::Value;

/// The samples' directory, without `..`.
fn persisted() -> PathBuf {
    let mut dir = PathBuf::new();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/persisted");
    for part in std::path::absolute(path).unwrap().components() {
        match part {
            std::path::Component::ParentDir => {
                dir.pop();
            }
            std::path::Component::CurDir => {}
            part => dir.push(part),
        }
    }
    dir
}

/// Every string in `value` with each (URL, replacement) of `forms` replaced.
fn strings(value: &mut Value, forms: &[(String, &str)]) {
    match value {
        Value::String(text) => {
            for (url, replacement) in forms {
                *text = text.replace(url.as_str(), replacement);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| strings(item, forms)),
        Value::Object(fields) => fields.values_mut().for_each(|item| strings(item, forms)),
        _ => {}
    }
}

/// The user layer of the profile `dir` as dsh's loader storage reads it.
fn read(dir: &Path) -> Value {
    let context = ProfileContext {
        name: "sample".into(),
        dir: dir.to_owned(),
        install_anchor: dir.join("package.json"),
        home: dir.to_owned(),
        overlays: Vec::new(),
        telemetry_disabled: None,
        runtime_version: None,
    };
    let store = UserLayerStore::new(&context);
    let (patches, _) = futures::executor::block_on(store.load("user"))
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    // How this platform writes `<dir>` and its parent as `file:` URLs: as
    // rows named `./x` and `../x` in a layer there are anchored. `<dir>`
    // first: the parent's URL begins it.
    let url = |name: &str| -> String {
        let probe = parse_patch_list(
            &context.user_layer_path(),
            &format!("- insert:\n    - name: {name}\n"),
        )
        .unwrap();
        let url = probe[0].insert.as_ref().unwrap()[0]["name"]
            .as_str()
            .unwrap()
            .to_owned();
        url.strip_suffix("/x").unwrap().to_owned()
    };
    let forms = [
        (url("./x"), "file://<dir>"),
        (url("../x"), "file://<dir>/.."),
    ];
    let mut value = serde_json::to_value(patches).unwrap();
    strings(&mut value, &forms);
    value
}

/// Each release's user layer is read as that release read it.
/// risk: J3
/// guarantee: dsh/user-layer-format
#[test]
fn every_release_s_user_layer_reads_as_it_did() {
    let mut versions: Vec<(String, PathBuf)> = std::fs::read_dir(persisted())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|dir| dir.join("cordis.patch.yml").exists())
        .filter_map(|dir| Some((dir.file_name()?.to_str()?.to_owned(), dir)))
        .filter(|(name, _)| name != "rejected")
        .collect();
    versions.sort();
    let names: Vec<&str> = versions.iter().map(|(name, _)| name.as_str()).collect();
    for release in [
        "0.8.0", "0.9.0", "0.9.1", "0.10.0", "0.11.0", "0.12.0", "0.12.1", "0.13.0", "0.15.0",
        "0.15.1", "0.16.0", "0.17.0", "0.17.1", "0.17.2", "0.18.0",
    ] {
        assert!(
            names.contains(&release),
            "no sample of {release}: {names:?}"
        );
    }
    let write = std::env::var("RUTIS_PERSISTED_WRITE").ok();
    for (version, dir) in &versions {
        let actual = read(dir);
        let expected_file = dir.join("cordis.patch.expected.json");
        if write.as_deref() == Some(version.as_str()) {
            let text = serde_json::to_string_pretty(&actual).unwrap() + "\n";
            std::fs::write(&expected_file, text).unwrap();
            continue;
        }
        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(&expected_file)
                .unwrap_or_else(|error| panic!("{}: {error}", expected_file.display())),
        )
        .unwrap();
        assert_eq!(
            actual,
            expected,
            "{version}: the user layer no longer reads as it did; this is an incompatible \
             change (docs/quality-standard.md §9.3). Now:\n{}",
            serde_json::to_string_pretty(&actual).unwrap()
        );
    }
}
