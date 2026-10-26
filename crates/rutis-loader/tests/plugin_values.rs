//! Plugin definitions across languages (`docs/design-plugin-values-2026-10-25.md`
//! §9), the same cases for each runtime: a row hands a plugin definition to a
//! Rust service in `apply`; Rust host parts in two instances each mount a
//! copy, which runs in the row's runtime with the instance's services; the
//! row's unload ends the copies first; a reloaded row's new definition
//! mounts again. Python, Go, Swift and Kotlin run the same origin, written in
//! their SDK (Go's, Swift's and Kotlin's compiled into their fixtures:
//! `tests/fixtures/go/plugins/values`,
//! `swift/rutis/Sources/RutisFixturePlugins/PluginValues.swift`,
//! `kotlin/rutis/rutis-conformance/.../plugins/PluginValues.kt`).
#![cfg(all(
    unix,
    any(
        feature = "node",
        feature = "bun",
        feature = "python",
        feature = "go",
        all(feature = "swift", target_os = "macos"),
        feature = "kotlin"
    )
))]

mod common;

#[cfg(any(feature = "node", feature = "bun"))]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(feature = "go")]
use common::runtimes::go_binary;
#[cfg(any(feature = "node", feature = "bun", feature = "python"))]
use common::runtimes::repo;
use common::runtimes::{title, NoConfig, Probe, Title, Tools, COLD};
use common::Scoped;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView, PluginFactory};
use rutis::{Ctx, Definition, Error};
#[cfg(any(
    feature = "node",
    feature = "bun",
    feature = "python",
    feature = "kotlin"
))]
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session::{host_key, host_key_in};
use rutis_loader::edit::Instance;
#[cfg(feature = "go")]
use rutis_loader::runtime::go::{GoBinaries, GoResolver, GoRuntimes};
#[cfg(any(
    feature = "node",
    feature = "bun",
    feature = "python",
    feature = "kotlin"
))]
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{
    Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, Resolver, ServiceCatalog,
};
use serde_json::json;

// ── The part registry (Dim's, in miniature) ─────────────────────

/// What plugins hand their session parts to: an ordinary Rust service.
#[derive(Default)]
struct Parts {
    registered: Mutex<Vec<(String, Definition)>>,
}

#[rutis::service]
impl Parts {
    pub fn register(&self, scope: String, def: Definition) {
        self.registered.lock().unwrap().push((scope, def));
    }

    /// A data parameter: a plugin definition is refused here, as before.
    pub fn takes_data(&self, value: serde_json::Value) -> String {
        value.to_string()
    }

    /// The definition registered last for `scope`, handed back: it goes
    /// home as the original value (the Swift and Kotlin origins check it).
    pub fn latest(&self, scope: String) -> Option<Definition> {
        self.registered
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(of, _)| *of == scope)
            .map(|(_, def)| def.clone())
    }
}

impl Parts {
    /// The definitions for `scope` that have not ended.
    fn of(&self, scope: &str) -> Vec<Definition> {
        self.registered
            .lock()
            .unwrap()
            .iter()
            .filter(|(of, def)| of == scope && !def.is_ended())
            .map(|(_, def)| def.clone())
            .collect()
    }

    fn all(&self) -> Vec<Definition> {
        self.registered
            .lock()
            .unwrap()
            .iter()
            .map(|(_, def)| def.clone())
            .collect()
    }

    async fn wait_for(&self, count: usize) {
        COLD.eventually_with(
            || (self.of("session").len() >= count).then_some(()),
            &format!("{count} session parts registered"),
            || format!("{:?}", self.all()),
        )
        .await
    }
}

// ── The host part: mounts every session part in its instance ─────

struct Host {
    title: String,
    mounted: Arc<Mutex<Vec<FiberView>>>,
}

impl DynPlugin for Host {
    fn name(&self) -> &str {
        "host"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let parts = ctx.get::<Parts>().expect("the registry");
            for def in parts.of("session") {
                let view = ctx.plugin(def.at(ctx, json!({ "n": self.title }))?);
                self.mounted.lock().unwrap().push(view);
            }
            Ok(Effect::Done)
        })
    }
}

struct HostFactory {
    title: String,
    mounted: Arc<Mutex<Vec<FiberView>>>,
}

impl PluginFactory<NoConfig> for HostFactory {
    fn name(&self) -> &str {
        "host"
    }

    fn build(&self, _: &NoConfig) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Host {
            title: self.title.clone(),
            mounted: self.mounted.clone(),
        }))
    }
}

