//! `rutis.json` reading as properties (#631): the file's format checked
//! before any other field, relative paths rebased onto the file's
//! directory the same way twice and from any working directory, unknown
//! fields refused where they are — line and column — while a row's only
//! warn, and any text read without a panic.
//!
//! The fixed-shape cases stay in `tests/config.rs`; these generate the
//! shapes (which fields are there, which paths, which unknown names).

use std::path::{Path, PathBuf};

use proptest::prelude::*;
use rutis_host::config::{file_url, HostConfig, FORMAT};
use serde::Serialize;
use serde_json::{Map, Value};

/// The file the properties read, wherever it would be.
const FILE: &str = "/srv/app/rutis.json";

fn file() -> &'static Path {
    Path::new(FILE)
}

fn parse(text: &str) -> Result<HostConfig, String> {
    HostConfig::parse(text, file())
}

// ---------------------------------------------------------------- values

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-3i64..10).prop_map(Value::from),
        "[a-z_]{0,6}".prop_map(Value::String),
    ]
}

/// Arbitrary JSON, nested a little: what a field may hold.
fn json() -> impl Strategy<Value = Value> {
    scalar().prop_recursive(4, 64, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Value::Array),
            prop::collection::vec(("[a-z_]{1,6}", inner), 0..3).prop_map(|entries| {
                let mut object = Map::new();
                for (key, value) in entries {
                    object.insert(key, value);
                }
                Value::Object(object)
            }),
        ]
    })
}

/// Random top-level fields with arbitrary values, in random order.
fn object() -> impl Strategy<Value = Vec<(String, Value)>> {
    prop::collection::vec(("[a-z_]{1,8}", json()), 0..6)
}

// ------------------------------------------------------------- format

proptest! {
    /// guarantee: host/rutis-json-format
    ///
    /// Whatever else the file has — fields this host knows with the wrong
    /// types, fields it does not know, `rows` that are no rows — a newer
    /// format is refused first, saying what to upgrade; the rest is never
    /// judged.
    #[test]
    fn a_newer_format_is_refused_before_any_other_field(
        mut fields in object(),
        newer in 2u64..=1000,
        position in 0usize..8,
    ) {
        fields.retain(|(key, _)| key != "format");
        let position = position.min(fields.len());
        fields.insert(position, ("format".to_owned(), Value::from(newer)));
        let text = Value::Object(fields.into_iter().collect()).to_string();
        prop_assert_eq!(
            parse(&text).unwrap_err(),
            format!(
                "{}: this file is format {newer}, this rutis-host reads up to {FORMAT}: upgrade rutis-host",
                file().display()
            )
        );
    }

    /// guarantee: host/rutis-json-format
    ///
    /// Format 0 is no format, and is refused before the other fields too.
    #[test]
    fn format_zero_is_refused_before_any_other_field(
        mut fields in object(),
        position in 0usize..8,
    ) {
        fields.retain(|(key, _)| key != "format");
        let position = position.min(fields.len());
        fields.insert(position, ("format".to_owned(), Value::from(0)));
        let text = Value::Object(fields.into_iter().collect()).to_string();
        prop_assert_eq!(
            parse(&text).unwrap_err(),
            format!(
                "{}: format 0 is not a rutis.json format: formats start at 1, and this rutis-host reads up to {FORMAT}",
                file().display()
            )
        );
    }
}

// -------------------------------------------------------------- paths

fn path_text() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(".".to_owned()),
        "[a-z][a-z0-9_]{0,5}".prop_map(|name| name),
        "[a-z][a-z0-9_]{0,5}".prop_map(|name| format!("{name}/file")),
        "[a-z][a-z0-9_]{0,5}".prop_map(|name| format!("../{name}")),
        "[a-z][a-z0-9_]{0,5}".prop_map(|name| format!("/srv/{name}")),
    ]
}

fn row_name() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z][a-z0-9_]{0,5}".prop_map(|name| format!("./{name}.mjs")),
        "[a-z][a-z0-9_]{0,5}".prop_map(|name| format!("../{name}.mjs")),
        "[a-z][a-z0-9_]{0,5}".prop_map(|name| format!("{name}-plugin")),
        // A runtime's module resolves in its project: no `file:` URL.
        "[a-z][a-z0-9_]{0,5}".prop_map(|name| format!("bun:./{name}.ts")),
    ]
}

