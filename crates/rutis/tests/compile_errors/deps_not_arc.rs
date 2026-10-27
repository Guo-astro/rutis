// A dependency written as the service type itself instead of `Arc<T>`.
use std::sync::Arc;

use rutis::{Ctx, Plugin};

struct Db;
struct Log;

struct Uses;
impl Plugin for Uses {
    type Deps = (Arc<Log>, Db);
    async fn apply(&self, _: &Ctx, _deps: Self::Deps) -> rutis::Result {
        Ok(())
    }
}

fn main() {}