/// The instance's `tools`.
struct ToolsPlugin(
    rutis::host::Key<dyn rutis_bridge::session::HostDispatch>,
    String,
);

impl DynPlugin for ToolsPlugin {
    fn name(&self) -> &str {
        "tools"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            ctx.provide_at(self.0.clone(), Arc::new(Tools(self.1.clone())))?;
            Ok(Effect::Done)
        })
    }
}

struct ToolsFactory(
    rutis::host::Key<dyn rutis_bridge::session::HostDispatch>,
    String,
);

impl PluginFactory<NoConfig> for ToolsFactory {
    fn name(&self) -> &str {
        "tools"
    }

    fn build(&self, _: &NoConfig) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(ToolsPlugin(self.0.clone(), self.1.clone())))
    }
}

fn builtins(mounted: Arc<Mutex<Vec<FiberView>>>) -> Scoped {
    let mut builtins = Scoped::new();
    // The host part comes first: its copies wait for `tools`.
    builtins.register::<NoConfig, _, _>("host", move |build| {
        Ok(HostFactory {
            title: title(build),
            mounted: mounted.clone(),
        })
    });
    builtins.register::<NoConfig, _, _>("tools", |build| {
        let instance = build.instance("session")?;
        Ok(ToolsFactory(host_key_in("tools", instance), title(build)))
    });
    builtins
}

// ── The rows ────────────────────────────────────────────────────

/// The origin: prepares a value, hands a session part using it to `parts`,
/// and provides `maker`, whose `make` returns another one.
#[cfg(any(feature = "node", feature = "bun"))]
const ORIGIN: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['parts', 'probe'],
  async apply(ctx) {
    const probe = ctx.use('probe')
    const prepared = 'prepared in apply'
    const part = (tag) => definePlugin({
      inject: ['tools', 'probe'],
      apply(ctx, config) {
        const probe = ctx.use('probe')
        probe.record(`${tag} ${config.n} sees ${ctx.use('tools').title()} with ${prepared}`)
        ctx.effect(() => { probe.record(`${tag} ${config.n} down`) })
      },
    })
    await ctx.use('parts').register('session', part('part'))
    try {
      await ctx.use('parts').takesData(part('data'))
      probe.record('data took a definition')
    } catch (error) {
      probe.record(`data refused: ${error.message}`)
    }
    ctx.provide('maker', { make() { return part('made') } })
    ctx.effect(() => { probe.record('origin down') })
  },
})
"#;

/// A Cordis plugin handing over Cordis plugins as they are written: an
/// object with `apply`, a function and a class (Node only).
#[cfg(feature = "node")]
const CORDIS: &str = r#"
export const inject = ['parts', 'probe']
export async function apply(ctx) {
  const prepared = 'cordis prepared'
  const seen = (ctx, kind, config) =>
    ctx.get('probe').record(`${kind} ${config.n} sees ${ctx.get('tools').title()} with ${prepared}`)
  const object = { inject: ['tools', 'probe'], apply(ctx, config) { seen(ctx, 'object', config) } }
  function functional(ctx, config) { seen(ctx, 'function', config) }
  functional.inject = ['tools', 'probe']
  class Classy {
    static inject = ['tools', 'probe']
    constructor(ctx, config) { seen(ctx, 'class', config) }
  }
  const parts = ctx.get('parts')
  await parts.register('session', object)
  await parts.register('session', functional)
  await parts.register('session', Classy)
}
"#;

/// Ends its runtime's process soon after it loads.
const CRASH: &str = "export function apply() { setTimeout(() => process.exit(17), 200) }\n";

/// ORIGIN in Python.
#[cfg(feature = "python")]
const PY_ORIGIN: &str = r#"
import inspect

from rutis import define_plugin

inject = ["parts", "probe"]


async def settle(value):
    return await value if inspect.isawaitable(value) else value


def part(tag, prepared):
    def apply(ctx, config):
        probe = ctx.use("probe")
        title = ctx.use("tools").title()
        probe.record(f"{tag} {config['n']} sees {title} with {prepared}")
        ctx.effect(lambda: probe.record(f"{tag} {config['n']} down"))

    return define_plugin(apply, inject=["tools", "probe"])


class Maker:
    def __init__(self, prepared):
        self.prepared = prepared

    def make(self):
        return part("made", self.prepared)


async def apply(ctx, config):
    probe = ctx.use("probe")
    prepared = "prepared in apply"
    await settle(ctx.use("parts").register("session", part("part", prepared)))
    try:
        await settle(ctx.use("parts").takesData(part("data", prepared)))
        probe.record("data took a definition")
    except Exception as error:
        probe.record(f"data refused: {error}")
    ctx.provide("maker", Maker(prepared))
    ctx.effect(lambda: probe.record("origin down"))
