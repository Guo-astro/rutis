//! `rutis.json` files written for earlier releases, read by this one
//! (Q5.5.3, Q6.10.4).
//!
//! `tests/fixtures/persisted/<version>/rutis.json` is a file of release
//! `<version>`, using as many of its fields as that release had;
//! `rutis.expected.json` beside it is what that release read from it
//! ([`normalized`]: what the host runs, paths relative to the sample's
//! directory as `<dir>`). Every sample must still be read as it was then.
//! `tests/fixtures/persisted/rejected/<case>/rutis.json` is a file this
//! release no longer reads; `rutis.error` beside it is the error, which says
//! what to do.
//!
//! Saving a release's sample (docs/release.md): copy the previous
//! release's directory to `<version>/`, use the fields the release added,
//! and write its expectation with
//! `RUTIS_PERSISTED_WRITE=<version> cargo test -p rutis-host --test persisted`
//! (only that directory's file is written; review the diff).

mod common;

use std::path::{Component, Path, PathBuf};

use common::{normalize, repo};
use rutis_host::config::{GoStart, HostConfig, Restart};
use serde_json::{json, Map, Value};

/// The samples' directory, without `..` (which `file:` URLs would drop).
fn persisted() -> PathBuf {
    let mut dir = PathBuf::new();
    for part in std::path::absolute(repo().join("tests/fixtures/persisted"))
        .unwrap()
        .components()
    {
        match part {
            Component::ParentDir => {
                dir.pop();
            }
            Component::CurDir => {}
            part => dir.push(part),
        }
    }
    dir
}

/// `path` with `dir` as `<dir>` and `/` between components.
fn path(path: &Path, dir: &Path) -> Value {
    let (head, rest) = match path.strip_prefix(dir) {
        Ok(rest) => (Some("<dir>"), rest),
        Err(_) => (None, path),
    };
    let parts: Vec<String> = head
        .map(str::to_owned)
        .into_iter()
        .chain(rest.components().filter_map(|part| match part {
            Component::CurDir => None,
            part => Some(part.as_os_str().to_string_lossy().into_owned()),
        }))
        .collect();
    json!(parts.join("/"))
}

/// Every string in `value` with `dir` as `<dir>` (the `file:` URLs of
/// plugin files).
fn strings(value: &mut Value, dir: &Path) {
    match value {
        Value::String(text) => *text = normalize(text, dir),
        Value::Array(items) => items.iter_mut().for_each(|item| strings(item, dir)),
        Value::Object(fields) => fields.values_mut().for_each(|item| strings(item, dir)),
        _ => {}
    }
}

/// What the host runs from `config`, read from a file in `dir`: each field
/// as the host takes it, absent options left out, and the rows as the
/// loader gets them.
fn normalized(config: &HostConfig, dir: &Path) -> Value {
    let mut out = Map::new();
    let paths = |list: &[&Option<PathBuf>], names: &[&str], into: &mut Map<String, Value>| {
        for (value, name) in list.iter().zip(names) {
            if let Some(value) = value {
                into.insert((*name).into(), path(value, dir));
            }
        }
    };
    out.insert("format".into(), json!(config.format));
    out.insert("id".into(), json!(config.id));
    let mut runtimes = Map::new();
    // `restart` (0.12) only when it is not the default: files of earlier
    // releases read as they did.
    let restart = |restart: Restart, into: &mut Map<String, Value>| {
        if restart != Restart::No {
            into.insert("restart".into(), json!(restart.name()));
        }
    };
    if let Some(node) = &config.runtimes.node {
        let mut fields = Map::new();
        fields.insert("project".into(), path(&node.project, dir));
        paths(&[&node.runtime], &["runtime"], &mut fields);
        restart(node.restart, &mut fields);
        runtimes.insert("node".into(), fields.into());
    }
    if let Some(py) = &config.runtimes.py {
        let mut fields = Map::new();
        fields.insert("project".into(), path(&py.project, dir));
        paths(&[&py.python], &["python"], &mut fields);
        restart(py.restart, &mut fields);
        runtimes.insert("py".into(), fields.into());
    }
    if let Some(bun) = &config.runtimes.bun {
        let mut fields = Map::new();
        fields.insert("project".into(), path(&bun.project, dir));
        paths(
            &[&bun.runtime, &bun.program],
            &["runtime", "program"],
            &mut fields,
        );
        restart(bun.restart, &mut fields);
        runtimes.insert("bun".into(), fields.into());
    }
    if let Some(go) = &config.runtimes.go {
        let mut fields = Map::new();
        paths(&[&go.dir], &["dir"], &mut fields);
        let binaries: Vec<Value> = go.binaries.iter().map(|b| path(b, dir)).collect();
        fields.insert("binaries".into(), binaries.into());
        let start = match go.start {
            GoStart::OnDemand => "on-demand",
            GoStart::Eager => "eager",
        };
        fields.insert("start".into(), json!(start));
        if let Some(idle) = go.idle {
            fields.insert("idle".into(), json!(idle));
        }
        fields.insert("project".into(), path(&go.project, dir));
        runtimes.insert("go".into(), fields.into());
    }
    let remote: Vec<Value> = config
        .runtimes
        .remote
        .iter()
        .map(|remote| {
            let mut fields = Map::new();
            fields.insert("name".into(), json!(remote.name));
            fields.insert("language".into(), json!(remote.language));
            if let Some(listen) = &remote.listen {
                fields.insert("listen".into(), json!(listen));
            }
            if let Some(instances) = &remote.instances {
                fields.insert("instances".into(), json!(instances));
            }
            Value::Object(fields)
        })
        .collect();
    runtimes.insert("remote".into(), remote.into());
    out.insert("runtimes".into(), runtimes.into());
    let listen: Vec<Value> = config
        .listen
        .iter()
        .map(|listener| {
            let mut fields = Map::new();
            fields.insert("name".into(), json!(listener.name));
            fields.insert("address".into(), json!(listener.address.to_string()));
            paths(
                &[&listener.cert, &listener.key],
                &["cert", "key"],
                &mut fields,
            );
            Value::Object(fields)
        })
        .collect();
    out.insert("listen".into(), listen.into());
    let mut rows = Value::from(config.rows());
    strings(&mut rows, dir);
    out.insert("rows".into(), rows);
    Value::Object(out)
}

