//! The service `loader` (#185): plugins of Node, Python and Go add rows,
//! reload their code, read the state and remove them by name, and every
//! failure throws the text rutis prints for people, with the row in
//! `data`: each line of §7 of the design, word for word.
#![cfg(all(feature = "node", feature = "python", feature = "go"))]

mod common;

use std::sync::Arc;

use common::runtimes::{go_binary, greet_as_main, repo, rows_stage, spawner, Probe, TestRuntime};
use common::MemStore;
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::session::host_key;
use rutis_loader::edit::Editable;
use rutis_loader::runtime::binary::{Binaries, BinaryResolver, BinaryRuntimes};
use rutis_loader::runtime::go::Go;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

/// The scenario, in Python: each call's result or error, recorded as
/// `<label>: ok [<id>]` or `<label>: <name> <code> <row> | <message>`.
/// It runs once the row has started: the calls wait for the loader's
/// operation that starts it.
const PY_MAKER: &str = r#"
import asyncio
import pathlib

inject = ["loader", "probe"]
HERE = pathlib.Path(__file__).parent

V1 = 'inject = ["probe"]\n\n\ndef apply(ctx, config):\n    ctx.use("probe").record("weather v1")\n'
SYNTAX = 'inject = ["probe"]\n\ndef apply(ctx, config)\n    pass\n'
V2 = 'inject = ["probe"]\n\n\ndef apply(ctx, config):\n    ctx.use("probe").record("weather version 2")\n'
FAILS = 'def apply(ctx, config):\n    raise KeyError("units")\n'
STRICT = (
    'from rutis import define_plugin\n\n\ndef apply(ctx, config):\n    pass\n\n\n'
    'plugin = define_plugin(apply, config={"type": "object", "properties": '
    '{"units": {"enum": ["metric", "imperial"]}}})\n'
)
tasks = []


async def script(loader, probe):
    async def attempt(label, call):
        try:
            result = await call
            probe.record(f"{label}: ok {result or ''}".rstrip())
        except Exception as error:
            probe.record(f"{label}: {error.name} {error.code} {error.data['row']} | {error.message}")

    def write(name, text):
        (HERE / name).write_text(text)

    def row(name):
        return {"id": name, "name": f"py:{name}"}

    async def states(label):
        status = await loader.status()
        probe.record(f"{label}: " + ", ".join(f"{r['id']} {r['state']}" for r in status["rows"]))

    write("weather.py", V1)
    await attempt("create", loader.create(row("weather")))
    await attempt("create again", loader.create(row("weather")))
    await attempt("create unknown", loader.create(row("wether")))
    write("fails.py", FAILS)
    await attempt("create fails", loader.create(row("fails")))
    write("syntax.py", SYNTAX)
    await attempt("create syntax", loader.create(row("syntax")))
    write("strict.py", STRICT)
    await attempt("create strict", loader.create({"id": "strict", "name": "py:strict", "config": {"units": "kelvin"}}))
    await attempt("create no id", loader.create({"name": "py:tiny"}))
    write("weather.py", SYNTAX)
    await attempt("reload syntax", loader.reload("weather"))
    write("weather.py", FAILS)
    await attempt("reload fails", loader.reload("weather"))
    write("weather.py", V2)
    await attempt("reload", loader.reload("weather"))
    await attempt("reload unknown", loader.reload("wether"))
    await states("status")
    await attempt("remove", loader.remove("weather"))
    await attempt("remove unknown", loader.remove("weather"))
    await states("status after")


def apply(ctx, config):
    tasks.append(asyncio.get_running_loop().create_task(script(ctx.use("loader"), ctx.use("probe"))))
"#;