"#;

/// CRASH in Python.
#[cfg(feature = "python")]
const PY_CRASH: &str = "import os, threading\n\n\ndef apply(ctx, config):\n    threading.Timer(0.2, lambda: os._exit(17)).start()\n";

#[derive(Clone, Copy, Debug)]
enum Lang {
    #[cfg(feature = "node")]
    Node,
    #[cfg(feature = "bun")]
    Bun,
    /// The origin is `py:origin`.
    #[cfg(feature = "python")]
    Python,
    /// The origin and the crash are plugins of the Go fixture binary
    /// (`tests/fixtures/go/plugins/values`).
    #[cfg(feature = "go")]
    Go,
    /// The origin and the crash are plugins of the Swift package's
    /// `RutisMultilang` binary: `values_origin`, `values_crash`.
    #[cfg(all(feature = "swift", target_os = "macos"))]
    Swift,
    /// The origin and the crash are plugins of the Kotlin conformance jar:
    /// `values_origin`, `values_crash`.
    #[cfg(feature = "kotlin")]
    Kotlin,
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    parts: Arc<Parts>,
    mounted: Arc<Mutex<Vec<FiberView>>>,
    origin: serde_json::Value,
    lang: Lang,
    /// Rows added besides the origin and the session group.
    extra: Mutex<Vec<serde_json::Value>>,
    /// The rows' sources (or, for Go, the runtimes' directory), kept until
    /// the fixture ends.
    #[cfg_attr(
        not(any(feature = "node", feature = "bun", feature = "python")),
        allow(dead_code)
    )]
    dir: tempfile::TempDir,
}

/// What starts a runtime's rows, once the loader is mounted.
type Start = Box<dyn FnOnce(&Ctx)>;

/// Starts the rows of a runtime resolver, once the loader is mounted.
#[cfg(any(
    feature = "node",
    feature = "bun",
    feature = "python",
    feature = "kotlin"
))]
fn rows_plugin(rows: Arc<RuntimeResolver>) -> Start {
    Box::new(move |root: &Ctx| {
        root.plugin(RuntimeRowsPlugin::new(rows));
    })
}

/// The JavaScript origin, `origin.mjs` in `dir`.
#[cfg(any(feature = "node", feature = "bun"))]
fn js_origin(dir: &std::path::Path) -> PathBuf {
    std::fs::write(dir.join("package.json"), "{}").unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let file: PathBuf = dir.join("origin.mjs");
    std::fs::write(
        &file,
        ORIGIN.replace("PLUGIN", url::Url::from_file_path(&sdk).unwrap().as_str()),
    )
    .unwrap();
    file
}

