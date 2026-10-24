//! rutis-migrate on small 0.16 sources: what each rewrite produces, and
//! what it must leave alone.

use rutis_migrate::migrate;

fn out(source: &str) -> String {
    migrate(source).source
}

#[test]
fn the_object_safe_plugin_becomes_dyn_plugin() {
    let source = r#"
use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin, TypeKey};

struct P(Vec<TypeKey>);

impl Plugin for P {
    fn name(&self) -> &str { "p" }
    fn injects(&self) -> &[TypeKey] { &self.0 }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move { Ok(Effect::Done) })
    }
}
"#;
    let new = out(source);
    assert!(new.contains("impl DynPlugin for P"), "{new}");
    assert!(new.contains("fn injects(&self) -> Vec<TypeKey>"), "{new}");
    assert!(new.contains("self.0.to_vec()"), "{new}");
    assert!(new.contains("use rutis::host::"), "{new}");
    assert!(!new.contains("CordisError"), "{new}");
}

#[test]
fn shared_errors_become_arc_error_kind() {
    let source = r#"
use std::sync::Arc;
use rutis::CordisError;

fn sink(errors: &mut Vec<Arc<CordisError>>, error: CordisError) {
    errors.push(Arc::new(CordisError::Closed));
    let _ = error;
}
"#;
    let new = out(source);
    assert!(new.contains("Vec<Arc<ErrorKind>>"), "{new}");
    assert!(new.contains("Arc::new(ErrorKind::Closed)"), "{new}");
}

#[test]
fn std_io_error_kinds_are_left_alone() {
    let source = r#"
use rutis::CordisError;

fn refuse() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::AddrInUse, "taken")
}
fn closed() -> Result<(), CordisError> {
    Err(CordisError::Closed)
}
"#;
    let new = out(source);
    assert!(
        new.contains("std::io::Error::new(std::io::ErrorKind::AddrInUse"),
        "{new}"
    );
    assert!(!new.contains("Error::from(std::io::ErrorKind"), "{new}");
    assert!(new.contains("Error::from(ErrorKind::Closed)"), "{new}");
}

#[test]
fn another_crates_plugin_type_keeps_its_name() {
    let source = r#"
use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin};

struct Mine;
impl Plugin for Mine {
    fn name(&self) -> &str { "mine" }
    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}

fn mount(ctx: &Ctx) {
    ctx.plugin(bindings::Plugin::new(bindings::Config::default()));
}
"#;
    let new = out(source);
    assert!(new.contains("impl DynPlugin for Mine"), "{new}");
    assert!(new.contains("bindings::Plugin::new"), "{new}");
}

#[test]
fn a_dispatch_under_a_dynamic_name_keeps_the_name() {
    let source = r#"
use std::sync::Arc;
use rutis::{Ctx, EventKey};

fn send(ctx: &Ctx, i: u32) {
    ctx.events()
        .emit::<Ping>(ctx, &rutis::EventKey::dynamic(format!("branch/{i}")), Arc::new(Ping))
        .unwrap();
}
"#;
    let new = out(source);
    assert!(
        new.contains(r#"ctx.named(format!("branch/{i}")).emit("#),
        "{new}"
    );
    assert!(new.contains("CtxInternal"), "{new}");
}

#[test]
fn a_dispatch_under_a_key_from_elsewhere_uses_its_name() {
    let source = r#"
use std::sync::Arc;
use rutis::Ctx;

async fn send(ctx: &Ctx) {
    ctx.events().parallel(ctx, &node_event("tick"), Arc::new(Args(vec![]))).await.unwrap();
}
"#;
    let new = out(source);
    assert!(
        new.contains(r#"ctx.named(node_event("tick").name()).parallel("#),
        "{new}"
    );
}

#[test]
fn a_cleanup_effect_becomes_a_future() {
    let source = r#"
use std::sync::Arc;
use rutis::{Ctx, Effect};

fn register(ctx: &Ctx, release: Arc<tokio::sync::Semaphore>) {
    ctx.effect({
        let release = release.clone();
        move || {
            Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    release.close();
                    Ok(())
                })
            }))
        }
    })
    .unwrap();
}
"#;
    let new = out(source);
    assert!(!new.contains("AsyncDisposer"), "{new}");
    assert!(new.contains("async move"), "{new}");
    assert!(!new.contains("async move { async move"), "{new}");
}

