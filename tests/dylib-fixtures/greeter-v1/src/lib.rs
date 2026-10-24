#![cfg(feature = "export")]

//! The plugin as its author writes it: a `Plugin`, exported with its
//! constructor (`build:`). greeter-v2 exports a `PluginFactory` instead.

use rutis_sdk::rutis::host::{FiberState, Snapshot};
use rutis_sdk::rutis::{Ctx, Plugin};
use rutis_sdk::ConfigValue;

struct Greeter;

/// Served by name to the plugins of other languages once provided (#510).
pub struct Greeting(&'static str);

#[rutis_sdk::rutis::service]
impl Greeting {
    pub fn text(&self) -> String {
        self.0.to_owned()
    }
}

impl Drop for Greeter {
    fn drop(&mut self) {
        if let Some(path) = std::env::var_os("RUTIS_PLUGIN_DROP_MARKER") {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("open plugin drop marker");
            writeln!(file, "drop").expect("write plugin drop marker");
        }
    }
}

impl Plugin for Greeter {
    type Deps = ();

    fn name(&self) -> &str {
        "greeter"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis_sdk::rutis::Result {
        rutis_sdk::tokio::spawn(async {}).await?;
        ctx.provide("hello v1".to_string())?;
        ctx.provide(Greeting("hello v1"))?;
        ctx.provide(Snapshot {
            generation: 1,
            state: FiberState::Active,
            error: None,
        })?;
        Ok(())
    }
}

fn greeter(_: &ConfigValue) -> rutis_sdk::rutis::Result<Greeter> {
    Ok(Greeter)
}
#[cfg(feature = "export")]
rutis_sdk::export_plugin! { id: "greeter", build: greeter }

#[cfg(feature = "export")]
#[used]
#[cfg_attr(target_os = "linux", link_section = ".init_array")]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
// Run by the CRT from DllMain, before LoadLibraryExW returns.
#[cfg_attr(windows, link_section = ".CRT$XCU")]
static INIT_MARKER: extern "C" fn() = mark_load;

#[cfg(feature = "export")]
extern "C" fn mark_load() {
    if let Some(path) = std::env::var_os("RUTIS_PLUGIN_INIT_MARKER") {
        let _ = std::fs::write(path, b"plugin initializer executed");
    }
}
