//! Rewrite a candidate while the caller holds a std Mutex, then commit the
//! checked result. Run with `cargo run -p rutis --example sync_decision`.

use std::sync::Mutex;

use rutis::host::{CtxHost, ErrorKind};
use rutis::{Ctx, Error, Event};

#[derive(serde::Serialize, serde::Deserialize)]
struct BeforeSave {
    candidate: u64,
}
impl Event for BeforeSave {
    const NAME: &'static str = "before-save";
    type Value = u64;
}

fn save(ctx: &Ctx, state: &Mutex<u64>, candidate: u64) -> rutis::Result<u64> {
    let mut current = state.lock().unwrap();
    let proposed = ctx
        .waterfall_sync(BeforeSave { candidate }, |event| {
            Ok(Some((*current).max(event.candidate)))
        })?
        .unwrap_or(*current);
    // Middleware only proposes a value; the caller validates before writing.
    if proposed < *current {
        return Err(Error::from(ErrorKind::Validation {
            issues: vec!["saved value must not decrease".into()],
        }));
    }
    *current = proposed;
    Ok(proposed)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    root.on_sync::<BeforeSave>(|_, call| {
        // Fixed input: rewrite the downstream result as it returns.
        Ok(call.next()?.map(|value| value.min(100)))
    })?;
    let state = Mutex::new(10);
    assert_eq!(save(&root, &state, 120)?, 100);
    println!("saved {}", state.lock().unwrap());
    root.shutdown().await?;
    Ok(())
}
