//! `Loader::settled` waits for what a change sets going in the kernel too:
//! a row that restarts because a row it uses restarted, after `settled`
//! may already have waited for it (#789: `rutis-host dev` said a row
//! stopped that was on its way back).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, PluginFactory, TypeKey};
use rutis::{Ctx, Error};
use rutis_loader::{Builtins, LoaderOptions, LoaderPlugin};
use serde_json::{json, Value};

struct A;
struct B;

/// `a` provides `A`; `b` uses `A` and takes a while before it provides
/// `B`; `c` uses `B`, and takes a while too.
struct Chain {
    name: &'static str,
    log: Log,
}

impl DynPlugin for Chain {
    fn name(&self) -> &str {
        self.name
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            match self.name {
                "a" => {
                    ctx.provide(A)?;
                }
                "b" => {
                    ctx.require::<A>()?;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    ctx.provide(B)?;
                }
                _ => {
                    ctx.require::<B>()?;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            self.log
                .lock()
                .unwrap()
                .push(format!("apply {}", self.name));
            Ok(Effect::Done)
        })
    }
}

struct ChainFactory {
    name: &'static str,
    injects: Vec<TypeKey>,
    log: Log,
}

impl PluginFactory<Value> for ChainFactory {
    fn name(&self) -> &str {
        self.name
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.clone()
    }

    fn build(&self, _: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Chain {
            name: self.name,
            log: self.log.clone(),
        }))
    }
}

#[tokio::test]
async fn settled_waits_for_the_rows_a_restart_restarts() {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let mut builtins = Builtins::new();
    for (name, injects) in [
        ("a", vec![]),
        ("b", vec![TypeKey::of::<A>()]),
        ("c", vec![TypeKey::of::<B>()]),
    ] {
        builtins.register_raw(
            name,
            ChainFactory {
                name,
                injects,
                log: log.clone(),
            },
            None,
        );
    }
    let store = MemStore::default();
    let root = Ctx::root().unwrap();
    let plugin = LoaderPlugin::new(
        builtins,
        LoaderOptions {
            persist: Arc::new(store.clone()),
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    reconcile(
        &loader,
        patches(json!([{ "insert": [
            // The rows in the order the loader waits for them: the one
            // using the others first, as `caller` before `weather` in dev.
            { "id": "x", "name": "c" },
            { "id": "y", "name": "b" },
            { "id": "z", "name": "a" }
        ] }])),
        &store,
    )
    .await;
    loader.settled().await;
    let applies = |name: &str| {
        log.lock()
            .unwrap()
            .iter()
            .filter(|line| **line == format!("apply {name}"))
            .count()
    };
    assert_eq!((applies("a"), applies("b"), applies("c")), (1, 1, 1));

    // The loader waits for its rows in no set order: each round may wait
    // for x before y has given it what it uses.
    for round in 2..=6 {
        loader.restart("z").await.unwrap();
        loader.settled().await;
        assert_eq!(
            state(&loader, "x"),
            Some(FiberState::Active),
            "round {round}"
        );
        assert_eq!(
            (applies("a"), applies("b"), applies("c")),
            (round, round, round)
        );
    }
}