async fn fixture(lang: Lang) -> Fixture {
    let dir = tempfile::tempdir().unwrap();

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    root.provide(Parts::default()).unwrap();
    let parts = root.get::<Parts>().unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("probe");
    catalog.register_shared("parts");
    catalog.register_shared("maker");
    catalog.register_shared_instance("tools", "session");

    let mounted = Arc::new(Mutex::new(Vec::new()));
    // `start` starts what runs the rows, once the loader is mounted.
    let (rows, origin, start): (Arc<dyn Resolver>, _, Start) = match lang {
        #[cfg(feature = "node")]
        Lang::Node => {
            let file = js_origin(dir.path());
            let node = LocalRuntime::node(
                repo().join("node/rutis-runtime"),
                repo().join("node/rutis-runtime/package.json"),
            );
            let rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
            root.plugin(node);
            let start = rows_plugin(rows.clone());
            (rows, json!(file.to_string_lossy()), start)
        }
        #[cfg(feature = "bun")]
        Lang::Bun => {
            js_origin(dir.path());
            let bun = LocalRuntime::bun(repo().join("bun/rutis-bun"), dir.path());
            let rows = Arc::new(RuntimeResolver::modules(bun.handle()).with_catalog(&catalog));
            root.plugin(bun);
            let start = rows_plugin(rows.clone());
            (rows, json!("bun:./origin.mjs"), start)
        }
        #[cfg(feature = "python")]
        Lang::Python => {
            std::fs::write(dir.path().join("origin.py"), PY_ORIGIN).unwrap();
            let python = LocalRuntime::python(dir.path()).python_path(repo().join("python/rutis"));
            let rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
            root.plugin(python);
            let start = rows_plugin(rows.clone());
            (rows, json!("py:origin"), start)
        }
        #[cfg(feature = "go")]
        Lang::Go => {
            let go = Arc::new(
                GoResolver::new(GoBinaries::new().file(go_binary())).with_catalog(&catalog),
            );
            let runtimes = GoRuntimes::new(go.clone(), dir.path()).idle(None);
            let start: Start = Box::new(move |root: &Ctx| {
                root.plugin(runtimes);
            });
            (go, json!("go:pv_origin"), start)
        }
        #[cfg(all(feature = "swift", target_os = "macos"))]
        Lang::Swift => {
            use rutis_loader::runtime::swift::{SwiftBinaries, SwiftResolver, SwiftRuntimes};
            let plugins = dir.path().join("plugins");
            std::fs::create_dir_all(&plugins).unwrap();
            std::fs::copy(
                common::runtimes::swift_binary("RutisMultilang", "multilang"),
                plugins.join("multilang"),
            )
            .unwrap();
            let swift = Arc::new(
                SwiftResolver::new(SwiftBinaries::new().dir(&plugins)).with_catalog(&catalog),
            );
            let runtimes = SwiftRuntimes::new(swift.clone(), dir.path()).eager();
            let start: Start = Box::new(move |root: &Ctx| {
                root.plugin(runtimes);
            });
            (swift, json!("swift:values_origin"), start)
        }
        #[cfg(feature = "kotlin")]
        Lang::Kotlin => {
            let project = dir.path().join("kt");
            std::fs::create_dir_all(&project).unwrap();
            let kt =
                LocalRuntime::launcher("kt", common::runtimes::kotlin_launcher(&project), &project);
            let rows = Arc::new(RuntimeResolver::modules(kt.handle()).with_catalog(&catalog));
            root.plugin(kt);
            let start = rows_plugin(rows.clone());
            (rows, json!("kt:values_origin"), start)
        }
    };
    let chain = Chain::new()
        .with(builtins(mounted.clone()))
        .with_shared(rows.clone());
    let plugin = LoaderPlugin::new(
        chain,
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    start(&root);

    let report = loader
        .reconcile(rows_with(Some(&origin), &[]), None)
        .await
        .unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    parts.wait_for(1).await;
    Fixture {
        root,
        loader,
        probe,
        parts,
        mounted,
        origin,
        lang,
        extra: Mutex::default(),
        dir,
    }
}

/// The rows: the origin (when given), `extra`, and the session group.
fn rows_with(origin: Option<&serde_json::Value>, extra: &[serde_json::Value]) -> Vec<Layer> {
    let mut rows = Vec::new();
    if let Some(origin) = origin {
        rows.push(json!({ "id": "origin", "name": origin }));
    }
    rows.extend(extra.iter().cloned());
    rows.push(
        json!({ "id": "session", "group": true, "instanced": true, "config": [
        { "id": "host", "name": "host" },
        { "id": "tools", "name": "tools" }
    ] }),
    );
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

impl Fixture {
    /// Unload the origin row, or load it again.
    async fn origin(&self, loaded: bool) {
        let extra = self.extra.lock().unwrap().clone();
        let report = self
            .loader
            .reconcile(rows_with(loaded.then_some(&self.origin), &extra), None)
            .await
            .unwrap();
        assert!(report.failures.is_empty(), "{report:?}");
    }

    /// Add a row `id` running `source` (JavaScript) in the fixture's
    /// runtime: CRASH there is the crash of each language.
    async fn add(&self, id: &str, source: &str) {
        let name = match self.lang {
            #[cfg(feature = "node")]
            Lang::Node => {
                let file = self.dir.path().join(format!("{id}.mjs"));
                std::fs::write(&file, source).unwrap();
                json!(file.to_string_lossy())
            }
            #[cfg(feature = "bun")]
            Lang::Bun => {
                std::fs::write(self.dir.path().join(format!("{id}.mjs")), source).unwrap();
                json!(format!("bun:./{id}.mjs"))
            }
            #[cfg(feature = "python")]
            Lang::Python => {
                assert_eq!(source, CRASH, "only the crash has a Python version");
                std::fs::write(self.dir.path().join(format!("{id}.py")), PY_CRASH).unwrap();
                json!(format!("py:{id}"))
            }
            #[cfg(feature = "go")]
            Lang::Go => {
                assert_eq!(source, CRASH, "only the crash is in the Go binary");
                json!("go:pv_crash")
            }
            #[cfg(all(feature = "swift", target_os = "macos"))]
            Lang::Swift => {
                assert_eq!(source, CRASH, "only the crash is in the Swift binary");
                json!("swift:values_crash")
            }
            #[cfg(feature = "kotlin")]
            Lang::Kotlin => match source == CRASH {
                true => json!("kt:values_crash"),
                // The jar's other plugins, by their name: `values_<id>`.
                false => json!(format!("kt:values_{id}")),
            },
        };
        self.extra
            .lock()
            .unwrap()
            .push(json!({ "id": id, "name": name }));
        self.origin(true).await;
    }

    async fn create(&self, title: &str) -> Instance {
        self.loader
            .create_instance(&self.root, "session")
            .with(Title(title.into()))
            .await
            .unwrap()
    }

    fn copies(&self) -> Vec<FiberView> {
        self.mounted.lock().unwrap().clone()
    }
}

async fn each_instance_mounts_its_own_copy(lang: Lang) {
    let fixture = fixture(lang).await;
    let definition = fixture.parts.of("session").pop().unwrap();
    assert!(!definition.is_ended());
    fixture.create("A").await;
    fixture.create("B").await;
    // Each copy uses what the origin's apply prepared, and only its own
    // instance's `tools`: it waited for it (the host part comes first).
    fixture
        .probe
        .wait_for("part A sees A with prepared in apply")
        .await;
    fixture
        .probe
        .wait_for("part B sees B with prepared in apply")
        .await;
    assert!(!fixture.probe.has("part A sees B with prepared in apply"));
    // The same value handed over is one definition.
    assert_eq!(fixture.parts.all().len(), 1);
    // A data parameter refuses a definition, as it refuses any reference.
    let refused = fixture
        .probe
        .lines()
        .into_iter()
        .find(|line| line.starts_with("data refused:"))
        .expect("refused");
    assert!(refused.contains("reference"), "{refused}");
    fixture.root.shutdown().await.unwrap();
}

async fn the_origin_s_unload_ends_its_copies_first(lang: Lang) {
    let fixture = fixture(lang).await;
    let definition = fixture.parts.of("session").pop().unwrap();
    fixture.create("A").await;
    fixture.create("B").await;
    fixture
        .probe
        .wait_for("part B sees B with prepared in apply")
        .await;
    fixture.probe.take();

    fixture.origin(false).await;
    fixture.probe.wait_for("origin down").await;
    let lines = fixture.probe.lines();
    let origin = fixture.probe.position("origin down").unwrap();
    for part in ["part A down", "part B down"] {
        let at = fixture
            .probe
            .position(part)
            .unwrap_or_else(|| panic!("{part}: {lines:?}"));
        assert!(at < origin, "{lines:?}");
    }
    tokio::time::timeout(std::time::Duration::from_secs(10), definition.ended())
        .await
        .expect("ended");
    for copy in fixture.copies() {
        assert_eq!(copy.state().state, FiberState::Disposed);
    }
    assert!(definition
        .at(&fixture.root, json!({ "n": "late" }))
        .is_err());

    // Loaded again: a new definition, mounted by the next instance.
    fixture.origin(true).await;
    fixture.parts.wait_for(1).await;
    let again = fixture.parts.of("session").pop().unwrap();
    assert_ne!(again, definition);
    fixture.create("C").await;
    fixture
        .probe
        .wait_for("part C sees C with prepared in apply")
        .await;
    fixture.root.shutdown().await.unwrap();
}

async fn a_returned_definition_belongs_to_the_row_providing_the_service(lang: Lang) {
    let fixture = fixture(lang).await;
    let key = host_key("maker");
    let maker = COLD
        .eventually_with(|| fixture.root.get_at(&key), "maker provided", String::new)
        .await;
    let made = rutis_bridge::session::settle(
        maker
            .invoke("make", rutis_bridge::session::Value::List(Vec::new()))
            .unwrap(),
    )
    .await
    .unwrap();
    let definition = rutis_bridge::runtime::returned(&*maker, made)
        .await
        .unwrap();
    // It injects a service inside `session` instances: not at the root.
    let refused = definition
        .at(&fixture.root, json!({ "n": "root" }))
        .err()
        .expect("outside the instances");
    assert!(refused.to_string().contains("session"), "{refused}");
    // Handed to the registry, it is mounted by the next instance's host part.
    fixture.parts.register("session".into(), definition.clone());
    fixture.create("M").await;
    fixture
        .probe
        .wait_for("made M sees M with prepared in apply")
        .await;
    fixture.origin(false).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), definition.ended())
        .await
        .expect("ended with the row providing maker");
    fixture.probe.wait_for("made M down").await;
    let lines = fixture.probe.lines();
    let made = fixture.probe.position("made M down").unwrap();
    let origin = fixture.probe.position("origin down").unwrap();
    assert!(made < origin, "{lines:?}");
    fixture.root.shutdown().await.unwrap();
}