fn base() -> impl Strategy<Value = PathBuf> {
    "[a-z][a-z0-9_]{0,5}".prop_map(|name| PathBuf::from(format!("/srv/{name}/app")))
}

/// A configuration with every field that carries a path.
fn config(paths: &[String], names: &[String]) -> HostConfig {
    let at = |index: usize| paths[index % paths.len()].clone();
    let rows: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(index, name)| serde_json::json!({ "id": format!("r{index}"), "name": name }))
        .collect();
    serde_json::from_value(serde_json::json!({
        "runtimes": {
            "node": { "project": at(0), "runtime": at(1) },
            "py": { "project": at(2), "python": at(3) },
            "bun": { "project": at(4), "runtime": at(5), "program": at(6) },
            "go": { "dir": at(7), "binaries": [at(8)], "project": at(9) },
        },
        "listen": [{
            "name": "public",
            "address": "127.0.0.1:0",
            "cert": at(10),
            "key": at(11),
        }],
        "rows": rows,
    }))
    .unwrap()
}

/// Every path of `config`, and its row names, in order.
#[derive(Debug, PartialEq, Serialize)]
struct Paths {
    node: Option<(PathBuf, Option<PathBuf>)>,
    py: Option<(PathBuf, Option<PathBuf>)>,
    bun: Option<(PathBuf, Option<PathBuf>, Option<PathBuf>)>,
    go: Option<(Option<PathBuf>, Vec<PathBuf>, PathBuf)>,
    listen: Vec<(Option<PathBuf>, Option<PathBuf>)>,
    names: Vec<String>,
}

impl Paths {
    /// Whether every setting that names a place is absolute. A bare
    /// program name (`python3`, `bun`) is looked up on `PATH` and stays as
    /// it is; so does a row's plain plugin name.
    fn places_are_absolute(&self) -> bool {
        let absolute = |path: &PathBuf| path.is_absolute();
        let optional = |path: &Option<PathBuf>| path.as_ref().is_none_or(absolute);
        let node = self
            .node
            .as_ref()
            .is_none_or(|(project, runtime)| absolute(project) && optional(runtime));
        let py = self
            .py
            .as_ref()
            .is_none_or(|(project, _)| absolute(project));
        let bun = self
            .bun
            .as_ref()
            .is_none_or(|(project, runtime, _)| absolute(project) && optional(runtime));
        let go = self.go.as_ref().is_none_or(|(dir, binaries, project)| {
            optional(dir) && binaries.iter().all(absolute) && absolute(project)
        });
        let listen = self
            .listen
            .iter()
            .all(|(cert, key)| optional(cert) && optional(key));
        node && py && bun && go && listen
    }
}

fn paths_of(config: &HostConfig) -> Paths {
    let runtimes = &config.runtimes;
    Paths {
        node: runtimes
            .node
            .as_ref()
            .map(|node| (node.project.clone(), node.runtime.clone())),
        py: runtimes
            .py
            .as_ref()
            .map(|python| (python.project.clone(), python.python.clone())),
        bun: runtimes.bun.as_ref().map(|bun| {
            (
                bun.project.clone(),
                bun.runtime.clone(),
                bun.program.clone(),
            )
        }),
        go: runtimes
            .go
            .as_ref()
            .map(|go| (go.dir.clone(), go.binaries.clone(), go.project.clone())),
        listen: config
            .listen
            .iter()
            .map(|listener| (listener.cert.clone(), listener.key.clone()))
            .collect(),
        names: config
            .rows
            .iter()
            .map(|row| row["name"].as_str().unwrap_or_default().to_owned())
            .collect(),
    }
}

/// What [`HostConfig::rebase`] must make of `path`: an absolute one stays,
/// a relative one joins the (already absolute) base without its `.`
/// components.
fn rebased(path: &Path, base: &Path) -> PathBuf {
    match path.is_relative() {
        true => base.join(path).components().collect(),
        false => path.to_path_buf(),
    }
}