/// The same scenario in Node.
const JS_MAKER: &str = r#"
import { writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { definePlugin } from 'PLUGIN'

const V1 = 'inject = ["probe"]\n\n\ndef apply(ctx, config):\n    ctx.use("probe").record("weather v1")\n'
const SYNTAX = 'inject = ["probe"]\n\ndef apply(ctx, config)\n    pass\n'
const V2 = 'inject = ["probe"]\n\n\ndef apply(ctx, config):\n    ctx.use("probe").record("weather version 2")\n'
const FAILS = 'def apply(ctx, config):\n    raise KeyError("units")\n'
const STRICT = 'from rutis import define_plugin\n\n\ndef apply(ctx, config):\n    pass\n\n\n' +
  'plugin = define_plugin(apply, config={"type": "object", "properties": {"units": {"enum": ["metric", "imperial"]}}})\n'

export default definePlugin({
  inject: ['loader', 'probe'],
  config: { type: 'object', properties: { dir: { type: 'string' } } },
  apply(ctx, config) {
    const loader = ctx.use('loader')
    const probe = ctx.use('probe')
    const write = (name, text) => writeFileSync(join(config.dir, name), text)
    const row = (name) => ({ id: name, name: `py:${name}` })
    const attempt = async (label, call) => {
      try {
        const result = await call
        probe.record(`${label}: ok ${result ?? ''}`.trimEnd())
      } catch (error) {
        probe.record(`${label}: ${error.name} ${error.code} ${error.data.row} | ${error.message}`)
      }
    }
    const states = async (label) => {
      const status = await loader.status()
      probe.record(`${label}: ` + status.rows.map(r => `${r.id} ${r.state}`).join(', '))
    }
    setTimeout(async () => {
      write('weather.py', V1)
      await attempt('create', loader.create(row('weather')))
      await attempt('create again', loader.create(row('weather')))
      await attempt('create unknown', loader.create(row('wether')))
      write('fails.py', FAILS)
      await attempt('create fails', loader.create(row('fails')))
      write('syntax.py', SYNTAX)
      await attempt('create syntax', loader.create(row('syntax')))
      write('strict.py', STRICT)
      await attempt('create strict', loader.create({ id: 'strict', name: 'py:strict', config: { units: 'kelvin' } }))
      await attempt('create no id', loader.create({ name: 'py:tiny' }))
      write('weather.py', SYNTAX)
      await attempt('reload syntax', loader.reload('weather'))
      write('weather.py', FAILS)
      await attempt('reload fails', loader.reload('weather'))
      write('weather.py', V2)
      await attempt('reload', loader.reload('weather'))
      await attempt('reload unknown', loader.reload('wether'))
      await states('status')
      await attempt('remove', loader.remove('weather'))
      await attempt('remove unknown', loader.remove('weather'))
      await states('status after')
    })
  },
})
"#;

/// Adds and removes a row, for the hosts whose edits cannot be stored.
const PY_EDITS: &str = r#"
import asyncio

inject = ["loader", "probe"]
tasks = []


async def script(loader, probe):
    for label, call in [
        ("create", lambda: loader.create({"id": "made", "name": "py:tiny"})),
        ("remove", lambda: loader.remove("made")),
    ]:
        try:
            probe.record(f"{label}: ok {await call() or ''}".rstrip())
        except Exception as error:
            probe.record(f"{label}: {error.name} {error.code} {error.data['row']} | {error.message}")
    probe.record("edits: done")


def apply(ctx, config):
    tasks.append(asyncio.get_running_loop().create_task(script(ctx.use("loader"), ctx.use("probe"))))
"#;

const PY_TINY: &str = "def apply(ctx, config):\n    pass\n";

/// Where the loader keeps the rows plugins add.
enum Storage {
    /// An editable layer stored in memory, failing to save when `fails`.
    Editable { fails: bool },
    /// No editable layer.
    None,
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    _dir: tempfile::TempDir,
}

/// A loader running the row `maker` (`name`, with `config`) in a host
/// layer, plugins' edits going to the editable layer `storage` says.
async fn fixture(storage: Storage, name: &str, config: Value) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let py = dir.path().join("py");
    let js = dir.path().join("js");
    std::fs::create_dir_all(&py).unwrap();
    std::fs::create_dir_all(&js).unwrap();
    std::fs::write(py.join("maker.py"), PY_MAKER).unwrap();
    std::fs::write(py.join("edits.py"), PY_EDITS).unwrap();
    std::fs::write(py.join("tiny.py"), PY_TINY).unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    std::fs::write(js.join("js_maker.mjs"), JS_MAKER.replace("PLUGIN", &sdk)).unwrap();

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("probe");
    greet_as_main(&root);
    let node = TestRuntime::node(&repo().join("node/rutis-runtime/package.json"));
    let python = TestRuntime::python(&py);
    let sessions = Arc::new(SessionResolver::npm(&catalog, "node"));
    sessions.runtime(node.handle());
    sessions.runtime(python.handle());
    let go = Arc::new(
        BinaryResolver::<Go>::new(Binaries::new().file(go_binary()))
            .with_catalog(&catalog)
            .reserve(["node", "py"]),
    );
    root.plugin(node);
    root.plugin(python);
    let store = MemStore::default();
    let plugin = LoaderPlugin::new(
        Chain::new()
            .with_shared(go.clone())
            .with_shared(sessions.clone()),
        LoaderOptions {
            catalog,
            persist: Arc::new(store.clone()),
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    sessions.attach(&loader);
    root.plugin(plugin).await.unwrap();
    rows_stage(&root, &sessions, "node").await;
    rows_stage(&root, &sessions, "py").await;
    root.plugin(BinaryRuntimes::<Go>::new(go, dir.path(), spawner()).idle(None))
        .await
        .unwrap();
    let name = match name {
        "js" => js.join("js_maker.mjs").to_string_lossy().into_owned(),
        name => name.to_owned(),
    };
    let config = match config {
        Value::Null => json!({ "dir": py.to_string_lossy() }),
        config => config,
    };
    // The host's own row, in a layer of its own; the plugins' rows go to
    // the editable layer.
    let host: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
        { "id": "maker", "name": name, "config": config },
    ] }]))
    .unwrap();
    let mut layers = vec![Layer::new("host", host)];
    let editable = match storage {
        Storage::Editable { fails } => {
            store.set_fail(fails);
            layers.push(Layer::new("user", Vec::new()));
            Some(Editable::new("user", store.version()))
        }
        Storage::None => None,
    };
    loader.reconcile(layers, editable).await.unwrap();
    Fixture {
        root,
        loader,
        probe,
        _dir: dir,
    }
}

