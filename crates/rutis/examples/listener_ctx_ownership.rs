//! The registration context owns the listener, and the callback is called
//! with that same context (not the emitter's), so what it registers belongs
//! to the listener's plugin. Run with `cargo run -p rutis --example listener_ctx_ownership`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rutis::host::CtxHost;
use rutis::{Ctx, Event, Plugin};

#[derive(serde::Serialize, serde::Deserialize)]
struct Tick;

impl Event for Tick {
    const NAME: &'static str = "example::Tick";
    type Value = ();
}

struct Register(Arc<AtomicUsize>);

impl Plugin for Register {
    type Deps = ();

    fn name(&self) -> &str {
        "register-listener"
    }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        let owner = ctx.instance();
        let cleaned = self.0.clone();
        ctx.on::<Tick>(move |_, call| {
            let cleaned = cleaned.clone();
            async move {
                // The registering context, whoever emitted.
                let ctx = call.ctx();
                assert_eq!(ctx.instance(), owner);
                ctx.effect(|| async move {
                    cleaned.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })?;
                Ok(None)
            }
        })?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let cleaned = Arc::new(AtomicUsize::new(0));
    let plugin = root.plugin(Register(cleaned.clone()));
    (&plugin).await?;
    root.serial(Tick).await?;
    plugin.shutdown().await?;
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    root.shutdown().await?;
    Ok(())
}
