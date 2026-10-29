//! Rust services marked `#[rutis::service]` and provided as usual reach
//! the rows of every language by name, with no list and no
//! `HostDispatch` (#510): Node, Python, Go and Bun rows call them; one
//! provided in an instance is that instance's own; one that goes stops its
//! users.
#![cfg(all(feature = "node", feature = "python", feature = "go"))]

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use common::runtimes::{
    go_binary, greet_as_main, layer, repo, row_state, rows_stage, spawner, Probe, TestRuntime,
};
use rutis::host::{BoxFuture, CtxHost, FiberState, Key};
use rutis::{Ctx, Error};
use rutis_bridge::session::host_key;
use rutis_loader::report::{Wait, WaitReason};
use rutis_loader::runtime::binary::{Binaries, BinaryResolver, BinaryRuntimes};
use rutis_loader::runtime::go::Go;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Builtins, Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use rutis_test_support::{Guard, HANG_GUARD};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    title: String,
    day: u32,
}

#[derive(Debug, Serialize, thiserror::Error)]
pub enum CalendarError {
    #[error("no entry on day {day}")]
    Missing { day: u32 },
}

/// The host's calendar, provided at the root.
#[derive(Default)]
pub struct Calendar {
    entries: Mutex<Vec<Entry>>,
}

#[rutis::service]
impl Calendar {
    pub async fn add(&self, entry: Entry) -> usize {
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry);
        entries.len()
    }

    pub fn on_day(&self, day: u32) -> Result<Entry, CalendarError> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|entry| entry.day == day)
            .cloned()
            .ok_or(CalendarError::Missing { day })
    }

    /// The row calling, from its context.
    pub fn caller(&self, ctx: &Ctx) -> Option<String> {
        ctx.row()
    }

    /// Calls back into the caller for each day.
    pub async fn each(
        &self,
        days: Vec<u32>,
        callback: impl Fn(u32) -> BoxFuture<'static, Result<String, Error>>,
    ) -> Result<Vec<String>, Error> {
        let mut out = Vec::new();
        for day in days {
            out.push(callback(day).await?);
        }
        Ok(out)
    }
}

/// Not marked: not served to other languages.
pub struct Hidden;

/// Each `session` instance's own notes.
pub struct Notes {
    owner: String,
}

#[rutis::service]
impl Notes {
    pub fn owner(&self) -> String {
        self.owner.clone()
    }
}

const JS_CALENDAR: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['calendar', 'probe'],
  async apply(ctx) {
    const probe = ctx.use('probe')
    const calendar = ctx.use('calendar')
    await calendar.add({ title: 'js', day: 1 })
    const entry = await calendar.onDay(1)
    const days = await calendar.each([1, 2], (day) => `d${day}`)
    probe.record(`js caller: ${await calendar.caller()}`)
    try {
      await calendar.onDay(99)
    } catch (error) {
      probe.record(`js calendar: ${entry.title} ${days.join(',')} / ${error.name} ${error.code} ${error.message} ${error.data.Missing.day}`)
    }
    return () => probe.record('js calendar: bye')
  },
})
"#;

const PY_CALENDAR: &str = r#"
inject = ["calendar", "probe"]


async def apply(ctx, config):
    probe = ctx.use("probe")
    calendar = ctx.use("calendar")
    await calendar.add({"title": "py", "day": 2})
    entry = await calendar.onDay(2)
    probe.record(f"py caller: {await calendar.caller()}")
    try:
        await calendar.onDay(99)
    except Exception as error:
        probe.record(f"py calendar: {entry['title']} / {error.code} {error.data['Missing']['day']}")
"#;

/// Uses `calendar` without declaring it: found only through the
/// registration rows share once another row leases it, whose calls carry
/// no row; and `notes`, which nothing gives it.
const JS_SNEAK: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['probe'],
  async apply(ctx) {
    const probe = ctx.use('probe')
    try { ctx.use('notes') } catch (error) { probe.record(`sneak notes: ${error.message}`) }
    let calendar
    for (let tries = 0; calendar === undefined; tries++) {
      try { calendar = ctx.use('calendar') } catch (error) {
        if (tries > 400) throw error
        await new Promise(resolve => setTimeout(resolve, 25))
      }
    }
    try { await calendar.caller() } catch (error) { probe.record(`sneak caller: ${error.message}`) }
  },
})
"#;

const JS_HIDDEN: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['hidden'],
  apply(ctx) {},
})
"#;

const JS_NOTES: &str = r#"
import { definePlugin } from 'PLUGIN'
export default definePlugin({
  inject: ['notes', 'probe'],
  async apply(ctx) {
    const probe = ctx.use('probe')
    probe.record(`notes of ${await ctx.use('notes').owner()}`)
  },
})
"#;

/// The instance's owner, given when it is created.
struct Owner(String);

