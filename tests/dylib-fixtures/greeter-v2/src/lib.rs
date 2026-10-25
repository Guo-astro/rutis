#![cfg(feature = "export")]
// `changed_identity` and `fail_once` are exclusive versions of this plugin:
// with both on (`--all-features`), nothing is exported.
#![cfg_attr(
    all(feature = "changed_identity", feature = "fail_once"),
    allow(dead_code, reason = "exclusive versions: nothing is exported")
)]

use rutis_sdk::rutis::host::{
    BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, FiberState, PluginFactory, Snapshot, TypeKey,
};
use rutis_sdk::rutis::{Ctx, Error};
use rutis_sdk::ConfigValue;

struct Factory;
/// `scope`: provide everything in this isolation scope (`{"scope": "b"}`),
/// for a second build to run beside the first.
struct Greeter {
    scope: Option<String>,
}

/// Served by name to the plugins of other languages once provided (#510).
///
/// Built with `RUTIS_GREETER_REBUILT` set (tools/test-dylib.sh), this is
/// another build of the plugin under the same version, features and path,
/// so `Greeting` keeps its `TypeId`; it lays the value out differently and
/// answers differently, to show each build's values run that build's code.
pub struct Greeting(&'static str, [u64; EXTRA]);

const REBUILT: bool = option_env!("RUTIS_GREETER_REBUILT").is_some();
const EXTRA: usize = if REBUILT { 3 } else { 0 };

fn greeting() -> Greeting {
    Greeting("hello v2", [7; EXTRA])
}

#[rutis_sdk::rutis::service]
impl Greeting {
    pub fn text(&self) -> String {
        if REBUILT {
            format!("{} rebuilt {}", self.0, self.1.iter().sum::<u64>())
        } else {
            self.0.to_owned()
        }
    }
}

impl PluginFactory<ConfigValue> for Factory {
    fn name(&self) -> &str {
        #[cfg(feature = "changed_identity")]
        return "other-greeter";
        #[cfg(not(feature = "changed_identity"))]
        "greeter"
    }
    fn injects(&self) -> Vec<TypeKey> {
        #[cfg(feature = "changed_identity")]
        return vec![TypeKey::of::<u8>()];
        #[cfg(not(feature = "changed_identity"))]
        Vec::new()
    }
    fn build(&self, config: &ConfigValue) -> Result<Box<dyn DynPlugin>, Error> {
        Ok(Box::new(Greeter {
            scope: config["scope"].as_str().map(str::to_owned),
        }))
    }
}
impl DynPlugin for Greeter {
    fn name(&self) -> &str {
        "greeter"
    }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            rutis_sdk::tokio::spawn(async {})
                .await
                .map_err(|e| rutis_sdk::rutis::Error::from(ErrorKind::PluginFailed(e.into())))?;
            let ctx = match &self.scope {
                Some(scope) => ctx
                    .isolate(TypeKey::of::<String>(), scope)
                    .isolate(TypeKey::of::<Greeting>(), scope)
                    .isolate(TypeKey::of::<Snapshot>(), scope),
                None => ctx.clone(),
            };
            ctx.provide("hello v2".to_string())?;
            ctx.provide(greeting())?;
            ctx.provide(Snapshot {
                generation: 2,
                state: FiberState::Active,
                error: None,
            })?;
            Ok(Effect::Done)
        })
    }
}
#[cfg(all(
    feature = "export",
    not(any(feature = "changed_identity", feature = "fail_once"))
))]
rutis_sdk::export_plugin! {
    id: "greeter",
    factory: Factory,
    schema: rutis_sdk::serde_json::json!({ "type": "object", "properties": { "greeting": { "type": "string" } } }),
}

#[cfg(all(
    feature = "export",
    feature = "changed_identity",
    not(feature = "fail_once")
))]
rutis_sdk::export_plugin! { id: "other-greeter", factory: Factory }

#[cfg(all(
    feature = "export",
    feature = "fail_once",
    not(feature = "changed_identity")
))]
rutis_sdk::export_plugin! { id: "greeter", factory: factory_once() }

#[cfg(feature = "fail_once")]
fn factory_once() -> Factory {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
    if ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
        panic!("first factory entry attempt fails");
    }
    Factory
}