impl Fixture {
    /// The first line recorded starting with `prefix`, once there is one.
    async fn expect_prefix(&self, prefix: &str) -> String {
        Guard(HANG_GUARD * 4)
            .eventually_with(
                || {
                    self.probe
                        .lines()
                        .into_iter()
                        .find(|line| line.starts_with(prefix))
                },
                &format!("a line starting with {prefix:?}"),
                || format!("recorded: {:#?}", self.probe.lines()),
            )
            .await
    }
}

/// The lines every maker records, whatever its language, but the
/// generated id's.
fn scenario() -> Vec<String> {
    let error = |label: &str, row: &str, text: &str| format!("{label}: Error Error {row} | {text}");
    vec![
        "create: ok weather".to_owned(),
        error("create again", "weather", "a row weather already exists"),
        error(
            "create unknown",
            "wether",
            "wether: cannot load \"py:wether\": ModuleNotFoundError: No module named 'wether'",
        ),
        error("create fails", "fails", "fails: KeyError: 'units'"),
        error(
            "create syntax",
            "syntax",
            "syntax: cannot load syntax.py: SyntaxError: expected ':' (syntax.py:3)",
        ),
        error(
            "create strict",
            "strict",
            "strict: config: units should be \"metric\" or \"imperial\", got \"kelvin\"",
        ),
        error(
            "reload syntax",
            "weather",
            "weather: cannot reload: cannot load weather.py: SyntaxError: expected ':' \
             (weather.py:3); the previous version keeps running",
        ),
        error(
            "reload fails",
            "weather",
            "weather: cannot reload: KeyError: 'units'",
        ),
        "reload: ok".to_owned(),
        error("reload unknown", "wether", "no row wether"),
        "remove: ok".to_owned(),
        error("remove unknown", "weather", "no row weather"),
    ]
}