async fn a_dead_process_ends_its_copies(lang: Lang) {
    let fixture = fixture(lang).await;
    let definition = fixture.parts.of("session").pop().unwrap();
    fixture.create("A").await;
    fixture
        .probe
        .wait_for("part A sees A with prepared in apply")
        .await;
    fixture.add("crash", CRASH).await;
    tokio::time::timeout(std::time::Duration::from_secs(20), definition.ended())
        .await
        .expect("ended with the process");
    for copy in fixture.copies() {
        assert_eq!(copy.state().state, FiberState::Disposed);
    }
    fixture.root.shutdown().await.unwrap();
}

#[cfg(feature = "node")]
mod node {
    use super::*;

    /// Cordis plugins are handed over as they are written, and their
    /// copies run as Cordis plugins.
    #[tokio::test(flavor = "multi_thread")]
    async fn cordis_plugins_are_handed_over_as_they_are() {
        let fixture = fixture(Lang::Node).await;
        fixture.add("cordis", CORDIS).await;
        fixture.parts.wait_for(4).await;
        fixture.create("A").await;
        for kind in ["object", "function", "class"] {
            fixture
                .probe
                .wait_for(&format!("{kind} A sees A with cordis prepared"))
                .await;
        }
        let names: Vec<String> = fixture
            .parts
            .of("session")
            .iter()
            .map(|def| def.name().to_owned())
            .collect();
        assert!(names.contains(&"functional".to_owned()), "{names:?}");
        assert!(names.contains(&"Classy".to_owned()), "{names:?}");
        fixture.root.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dead_process_ends_its_copies() {
        super::a_dead_process_ends_its_copies(Lang::Node).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_instance_mounts_its_own_copy() {
        super::each_instance_mounts_its_own_copy(Lang::Node).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_origin_s_unload_ends_its_copies_first() {
        super::the_origin_s_unload_ends_its_copies_first(Lang::Node).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_returned_definition_belongs_to_the_row_providing_the_service() {
        super::a_returned_definition_belongs_to_the_row_providing_the_service(Lang::Node).await
    }
}

#[cfg(feature = "bun")]
mod bun {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dead_process_ends_its_copies() {
        super::a_dead_process_ends_its_copies(Lang::Bun).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_instance_mounts_its_own_copy() {
        super::each_instance_mounts_its_own_copy(Lang::Bun).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_origin_s_unload_ends_its_copies_first() {
        super::the_origin_s_unload_ends_its_copies_first(Lang::Bun).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_returned_definition_belongs_to_the_row_providing_the_service() {
        super::a_returned_definition_belongs_to_the_row_providing_the_service(Lang::Bun).await
    }
}

#[cfg(feature = "python")]
mod python {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dead_process_ends_its_copies() {
        super::a_dead_process_ends_its_copies(Lang::Python).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_instance_mounts_its_own_copy() {
        super::each_instance_mounts_its_own_copy(Lang::Python).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_origin_s_unload_ends_its_copies_first() {
        super::the_origin_s_unload_ends_its_copies_first(Lang::Python).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_returned_definition_belongs_to_the_row_providing_the_service() {
        super::a_returned_definition_belongs_to_the_row_providing_the_service(Lang::Python).await
    }
}

#[cfg(feature = "go")]
mod go {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dead_process_ends_its_copies() {
        super::a_dead_process_ends_its_copies(Lang::Go).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_instance_mounts_its_own_copy() {
        super::each_instance_mounts_its_own_copy(Lang::Go).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_origin_s_unload_ends_its_copies_first() {
        super::the_origin_s_unload_ends_its_copies_first(Lang::Go).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_returned_definition_belongs_to_the_row_providing_the_service() {
        super::a_returned_definition_belongs_to_the_row_providing_the_service(Lang::Go).await
    }
}

/// The origin asked `parts.latest` for what it registered: a Rust
/// parameter and result going home, decoded by the SDK's client.
#[cfg(any(all(feature = "swift", target_os = "macos"), feature = "kotlin"))]
async fn a_definition_comes_home_as_itself(lang: Lang) {
    let fixture = fixture(lang).await;
    // The origin records either line: a wrong value fails here at once.
    let line = COLD
        .eventually_with(
            || {
                fixture
                    .probe
                    .lines()
                    .into_iter()
                    .find(|line| line.starts_with("part came home as "))
            },
            "the origin told what came home",
            || format!("{:?}", fixture.probe.lines()),
        )
        .await;
    assert_eq!(line, "part came home as itself");
    fixture.root.shutdown().await.unwrap();
}

#[cfg(all(feature = "swift", target_os = "macos"))]
mod swift {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_definition_comes_home_as_itself() {
        super::a_definition_comes_home_as_itself(Lang::Swift).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dead_process_ends_its_copies() {
        super::a_dead_process_ends_its_copies(Lang::Swift).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_instance_mounts_its_own_copy() {
        super::each_instance_mounts_its_own_copy(Lang::Swift).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_origin_s_unload_ends_its_copies_first() {
        super::the_origin_s_unload_ends_its_copies_first(Lang::Swift).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_returned_definition_belongs_to_the_row_providing_the_service() {
        super::a_returned_definition_belongs_to_the_row_providing_the_service(Lang::Swift).await
    }
}

#[cfg(feature = "kotlin")]
mod kotlin {
    use super::*;

    /// A definition handed over through a blocking (not `suspend`) client
    /// method: rutis reads it with `plugins.read` while the calling thread
    /// waits, and the call is answered.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_blocking_call_hands_over_a_definition() {
        let fixture = fixture(Lang::Kotlin).await;
        fixture.add("blocking", "").await;
        fixture.probe.wait_for("blocking register answered").await;
        assert_eq!(fixture.parts.of("blocking").len(), 1);
        fixture.root.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_definition_comes_home_as_itself() {
        super::a_definition_comes_home_as_itself(Lang::Kotlin).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dead_process_ends_its_copies() {
        super::a_dead_process_ends_its_copies(Lang::Kotlin).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_instance_mounts_its_own_copy() {
        super::each_instance_mounts_its_own_copy(Lang::Kotlin).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_origin_s_unload_ends_its_copies_first() {
        super::the_origin_s_unload_ends_its_copies_first(Lang::Kotlin).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_returned_definition_belongs_to_the_row_providing_the_service() {
        super::a_returned_definition_belongs_to_the_row_providing_the_service(Lang::Kotlin).await
    }
}

/// An application's runtime that dials in (R4): its connection is an
/// instance of `phone`, and what its rows hand over mounts only in that
/// instance. The Kotlin conformance jar and the Swift `RutisMultilang`
/// binary, each dialing over WebSocket through `RutisApp` (Swift's on
/// macOS: no system permission involved).
#[cfg(any(feature = "kotlin", all(feature = "swift", target_os = "macos")))]
mod dialed_in {
    use super::*;
    use common::runtimes::id;
    use rutis_bridge::transport::websocket::{Config, ListenerConfig, WebSocketPlugin};
    use rutis_bridge::{AccessPointConfig, AccessPointPlugin, AdmissionPlugin, SharedToken};
    use rutis_loader::runtime::{RuntimeConnection, RuntimeInstances};
    use rutis_test_support::{scaled, HANG_GUARD};
    use tokio::io::AsyncBufReadExt;

    /// The connection's id: the title of its instance's `tools`.
    fn connection(build: &rutis_loader::resolve::Build) -> String {
        build
            .value::<RuntimeConnection>()
            .map(|connection| connection.id().to_string())
            .unwrap_or_default()
    }

    /// The Kotlin conformance jar, served by `Rutis.serve`.
    #[cfg(feature = "kotlin")]
    fn kotlin() -> tokio::process::Command {
        let mut command = tokio::process::Command::new("java");
        command
            .args(["-XX:TieredStopAtLevel=1", "-XX:+UseSerialGC", "-jar"])
            .arg(common::runtimes::kotlin_jar());
        command
    }

    /// The Swift `RutisMultilang` binary, served by `Rutis.serve`.
    #[cfg(all(feature = "swift", target_os = "macos"))]
    fn swift() -> tokio::process::Command {
        tokio::process::Command::new(common::runtimes::swift_binary(
            "RutisMultilang",
            "multilang",
        ))
    }

    /// Dial the access point as the endpoint `name`, with `command`.
    fn dial(
        mut command: tokio::process::Command,
        port: u16,
        name: &str,
        project: &std::path::Path,
    ) -> tokio::process::Child {
        let mut child = command
            .arg(format!("dial:ws://127.0.0.1:{port}/rutis"))
            .args(["--id", name, "--peer", "main"])
            .arg(project)
            .env("RUTIS_TOKEN", "shared")
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("the app starts");
        let mut stderr = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        tokio::spawn(async move {
            while let Ok(Some(line)) = stderr.next_line().await {
                eprintln!("[app] {line}");
            }
        });
        child
    }

    /// The `phone` group: the origin, then `tools`, then the host parts
    /// `hosts` names (mounting every registered part).
    fn rows(hosts: &[&str]) -> Vec<Layer> {
        let mut config = vec![
            json!({ "id": "origin", "name": "phone:values_origin" }),
            json!({ "id": "tools", "name": "tools" }),
        ];
        config.extend(hosts.iter().map(|id| json!({ "id": id, "name": "host" })));
        let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": [{
            "id": "phone", "group": true, "instanced": true, "config": config
        }] }]))
        .unwrap();
        vec![Layer::new("rows", patches)]
    }

    #[cfg(feature = "kotlin")]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_dialed_in_kotlin_app_s_definitions_mount_only_in_its_instance() {
        mount_only_in_its_instance(kotlin()).await
    }

    #[cfg(all(feature = "swift", target_os = "macos"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_dialed_in_swift_app_s_definitions_mount_only_in_its_instance() {
        mount_only_in_its_instance(swift()).await
    }

    async fn mount_only_in_its_instance(app: tokio::process::Command) {
        let dir = tempfile::tempdir().unwrap();
        let probe = Probe::default();
        let root = Ctx::root().unwrap();
        root.provide_at(host_key("probe"), Arc::new(probe.clone()))
            .unwrap();
        root.provide(Parts::default()).unwrap();
        let parts = root.get::<Parts>().unwrap();

        let websocket = WebSocketPlugin::new(Config::new().listener(ListenerConfig::new(
            "public",
            ([127, 0, 0, 1], 0).into(),
            id("main"),
        )))
        .unwrap();
        let transport = websocket.clone();
        (&root.plugin(websocket)).await.unwrap();
        let port = transport
            .transport()
            .unwrap()
            .local_addr("public")
            .unwrap()
            .port();
        (&root.plugin(AdmissionPlugin::new(
            "phone",
            SharedToken::new(id("main"), "shared"),
        )))
            .await
            .unwrap();

        let mut catalog = ServiceCatalog::new();
        catalog.register_shared("probe");
        catalog.register_shared("parts");
        catalog.register_shared("maker");
        catalog.register_shared_instance("tools", "phone");
        let mounted = Arc::new(Mutex::new(Vec::new()));
        let mut builtins = Scoped::new();
        {
            let mounted = mounted.clone();
            builtins.register::<NoConfig, _, _>("host", move |build| {
                Ok(HostFactory {
                    title: connection(build),
                    mounted: mounted.clone(),
                })
            });
        }
        builtins.register::<NoConfig, _, _>("tools", |build| {
            let instance = build.instance("phone")?;
            Ok(ToolsFactory(
                host_key_in("tools", instance),
                connection(build),
            ))
        });
        let instances = RuntimeInstances::new("phone", "phone", &catalog);
        let plugin = LoaderPlugin::new(
            Chain::new().with(builtins).with(instances.clone()),
            LoaderOptions {
                catalog,
                ..LoaderOptions::default()
            },
        );
        let loader = plugin.handle();
        (&root.plugin(plugin)).await.unwrap();
        let report = loader.reconcile(rows(&[]), None).await.unwrap();
        assert!(report.failures.is_empty(), "{report:?}");
        let access = AccessPointPlugin::new(
            AccessPointConfig::new(id("phone"), "websocket", "phone", "public")
                .require("runtime")
                .handshake(scaled(HANG_GUARD)),
        )
        .accept(instances.accept(loader.clone(), root.clone()));
        (&root.plugin(access)).await.unwrap();

        let mut pixel = dial(app, port, "pixel", dir.path());
        parts.wait_for(1).await;
        let definition = parts.of("session").pop().unwrap();
        // Outside the connection's instance, it does not mount.
        let refused = definition
            .at(&root, json!({ "n": "root" }))
            .err()
            .expect("outside the app's instance");
        assert!(
            refused.to_string().contains("inside instance phone "),
            "{refused}"
        );
        // A host part in the instance mounts it there.
        let report = loader.reconcile(rows(&["host"]), None).await.unwrap();
        assert!(report.failures.is_empty(), "{report:?}");
        probe
            .wait_for("part pixel sees pixel with prepared in apply")
            .await;
        // The app goes: its copies end with it.
        pixel.kill().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), definition.ended())
            .await
            .expect("ended with the app's connection");
        for copy in mounted.lock().unwrap().iter() {
            assert_eq!(copy.state().state, FiberState::Disposed);
        }
        root.shutdown().await.unwrap();
    }
}
