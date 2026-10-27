// A listener that returns a result instead of a future of one.
use rutis::{Ctx, Plugin};

#[derive(rutis::serde::Serialize, rutis::serde::Deserialize)]
#[serde(crate = "rutis::serde")]
struct Ping(u32);
impl rutis::Event for Ping {
    const NAME: &'static str = "ping";
    type Value = u32;
}

struct Listens;
impl Plugin for Listens {
    type Deps = ();
    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.on::<Ping>(|event, _call| Ok(Some(event.0)))?;
        Ok(())
    }
}

fn main() {}