fn rebased_opt(path: &Option<PathBuf>, base: &Path) -> Option<PathBuf> {
    path.as_deref().map(|path| rebased(path, base))
}

/// What `rebase_program` makes of a program: a bare name is looked up on
/// `PATH` and stays, one with a directory joins the base.
fn rebased_program(path: &Option<PathBuf>, base: &Path) -> Option<PathBuf> {
    path.as_deref()
        .map(|path| match path.components().count() > 1 {
            true => rebased(path, base),
            false => path.to_path_buf(),
        })
}

fn rebased_paths(config: &HostConfig, base: &Path) -> Paths {
    let paths = paths_of(config);
    Paths {
        node: paths
            .node
            .map(|(project, runtime)| (rebased(&project, base), rebased_opt(&runtime, base))),
        py: paths
            .py
            .map(|(project, python)| (rebased(&project, base), rebased_program(&python, base))),
        bun: paths.bun.map(|(project, runtime, program)| {
            (
                rebased(&project, base),
                rebased_opt(&runtime, base),
                rebased_program(&program, base),
            )
        }),
        go: paths.go.map(|(dir, binaries, project)| {
            (
                rebased_opt(&dir, base),
                binaries.iter().map(|path| rebased(path, base)).collect(),
                rebased(&project, base),
            )
        }),
        listen: paths
            .listen
            .into_iter()
            .map(|(cert, key)| (rebased_opt(&cert, base), rebased_opt(&key, base)))
            .collect(),
        names: paths
            .names
            .into_iter()
            .map(|name| {
                // What `rebase` rewrites: a plugin file named relative to
                // the configuration. `bun:` modules are not files of it.
                let relative = ["./", "../"]
                    .iter()
                    .chain(if cfg!(windows) {
                        &[".\\", "..\\"][..]
                    } else {
                        &[]
                    })
                    .any(|prefix| name.starts_with(prefix));
                match relative {
                    true => file_url(&base.join(&name)),
                    false => name,
                }
            })
            .collect(),
    }
}

proptest! {
    /// guarantee: host/config-paths-and-defaults
    ///
    /// For any mix of relative and absolute settings: every relative path
    /// joins the file's directory once and is absolute afterwards; every
    /// absolute one stays as it is. Rebasing the result again changes
    /// nothing, and rebasing it onto another directory changes nothing
    /// either — a second pass cannot reach the working directory.
    #[test]
    fn rebasing_is_idempotent_and_keeps_absolute_paths(
        paths in prop::collection::vec(path_text(), 8..16),
        names in prop::collection::vec(row_name(), 1..5),
        base in base(),
    ) {
        let original = config(&paths, &names);
        let base = std::path::absolute(&base).unwrap();
        let mut once = original.clone();
        once.rebase(&base);
        prop_assert_eq!(paths_of(&once), rebased_paths(&original, &base));
        prop_assert!(
            paths_of(&once).places_are_absolute(),
            "a relative place survived the first rewrite"
        );
        let mut twice = once.clone();
        twice.rebase(&base);
        prop_assert_eq!(paths_of(&twice), paths_of(&once));
        let other = std::path::absolute(Path::new("/srv/other/app")).unwrap();
        let mut elsewhere = once.clone();
        elsewhere.rebase(&other);
        prop_assert_eq!(paths_of(&elsewhere), paths_of(&once));
    }
}

/// The same file, named by its absolute path, read from two working
/// directories, gives the same paths: where the host was started cannot
/// reach them (#226).
/// guarantee: host/config-paths-and-defaults
#[test]
fn reading_from_any_working_directory_gives_the_same_paths() {
    // The child, run by the test below in another working directory.
    if let Ok(out) = std::env::var("RUTIS_631_PATHS") {
        let file = std::env::var("RUTIS_631_CONFIG").unwrap();
        let config = HostConfig::read(Path::new(&file)).unwrap();
        std::fs::write(out, serde_json::to_string(&paths_of(&config)).unwrap()).unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("rutis.json"),
        r#"{ "runtimes": { "node": {} }, "rows": [
            { "id": "a", "name": "./a.mjs" },
            { "id": "b", "name": "../shared/b.mjs" }
        ] }"#,
    )
    .unwrap();
    let file = app.join("rutis.json");
    let mut snapshots = Vec::new();
    for (index, cwd) in [dir.path().join("one"), dir.path().join("two")]
        .iter()
        .enumerate()
    {
        std::fs::create_dir_all(cwd).unwrap();
        let out = dir.path().join(format!("paths-{index}.json"));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "reading_from_any_working_directory_gives_the_same_paths",
                "--exact",
                "--nocapture",
            ])
            .current_dir(cwd)
            .env("RUTIS_631_PATHS", &out)
            .env("RUTIS_631_CONFIG", &file)
            .status()
            .unwrap();
        assert!(status.success(), "the child failed");
        snapshots.push(std::fs::read_to_string(out).unwrap());
    }
    assert_eq!(snapshots[0], snapshots[1]);
    assert!(
        snapshots[0].contains("file:"),
        "no plugin file was rewritten: {}",
        snapshots[0]
    );
}

