//! The service `loader` (#185): plugins of Node, Python and Go add a row,
//! reload its code, read the state and remove the row by name, and every
//! failure throws the text rutis prints for people, with the row in
//! `data`.
#![cfg(all(feature = "node", feature = "python", feature = "go"))]

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::runtimes::{go_binary, repo, rows_stage, Probe};
use common::MemStore;
use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::host_key;
use rutis_loader::edit::{Editable, NewEntry};
use rutis_loader::runtime::binary::{Binaries, BinaryResolver, BinaryRuntimes};
use rutis_loader::runtime::go::Go;
use rutis_loader::runtime::RuntimeResolver;
use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};

/// Runs the scenario of the design's §7 once it has started (a row cannot
/// edit rows while the loader starts it), recording what each call
/// returned or threw.
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


def tell(error):
    return f"{error.name} {error.code} {error.data} | {error.message}"


async def script(loader, probe):
    async def attempt(label, call):
        try:
            result = await call
            probe.record(f"{label}: ok {result!r}")
        except Exception as error:
            probe.record(f"{label}: {tell(error)}")

    def write(name, text):
        (HERE / name).write_text(text)

    write("weather.py", V1)
    await attempt("create", loader.create({"id": "weather", "name": "py:weather"}))
    await attempt("create again", loader.create({"id": "weather", "name": "py:weather"}))
    await attempt("create unknown", loader.create({"id": "wether", "name": "py:wether"}))
    write("fails.py", FAILS)
    await attempt("create fails", loader.create({"id": "fails", "name": "py:fails"}))
    write("syntax.py", SYNTAX)
    await attempt("create syntax", loader.create({"id": "syntax", "name": "py:syntax"}))
    await attempt("create no id", loader.create({"name": "py:tiny"}))
    write("strict.py", STRICT)
    await attempt("create strict", loader.create({"id": "strict", "name": "py:strict", "config": {"units": "kelvin"}}))
    write("weather.py", SYNTAX)
    await attempt("reload syntax", loader.reload("weather"))
    write("weather.py", FAILS)
    await attempt("reload fails", loader.reload("weather"))
    write("weather.py", V2)
    await attempt("reload", loader.reload("weather"))
    await attempt("reload unknown", loader.reload("wether"))
    status = await loader.status()
    probe.record("status: " + ", ".join(f"{row['id']} {row['state']}" for row in status["rows"]))
    probe.record("services: " + ", ".join(f"{s['name']} {s['provider']}" for s in status["services"] if s["name"] != "probe"))
    await attempt("remove", loader.remove("weather"))
    await attempt("remove unknown", loader.remove("weather"))
    status = await loader.status()
    probe.record("status after: " + ", ".join(row["id"] for row in status["rows"]))


def apply(ctx, config):
    tasks.append(asyncio.get_running_loop().create_task(script(ctx.use("loader"), ctx.use("probe"))))
"#;

/// The same in a few calls from Node.
const JS_MAKER: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['loader', 'probe'],
  apply(ctx) {
    const loader = ctx.use('loader')
    const probe = ctx.use('probe')
    setTimeout(async () => {
      const id = await loader.create({ id: 'from-js', name: 'py:tiny' })
      await loader.reload(id)
      const status = await loader.status()
      const state = status.rows.find(row => row.id === id).state
      await loader.remove(id)
      try { await loader.remove(id) } catch (error) {
        probe.record(`js: ${id} ${state} / ${error.name} ${error.data.row} ${error.message}`)
      }
    })
  },
})
"#;

const PY_TINY: &str = "def apply(ctx, config):\n    pass\n";

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    js: PathBuf,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let py = dir.path().join("py");
    let js = dir.path().join("js");
    std::fs::create_dir_all(&py).unwrap();
    std::fs::create_dir_all(&js).unwrap();
    std::fs::write(py.join("maker.py"), PY_MAKER).unwrap();
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
    let node = LocalRuntime::node(
        repo().join("node/rutis-runtime"),
        repo().join("node/rutis-runtime/package.json"),
    );
    let python = LocalRuntime::python(&py).python_path(repo().join("python/rutis"));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
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
            .with_shared(python_rows.clone())
            .with_shared(node_rows.clone()),
        LoaderOptions {
            catalog,
            persist: Arc::new(store.clone()),
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    rows_stage(&root, node_rows).await;
    rows_stage(&root, python_rows).await;
    root.plugin(BinaryRuntimes::<Go>::new(go, dir.path()).idle(None))
        .await
        .unwrap();
    loader
        .reconcile(
            vec![Layer::new("user", Vec::<Patch>::new())],
            Some(Editable::new("user", store.version())),
        )
        .await
        .unwrap();
    Fixture {
        root,
        loader,
        probe,
        js,
        _dir: dir,
    }
}

