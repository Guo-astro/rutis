// Listening to a type that is not an event.
use rutis::{Ctx, Plugin};

struct Ping(u32);

struct Listens;
impl Plugin for Listens {
    type Deps = ();
    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.on::<Ping>(|_event, _call| async move { Ok(None) })?;
        Ok(())
    }
}

fn main() {}