// ---------------------------------------------------- unknown fields

/// The fields this host knows at the top, with values of their types.
fn known_fields() -> Vec<(&'static str, Value)> {
    vec![
        ("id", Value::String("main".to_owned())),
        ("runtimes", Value::Object(Map::new())),
        ("listen", Value::Array(Vec::new())),
        ("installed", Value::Object(Map::new())),
        ("rows", Value::Array(Vec::new())),
    ]
}

proptest! {
    /// guarantee: host/unknown-fields
    ///
    /// A field this host does not know is an error that names it and where
    /// it is — its line and the column of its key — however many known
    /// fields are beside it and in whatever order, and whatever it holds.
    #[test]
    fn an_unknown_top_level_field_is_refused_where_it_is(
        take in prop::collection::vec(any::<bool>(), 5..=5),
        order in prop::collection::vec(any::<u32>(), 5..=5),
        unknown in "[a-z][a-z0-9_]{0,8}".prop_map(|name| format!("unknown_{name}")),
        value in json(),
        position in 0usize..6,
    ) {
        let known = known_fields();
        let mut fields: Vec<(String, Value)> = known
            .into_iter()
            .zip(&take)
            .filter_map(|((key, value), take)| {
                take.then(|| (key.to_owned(), value))
            })
            .collect();
        let keys = order.iter().take(fields.len()).copied().collect::<Vec<_>>();
        let mut keyed: Vec<(u32, (String, Value))> = keys.into_iter().zip(fields).collect();
        keyed.sort_by_key(|(key, _)| *key);
        fields = keyed.into_iter().map(|(_, field)| field).collect();
        let position = position.min(fields.len());
        fields.insert(position, (unknown.clone(), value));
        let text = serde_json::to_string_pretty(&Value::Object(fields.into_iter().collect())).unwrap();
        // serde reports the key's closing quote: two spaces of indentation,
        // the opening quote, the name, then it.
        let needle = format!("\n  \"{unknown}\":");
        let at = text.find(&needle).expect("the unknown field in the text");
        let line = text[..at].matches('\n').count() + 2;
        let column = unknown.len() + 4;
        prop_assert_eq!(
            parse(&text).unwrap_err(),
            format!(
                "{}: unknown field `{unknown}`, expected one of `format`, `id`, `runtimes`, `listen`, `installed`, `rows` at line {line} column {column}",
                file().display()
            )
        );
    }
}

/// A row and the unknown fields in it.
#[derive(Debug)]
struct RowSpec {
    unknown: Vec<(String, Value)>,
    /// A `group` and the rows in its `config`: whether the flag was truthy
    /// (the host reports their unknown fields too) and the rows.
    children: Option<(bool, Vec<RowSpec>)>,
}

fn row_spec(depth: u32) -> impl Strategy<Value = RowSpec> {
    let unknown = prop::collection::vec(
        (
            "[a-z][a-z0-9_]{0,6}".prop_map(|name| format!("z_{name}")),
            json(),
        ),
        0..4,
    );
    if depth == 0 {
        unknown
            .prop_map(|unknown| RowSpec {
                unknown,
                children: None,
            })
            .boxed()
    } else {
        (
            unknown,
            prop::option::of((
                any::<bool>(),
                prop::collection::vec(row_spec(depth - 1), 0..3),
            )),
        )
            .prop_map(|(unknown, children)| RowSpec { unknown, children })
            .boxed()
    }
}

