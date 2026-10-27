// Emitting a value whose type is not an event.
use rutis::{Ctx, Plugin};

struct Ping(u32);

struct Pinger;
impl Plugin for Pinger {
    type Deps = ();
    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.emit(Ping(1))?;
        Ok(())
    }
}

fn main() {}