/// Runs the maker `name` and compares what it recorded with §7, word for
/// word.
async fn run_scenario(name: &str, config: Value) {
    let fixture = fixture(Storage::Editable { fails: false }, name, config).await;
    let after = fixture.expect_prefix("status after: ").await;
    let lines = fixture.probe.lines();
    for line in scenario() {
        assert!(lines.contains(&line), "{line:?} not in {lines:#?}");
    }
    // The id `create` generates: 8 hex digits.
    let generated = lines
        .iter()
        .find_map(|line| line.strip_prefix("create no id: ok "))
        .expect("create no id returned an id");
    assert!(
        generated.len() == 8 && generated.chars().all(|c| c.is_ascii_hexdigit()),
        "{generated}"
    );
    let status = format!("status: maker running, weather running, {generated} running");
    assert!(lines.contains(&status), "{status:?} not in {lines:#?}");
    // The syntax error left the previous version running; the fixed code runs.
    assert!(
        lines.contains(&"weather version 2".to_owned()),
        "{lines:#?}"
    );
    // Removed: nothing of the row is left.
    assert_eq!(
        after,
        format!("status after: maker running, {generated} running")
    );
    fixture.root.shutdown().await.unwrap();
}

/// A Python plugin manages rows through `loader`, every line of §7 word
/// for word.
/// guarantee: loader/service
#[tokio::test(flavor = "multi_thread")]
async fn a_python_plugin_manages_rows_through_the_loader_service() {
    run_scenario("py:maker", json!({})).await;
}

/// The same from Node.
/// guarantee: loader/service
#[tokio::test(flavor = "multi_thread")]
async fn a_node_plugin_manages_rows_through_the_loader_service() {
    run_scenario("js", Value::Null).await;
}

/// The same from Go.
/// guarantee: loader/service
#[tokio::test(flavor = "multi_thread")]
async fn a_go_plugin_manages_rows_through_the_loader_service() {
    run_scenario("go:go_maker", Value::Null).await;
}

/// A host keeping no editable configuration: adding or removing a row
/// says so, and nothing changes.
/// guarantee: loader/service
#[tokio::test(flavor = "multi_thread")]
async fn without_an_editable_configuration_rows_cannot_be_added_or_removed() {
    let fixture = fixture(Storage::None, "py:edits", json!({})).await;
    fixture.expect_prefix("edits: done").await;
    let text = "this host keeps no editable configuration, so rows cannot be added or removed";
    let lines = fixture.probe.lines();
    for line in [
        format!("create: Error Error made | {text}"),
        format!("remove: Error Error made | {text}"),
    ] {
        assert!(lines.contains(&line), "{line:?} not in {lines:#?}");
    }
    assert!(fixture.loader.get("made").is_none());
    fixture.root.shutdown().await.unwrap();
}

/// An edit in effect that could not be stored says what it did and why it
/// was not stored.
/// guarantee: loader/service
#[tokio::test(flavor = "multi_thread")]
async fn an_edit_that_could_not_be_stored_says_so() {
    let fixture = fixture(Storage::Editable { fails: true }, "py:edits", json!({})).await;
    fixture.expect_prefix("edits: done").await;
    let lines = fixture.probe.lines();
    for line in [
        "create: Error Error made | made: added, but disk full",
        "remove: Error Error made | made: removed, but disk full",
    ] {
        assert!(
            lines.contains(&line.to_owned()),
            "{line:?} not in {lines:#?}"
        );
    }
    fixture.root.shutdown().await.unwrap();
}