/// `spec` as a row, counting every unknown field the host must warn about
/// and naming the row it is in.
fn build_row(
    spec: &RowSpec,
    at: usize,
    count: &mut usize,
    expected: &mut Vec<(String, String)>,
) -> Value {
    let id = format!("r{at}");
    let mut row = Map::new();
    row.insert("id".to_owned(), Value::String(id.clone()));
    if at.is_multiple_of(2) {
        row.insert(
            "name".to_owned(),
            Value::String("weather-plugin".to_owned()),
        );
    }
    for (field, value) in &spec.unknown {
        if row.contains_key(field) {
            continue;
        }
        row.insert(field.clone(), value.clone());
        *count += 1;
        expected.push((field.clone(), id.clone()));
    }
    match &spec.children {
        Some((true, children)) => {
            row.insert("group".to_owned(), Value::Bool(true));
            let children: Vec<Value> = children
                .iter()
                .enumerate()
                .map(|(index, child)| build_row(child, index, count, expected))
                .collect();
            row.insert("config".to_owned(), Value::Array(children));
        }
        Some((false, children)) => {
            // Not a group: the loader never reads these rows, so their
            // fields are not the host's to warn about.
            row.insert("group".to_owned(), Value::Bool(false));
            let children: Vec<Value> = children
                .iter()
                .map(|child| {
                    let mut hidden = Map::new();
                    for (field, value) in &child.unknown {
                        hidden.insert(field.clone(), value.clone());
                    }
                    Value::Object(hidden)
                })
                .collect();
            row.insert("config".to_owned(), Value::Array(children));
        }
        None => {
            row.insert("config".to_owned(), Value::Object(Map::new()));
        }
    }
    Value::Object(row)
}

proptest! {
    /// guarantee: host/unknown-fields
    ///
    /// A row's unknown field is only a warning: the file is read, every
    /// one is reported once with the row it is in, the rows still go to
    /// the loader as they are, and the host records which rows `check
    /// --strict` fails on. Unknown fields of rows the loader never reads
    /// (a non-group's `config`) are not warned about.
    #[test]
    fn a_rows_unknown_field_only_warns(
        specs in prop::collection::vec(row_spec(2), 1..4),
    ) {
        let mut count = 0;
        let mut expected = Vec::new();
        let rows: Vec<Value> = specs
            .iter()
            .enumerate()
            .map(|(index, spec)| build_row(spec, index, &mut count, &mut expected))
            .collect();
        let text = serde_json::to_string(&serde_json::json!({ "rows": rows })).unwrap();
        let config = parse(&text).unwrap();
        prop_assert_eq!(config.warnings.len(), count);
        prop_assert_eq!(config.unknown_row_fields.len(), count);
        for (field, id) in &expected {
            let warning = config
                .warnings
                .iter()
                .find(|warning| {
                    warning.contains(&format!("unknown field `{field}` is ignored"))
                        && warning.contains(&format!("id \"{id}\""))
                })
                .unwrap_or_else(|| panic!("no warning for `{field}` in row {id}: {:?}", config.warnings));
            let prefix = format!("{}: ", file().display());
            prop_assert!(warning.starts_with(&prefix));
        }
        prop_assert_eq!(Value::Array(config.rows.clone()), serde_json::json!(rows));
    }
}

// -------------------------------------------------------------- fuzz

proptest! {
    /// guarantee: host/rutis-json-format, host/unknown-fields
    ///
    /// Any text — truncated JSON, wrong types everywhere, unknown fields,
    /// rows that are no rows — is read to an error or a configuration,
    /// never a panic.
    #[test]
    fn any_text_is_read_without_a_panic(
        text in prop::collection::vec(any::<char>(), 0..64)
            .prop_map(|chars| chars.into_iter().collect::<String>()),
    ) {
        let _ = parse(&text);
    }

    /// guarantee: host/rutis-json-format, host/unknown-fields
    ///
    /// The same for any JSON value, nested: no shape of a field panics the
    /// reader.
    #[test]
    fn any_json_is_read_without_a_panic(value in json()) {
        let _ = parse(&value.to_string());
    }
}