impl Fixture {
    async fn expect(&self, line: &str) {
        Guard(HANG_GUARD * 4)
            .eventually_with(
                || self.probe.has(line).then_some(()),
                &format!("{line:?} to be recorded"),
                || format!("recorded: {:#?}", self.probe.lines()),
            )
            .await
    }

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

    async fn add(&self, id: &str, name: &str) {
        let entry = NewEntry {
            id: Some(id.into()),
            name: name.into(),
            ..NewEntry::default()
        };
        self.loader.create(entry, None, None).await.unwrap();
    }

    fn js(&self) -> String {
        self.js.join("js_maker.mjs").to_string_lossy().into_owned()
    }
}

/// A Python plugin manages a row through `loader`: every call of the
/// design's §7 returns, or throws an `Error` with the row in `data` and
/// the text word for word.
/// guarantee: loader/service
#[tokio::test(flavor = "multi_thread")]
async fn a_python_plugin_manages_rows_through_the_loader_service() {
    let fixture = fixture().await;
    fixture.add("maker", "py:maker").await;
    let after = fixture.expect_prefix("status after: ").await;
    let error = |row: &str, text: &str| format!("Error Error {{'row': '{row}'}} | {text}");
    let mut expected = vec![
        "create: ok 'weather'".to_owned(),
        format!(
            "create again: {}",
            error("weather", "a row weather already exists")
        ),
        format!(
            "create unknown: {}",
            error(
                "wether",
                "wether: cannot load \"py:wether\": ModuleNotFoundError: No module named 'wether'"
            )
        ),
        format!(
            "create fails: {}",
            error("fails", "fails: KeyError: 'units'")
        ),
        format!(
            "create syntax: {}",
            error(
                "syntax",
                "syntax: cannot load syntax.py: SyntaxError: expected ':' (syntax.py:3)"
            )
        ),
        format!(
            "create strict: {}",
            error(
                "strict",
                "strict: config: units should be \"metric\" or \"imperial\", got \"kelvin\""
            )
        ),
        format!(
            "reload syntax: {}",
            error(
                "weather",
                "weather: cannot reload: cannot load weather.py: SyntaxError: expected ':' \
                 (weather.py:3); the previous version keeps running"
            )
        ),
        format!(
            "reload fails: {}",
            error("weather", "weather: cannot reload: KeyError: 'units'")
        ),
        "reload: ok None".to_owned(),
        format!("reload unknown: {}", error("wether", "no row wether")),
        "services: loader None".to_owned(),
        "remove: ok None".to_owned(),
        format!("remove unknown: {}", error("weather", "no row weather")),
    ];
    let lines = fixture.probe.lines();
    // The id `create` generates: 8 hex digits.
    let generated = lines
        .iter()
        .find_map(|line| line.strip_prefix("create no id: ok '"))
        .and_then(|rest| rest.strip_suffix('\''))
        .expect("create no id returned an id");
    assert!(
        generated.len() == 8 && generated.chars().all(|c| c.is_ascii_hexdigit()),
        "{generated}"
    );
    expected.push(format!(
        "status: maker running, weather running, {generated} running"
    ));
    for line in &expected {
        assert!(lines.contains(line), "{line:?} not in {lines:#?}");
    }
    // The syntax error did not stop the previous version; the fixed code runs.
    assert!(
        lines.contains(&"weather version 2".to_owned()),
        "{lines:#?}"
    );
    // Removed: neither the row nor its service is left.
    assert_eq!(after, format!("status after: maker, {generated}"));
    fixture.root.shutdown().await.unwrap();
}

/// A Node plugin and a Go plugin use the same four methods.
/// guarantee: loader/service
#[tokio::test(flavor = "multi_thread")]
async fn node_and_go_plugins_manage_rows_through_the_loader_service() {
    let fixture = fixture().await;
    let js = fixture.js();
    fixture.add("js-maker", &js).await;
    fixture.add("go-maker", "go:go_maker").await;
    fixture
        .expect("js: from-js running / Error from-js no row from-js")
        .await;
    fixture
        .expect("go: from-go running / Error map[row:from-go] no row from-go")
        .await;
    fixture.root.shutdown().await.unwrap();
}
