//! A host service sees the row that calls it (`session::caller_row`, #441):
//! leased for one row (`Process::lease_host_for`), its calls name that row;
//! leased for all (`Process::lease_host`), and from Rust, they name none.
//! A runtime without `scopes` (older than every runtime rutis ships) takes
//! the lease shared, and its calls name no row.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::runtime::{Mount, Process};
use rutis_bridge::session::{caller_row, CallerRow, HostDispatch, Reply, Value};
use serde_json::json;

const REGISTRANT: &str = r#"
export const inject = ['ledger']
export function apply(ctx, config) { ctx.ledger.register(config.name) }
"#;

/// A calling row's id, key and scope.
type Caller = (String, String, Option<String>);

/// Each name registered, with the calling row, if any.
type Heard = Vec<(String, Option<Caller>)>;

/// `register(name)` records `name` with the calling row, if any.
#[derive(Clone, Default)]
struct Ledger(Arc<Mutex<Heard>>);

impl HostDispatch for Ledger {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        assert_eq!(method, "register");
        let [name]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        let row = caller_row().map(|row| {
            let scope = row.scope().map(str::to_owned);
            (row.id().to_owned(), row.key().to_owned(), scope)
        });
        self.0.lock().unwrap().push((name, row));
        Ok(Value::Undefined)
    }

    fn methods(&self) -> Option<serde_json::Value> {
        Some(json!({ "register": "sync" }))
    }
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// guarantee: bridge/caller-row
#[tokio::test(flavor = "multi_thread")]
async fn a_lease_for_a_row_names_it_and_a_shared_one_does_not() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let anchor = write(dir.path(), "package.json", "{}");
    let entry = write(dir.path(), "registrant.mjs", REGISTRANT);
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime");
    let process = Process::mount(
        &package,
        Mount {
            anchor: Some(&anchor),
            ..Mount::default()
        },
    )
    .await
    .unwrap();
    assert!(process.supports("scopes"));
    let ledger = Ledger::default();
    let dispatch: Arc<dyn HostDispatch> = Arc::new(ledger.clone());
    let root = Ctx::root().unwrap();

    // Two rows, each with the service to itself; one of them in a scope.
    let mut leases = Vec::new();
    for (key, scope) in [("a.1", None), ("b.1", Some("team"))] {
        let row = CallerRow::new(key.split('.').next().unwrap(), key, root.clone());
        let lease = process
            .lease_host_for("ledger", scope, &row, dispatch.clone(), None)
            .await
            .unwrap();
        let label = lease.label().unwrap().to_owned();
        assert_ne!(Some(label.as_str()), scope, "a scope of the row's own");
        let isolate = [("ledger".to_owned(), label)];
        let name = key.split('.').next().unwrap();
        process
            .load_row(key, &entry, json!({ "name": name }), &isolate, &[], &[])
            .await
            .unwrap();
        leases.push(lease);
    }
    // One with the service shared, as before.
    let shared = process
        .lease_host("ledger", dispatch.clone(), None)
        .await
        .unwrap();
    process
        .load_row("c.1", &entry, json!({ "name": "c" }), &[], &[], &[])
        .await
        .unwrap();
    // And a call from Rust.
    dispatch.invoke("register", json!(["rust"]).into()).unwrap();

    let heard = ledger.0.lock().unwrap().clone();
    let of = |name: &str| {
        heard
            .iter()
            .find(|(registered, _)| registered == name)
            .map(|(_, row)| row.clone())
            .unwrap_or_else(|| panic!("{name} registered: {heard:?}"))
    };
    assert_eq!(of("a"), Some(("a".into(), "a.1".into(), None)));
    assert_eq!(
        of("b"),
        Some(("b".into(), "b.1".into(), Some("team".into())))
    );
    assert_eq!(of("c"), None);
    assert_eq!(of("rust"), None);

    for key in ["a.1", "b.1", "c.1"] {
        process.unload_row(key).await.unwrap();
    }
    for lease in leases {
        lease.release().await.unwrap();
    }
    shared.release().await.unwrap();
    process.dispose().await.unwrap();
}