#[test]
fn a_keyed_provide_keeps_its_service_type() {
    let source = r#"
use std::sync::Arc;
use rutis::{Ctx, TypeKey};

fn provide(ctx: &Ctx, key: TypeKey) {
    ctx.provide_as::<dyn Tool>(key, Arc::new(Hammer)).unwrap();
}
"#;
    let new = out(source);
    assert!(
        new.contains("provide_at::<dyn Tool>(key, Arc::new(Hammer))"),
        "{new}"
    );
}

#[test]
fn register_fn_becomes_register_with_its_config_type() {
    let source = r#"
use rutis_loader::Builtins;
use serde_json::Value;

fn builtins() -> Builtins {
    let mut builtins = Builtins::new();
    builtins.register_fn::<Value, _>("echo", |_| Ok(Box::new(Echo)));
    builtins
}
"#;
    let new = out(source);
    assert!(
        new.contains(r#"builtins.register::<Value, _, _>("echo", |_| Ok(Echo))"#),
        "{new}"
    );
}

#[test]
fn rutis_loader_names_move_to_their_modules() {
    let source = r#"
use rutis::Ctx;
use rutis_loader::{Builtins, Chain, Edit, ReconcileReport, RuntimeResolver, LoaderPlugin};

fn f(report: rutis_loader::CheckReport) {}
"#;
    let new = out(source);
    assert!(
        new.contains("use rutis_loader::{Builtins, Chain, LoaderPlugin};"),
        "{new}"
    );
    assert!(new.contains("use rutis_loader::edit::Edit;"), "{new}");
    assert!(
        new.contains("use rutis_loader::report::ReconcileReport;"),
        "{new}"
    );
    assert!(
        new.contains("use rutis_loader::runtime::RuntimeResolver;"),
        "{new}"
    );
    assert!(new.contains("rutis_loader::report::CheckReport"), "{new}");
}

#[test]
fn a_file_in_the_new_style_is_left_alone() {
    let source = r#"
use rutis::{Ctx, Plugin};

struct Greeter;
impl Plugin for Greeter {
    type Deps = ();
    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        Ok(())
    }
}
"#;
    assert_eq!(out(source), source);
}

#[test]
fn imports_inside_inline_modules_move_too() {
    let source = r#"
mod imp {
    use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin, Snapshot};
}
"#;
    let new = out(source);
    assert!(new.contains("    use rutis::host::"), "{new}");
    assert!(new.contains("Snapshot"), "{new}");
    assert!(!new.contains("rutis::{BoxFuture"), "{new}");
}

#[test]
fn a_dispatch_bound_to_an_unused_name_stays() {
    let source = r#"
use std::sync::Arc;
use rutis::{Ctx, EventKey};

async fn send(ctx: &Ctx) -> Result<(), rutis::CordisError> {
    let key = EventKey::<Ping>::of();
    let first = ctx.events().serial(ctx, &EventKey::of(), Arc::new(Ping)).await?;
    Ok(())
}
"#;
    let new = out(source);
    assert!(
        new.contains("let first = ctx.serial(Ping).await?;"),
        "{new}"
    );
    assert!(!new.contains("let key ="), "{new}");
    assert!(
        !new.contains("EventKey"),
        "the key's import goes too: {new}"
    );
}

#[test]
fn a_glob_import_brings_the_host_layer() {
    let source = r#"
use rutis::*;

struct P;
impl Plugin for P {
    fn name(&self) -> &str { "p" }
    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async { Ok(Effect::Done) })
    }
}
"#;
    let new = out(source);
    assert!(new.contains("impl DynPlugin for P"), "{new}");
    assert!(new.contains("use rutis::host::*;"), "{new}");
}