/// The release directories, in order.
fn versions() -> Vec<(String, PathBuf)> {
    let mut versions: Vec<(String, PathBuf)> = std::fs::read_dir(persisted())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|dir| dir.join("rutis.json").exists())
        .filter_map(|dir| {
            let name = dir.file_name()?.to_str()?.to_owned();
            (name != "rejected").then_some((name, dir))
        })
        .collect();
    let key = |name: &str| -> Vec<u64> { name.split('.').map(|n| n.parse().unwrap()).collect() };
    versions.sort_by_key(|(name, _)| key(name));
    versions
}

/// Each release's `rutis.json` is read as that release read it.
/// risk: J3
/// guarantee: host/rutis-json-format
#[test]
fn every_release_s_rutis_json_reads_as_it_did() {
    let versions = versions();
    let names: Vec<&str> = versions.iter().map(|(name, _)| name.as_str()).collect();
    for release in [
        "0.8.0", "0.9.0", "0.9.1", "0.10.0", "0.11.0", "0.12.0", "0.12.1",
    ] {
        assert!(
            names.contains(&release),
            "no sample of {release}: {names:?}"
        );
    }
    let write = std::env::var("RUTIS_PERSISTED_WRITE").ok();
    for (version, dir) in &versions {
        let config = HostConfig::read(&dir.join("rutis.json")).unwrap_or_else(|error| {
            panic!("{version}: {}", normalize(&error, dir));
        });
        assert!(
            config.warnings.is_empty(),
            "{version}: a field the release read is ignored now: {:?}",
            config.warnings
        );
        config
            .check_runtime_names()
            .and_then(|()| config.check_dialing_runtimes())
            .unwrap_or_else(|error| panic!("{version}: {error}"));
        let actual = normalized(&config, dir);
        let expected_file = dir.join("rutis.expected.json");
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
            "{version}: rutis.json no longer reads as it did; this is an incompatible change \
             (docs/quality-standard.md §9.3). Now:\n{}",
            serde_json::to_string_pretty(&actual).unwrap()
        );
    }
}

/// Each file of a format this release no longer reads is refused, saying
/// what to do.
/// risk: J3
/// guarantee: host/rutis-json-format
#[test]
fn formats_no_longer_read_are_refused() {
    let rejected = persisted().join("rejected");
    let mut cases = 0;
    for entry in std::fs::read_dir(&rejected).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.join("rutis.json").exists() {
            continue;
        }
        cases += 1;
        let error = HostConfig::read(&dir.join("rutis.json"))
            .map(|_| ())
            .expect_err(&format!("{}: refused", dir.display()));
        let expected = std::fs::read_to_string(dir.join("rutis.error")).unwrap();
        assert_eq!(
            normalize(&error, &dir),
            expected.trim_end(),
            "{}",
            dir.display()
        );
    }
    assert!(cases > 0, "no rejected sample in {}", rejected.display());
}
