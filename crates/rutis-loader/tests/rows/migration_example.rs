//! The example of docs/migration-0.6.0-to-0.6.1.md, kept compiling and
//! running: rows from a layer, then edits through the editable layer.

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_loader::edit::{Editable, Version};
use rutis_loader::{Builtins, Layer, LoaderOptions, LoaderPlugin, Patch};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, schemars::JsonSchema)]
struct MyConfig {
    #[allow(dead_code, reason = "the example shows the schema, not its use")]
    level: u32,
}

struct MyPlugin;

impl MyPlugin {
    fn new(_config: &MyConfig) -> rutis::Result<Self> {
        Ok(MyPlugin)
    }
}

impl rutis::Plugin for MyPlugin {
    type Deps = ();

    fn name(&self) -> &str {
        "my-plugin"
    }

    async fn apply(&self, _ctx: &Ctx, (): ()) -> rutis::Result {
        Ok(())
    }
}

#[tokio::test]
async fn the_migration_example_runs() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;

    let mut builtins = Builtins::new();
    builtins.register("my-plugin", MyPlugin::new);
    let plugin = LoaderPlugin::new(builtins, LoaderOptions::default());
    let loader = plugin.handle();
    root.plugin(plugin).await?;

    let app: Vec<Patch> = serde_json::from_value(json!([
        { "insert": [{ "id": "main", "name": "my-plugin", "config": { "level": 1 } }] }
    ]))?;
    loader
        .reconcile(
            vec![Layer::new("app", app), Layer::new("user", Vec::new())],
            Some(Editable::new("user", Version::default())),
        )
        .await?;
    loader.update("main", json!({ "level": 2 })).await?;
    loader.set_disabled("main", true).await?;

    assert_eq!(
        loader.layers()[1].patches.len(),
        1,
        "the edit went to the user layer"
    );
    assert_eq!(
        loader.layers()[0].patches.len(),
        1,
        "the app layer is untouched"
    );
    Ok(())
}