#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
struct NoConfig {}

/// The Rust row providing a `session` instance's own `Notes`: at the
/// native type's key in the instance, as an application does.
struct NotesRow {
    key: Key<Notes>,
    owner: String,
}

impl rutis::Plugin for NotesRow {
    type Deps = ();

    fn name(&self) -> &str {
        "notes"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.provide_at(
            self.key.clone(),
            Arc::new(Notes {
                owner: self.owner.clone(),
            }),
        )?;
        Ok(())
    }
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    probe: Probe,
    calendar: Option<rutis::Disposer>,
    js: PathBuf,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let py = dir.path().join("py");
    let js = dir.path().join("js");
    std::fs::create_dir_all(&py).unwrap();
    std::fs::create_dir_all(&js).unwrap();
    std::fs::write(py.join("py_calendar.py"), PY_CALENDAR).unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    let sdk = url::Url::from_file_path(&sdk).unwrap().to_string();
    for (file, source) in [
        ("js_calendar", JS_CALENDAR),
        ("js_notes", JS_NOTES),
        ("js_hidden", JS_HIDDEN),
        ("js_sneak", JS_SNEAK),
    ] {
        std::fs::write(
            js.join(format!("{file}.mjs")),
            source.replace("PLUGIN", &sdk),
        )
        .unwrap();
    }

    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    // Provided as usual; nothing else.
    let calendar = root.provide(Calendar::default()).unwrap();

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
    let mut builtins = Builtins::new();
    builtins.register_with("notes", |build, _: &NoConfig| {
        Ok(NotesRow {
            key: Key::of().instance(build.instance("session")?),
            owner: build
                .value::<Owner>()
                .map(|owner| owner.0.clone())
                .unwrap_or_default(),
        })
    });
    let plugin = LoaderPlugin::new(
        Chain::new()
            .with(builtins)
            .with_shared(go.clone())
            .with_shared(sessions.clone()),
        LoaderOptions {
            catalog,
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
    Fixture {
        root,
        loader,
        probe,
        calendar: Some(calendar),
        js,
        _dir: dir,
    }
}

impl Fixture {
    async fn expect(&self, line: &str) {
        Guard(HANG_GUARD * 3)
            .eventually_with(
                || self.probe.has(line).then_some(()),
                &format!("{line:?} to be recorded"),
                || {
                    let rows: Vec<String> = self
                        .loader
                        .entries()
                        .into_iter()
                        .map(|entry| format!("{}: {:?}", entry.id, entry.status))
                        .collect();
                    format!("recorded: {:?}\nrows: {rows:#?}", self.probe.lines())
                },
            )
            .await
    }

    fn js(&self, id: &str, file: &str) -> Value {
        json!({ "id": id, "name": self.js.join(format!("{file}.mjs")).to_string_lossy(), "config": {} })
    }
}

/// Rows of Node, Python and Go call one marked Rust service by name:
/// arguments and results cross as serde data, a callback calls back into
/// the caller, an error is thrown with its code and data.
/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn rows_of_every_language_call_a_marked_rust_service() {
    let mut fixture = fixture().await;
    let rows = vec![
        fixture.js("jc", "js_calendar"),
        json!({ "id": "pc", "name": "py:py_calendar", "config": {} }),
        json!({ "id": "gc", "name": "go:go_calendar", "config": {} }),
    ];
    let report = fixture.loader.reconcile(layer(rows), None).await.unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    fixture
        .expect("js calendar: js d1,d2 / CalendarError Missing no entry on day 99 99")
        .await;
    fixture.expect("py calendar: py / Missing 99").await;
    fixture
        .expect("go calendar: true go / CalendarError Missing no entry on day 99")
        .await;
    // A method taking `&Ctx` gets the calling row's.
    for line in ["js caller: jc", "py caller: pc", "go caller: gc"] {
        fixture.expect(line).await;
    }

    // The service goes with the native one, and its users stop first.
    fixture.calendar.take().unwrap().dispose().await.unwrap();
    fixture.expect("js calendar: bye").await;
    assert_eq!(row_state(&fixture.loader, "jc"), Some(FiberState::Pending));
    fixture.root.shutdown().await.unwrap();
}

/// Provided in an instance, a marked service is that instance's own: each
/// instance's rows get their own, with no instance group written anywhere.
/// guarantee: bridge/marked-services
#[tokio::test(flavor = "multi_thread")]
async fn a_marked_service_provided_in_an_instance_is_that_instances() {
    let fixture = fixture().await;
    let rows = vec![
        json!({ "id": "session", "group": true, "instanced": true, "config": [
        { "id": "notes", "name": "notes" },
        fixture.js("notes-js", "js_notes"),
    ] }),
    ];
    fixture.loader.reconcile(layer(rows), None).await.unwrap();
    for owner in ["A", "B"] {
        fixture
            .loader
            .create_instance(&fixture.root, "session")
            .with(Owner(owner.into()))
            .await
            .unwrap();
    }
    fixture.expect("notes of A").await;
    fixture.expect("notes of B").await;
    fixture.root.shutdown().await.unwrap();
}

/// A plugin using a service it does not declare in `inject` is told to
/// declare it: `use` of one nothing gives it, and a method taking `&Ctx`
/// of one it found through the registration rows share, whose calls carry
/// no row (#676).
/// guarantee: sdk/undeclared-use
#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_using_a_service_it_does_not_inject_is_told_to_declare_it() {
    let fixture = fixture().await;
    let rows = vec![
        fixture.js("jc", "js_calendar"),
        fixture.js("sneak", "js_sneak"),
    ];
    fixture.loader.reconcile(layer(rows), None).await.unwrap();
    fixture
        .expect("sneak notes: service notes is not available to this plugin, which does not declare it in inject: add 'notes' to its inject")
        .await;
    let said = Guard(HANG_GUARD * 3)
        .eventually_with(
            || {
                fixture
                    .probe
                    .lines()
                    .into_iter()
                    .find(|line| line.starts_with("sneak caller: "))
            },
            "the undeclared call to fail",
            || format!("{:?}", fixture.probe.lines()),
        )
        .await;
    assert!(
        said.ends_with(
            "caller needs the calling row's context, which a call carries only when the \
             calling plugin declares calendar in its inject: add 'calendar' to its inject"
        ),
        "{said}"
    );
    fixture.root.shutdown().await.unwrap();
}

/// A row waiting for a name that an unmarked Rust service would have is
/// told how to serve it.
#[tokio::test(flavor = "multi_thread")]
async fn waiting_for_an_unmarked_rust_service_says_how_to_serve_it() {
    let fixture = fixture().await;
    fixture.root.provide(Hidden).unwrap();
    let rows = vec![fixture.js("jh", "js_hidden")];
    fixture.loader.reconcile(layer(rows), None).await.unwrap();
    let entry = || {
        fixture
            .loader
            .entries()
            .into_iter()
            .find(|entry| entry.id == "jh")
            .unwrap()
    };
    // The row may first wait for its runtime (`node`) too, while that
    // starts (#530): what is asserted is the wait once only `hidden` is
    // missing, not the first list.
    let waits_for_hidden =
        |waits: &[Wait]| !waits.is_empty() && waits.iter().all(|wait| wait.service == "hidden");
    let waits = Guard(HANG_GUARD * 3)
        .eventually_with(
            || Some(fixture.loader.waits(&entry())).filter(|waits| waits_for_hidden(waits)),
            "jh to wait for hidden",
            || {
                format!(
                    "{:?}, waiting for {:?}",
                    entry(),
                    fixture.loader.waits(&entry())
                )
            },
        )
        .await;
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert!(
        matches!(
            &waits[0].reason,
            WaitReason::Explained(why)
                if why.starts_with("`hidden` is the Rust service")
                    && why.ends_with("add #[rutis::service] to its impl")
        ),
        "{waits:?}"
    );
    fixture.root.shutdown().await.unwrap();
}

/// Bun rows call it the same way.
#[cfg(all(unix, feature = "bun"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_bun_row_calls_a_marked_rust_service() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    std::fs::write(
        dir.path().join("calendar_user.ts"),
        JS_CALENDAR.replace("'PLUGIN'", &format!("{:?}", sdk.to_string_lossy())),
    )
    .unwrap();
    let probe = Probe::default();
    let root = Ctx::root().unwrap();
    root.provide_at(host_key("probe"), Arc::new(probe.clone()))
        .unwrap();
    root.provide(Calendar::default()).unwrap();
    let mut catalog = ServiceCatalog::new();
    catalog.register_shared("probe");
    greet_as_main(&root);
    let bun = TestRuntime::bun(dir.path());
    let sessions = Arc::new(SessionResolver::new(&catalog));
    sessions.runtime(bun.handle());
    root.plugin(bun).await.unwrap();
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(sessions.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    sessions.attach(&loader);
    root.plugin(plugin).await.unwrap();
    rows_stage(&root, &sessions, "bun").await;
    loader
        .reconcile(
            layer(vec![
                json!({ "id": "b", "name": "bun:./calendar_user.ts", "config": {} }),
            ]),
            None,
        )
        .await
        .unwrap();
    let line = "js calendar: js d1,d2 / CalendarError Missing no entry on day 99 99";
    Guard(HANG_GUARD * 3)
        .eventually_with(
            || probe.has(line).then_some(()),
            line,
            || format!("{:?} {:?}", probe.lines(), loader.entries()),
        )
        .await;
    root.shutdown().await.unwrap();
}
