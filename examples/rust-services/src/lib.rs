//! An application whose rows use, by name, the Rust services of a crate it
//! cannot change: [`rust_services_calendar`] knows nothing of rutis-bridge
//! or of other languages. Rust allows an `impl` block only in the crate
//! that defines the type, so the application wraps each service in a type
//! of its own, marked `#[rutis::service]`, whose methods call the wrapped
//! one: a [`Calendar`] for every row and a [`Notes`] per `session`
//! instance. Nothing here implements `HostDispatch`; `build.rs` only keeps
//! the TypeScript declarations current.
//!
//! ```ignore
//! root.provide(Calendar(rust_services_calendar::Calendar::new()))?;   // rows: ctx.use('calendar')
//! ```

use rust_services_calendar::{CalendarError, Change, Entry, Handler, NewEntry};
use rutis::host::BoxFuture;
use rutis::Ctx;

/// The calendar: entries, and subscribers told of each change.
pub struct Calendar(pub rust_services_calendar::Calendar);

#[rutis::service]
impl Calendar {
    /// Add an entry; returns it with its id.
    pub fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> {
        self.0.add(entry)
    }

    /// The entry `id`.
    pub async fn get(&self, id: u64) -> Result<Entry, CalendarError> {
        self.0.get(id).await
    }

    /// The entries, those with `tag` only when it is given.
    pub async fn list(&self, tag: Option<String>) -> Vec<Entry> {
        self.0.list(tag).await
    }

    pub fn rename(&self, id: u64, title: &str) -> Result<Entry, CalendarError> {
        self.0.rename(id, title)
    }

    pub fn remove(&self, id: u64) -> Result<(), CalendarError> {
        self.0.remove(id)
    }

    /// The largest id the calendar can give.
    pub fn last_id(&self) -> u64 {
        self.0.last_id()
    }

    /// Tell `handler` of every change from now on, until `ctx`'s plugin
    /// unloads or [`Calendar::unsubscribe`]; returns the subscription.
    pub fn subscribe(&self, ctx: &Ctx, handler: Handler) -> rutis::Result<u64> {
        self.0.subscribe(ctx, handler)
    }

    pub fn unsubscribe(&self, subscription: u64) -> bool {
        self.0.unsubscribe(subscription)
    }

    /// Visit every entry in order, each visit finishing before the next;
    /// returns how many were visited.
    pub async fn each(
        &self,
        visit: impl Fn(Entry) -> BoxFuture<'static, rutis::Result> + Send + Sync,
    ) -> rutis::Result<usize> {
        self.0.each(visit).await
    }

    /// Hand `change` to every subscriber in turn; done once each of them
    /// handled it, failed when one of them failed.
    pub async fn publish(&self, change: Change) -> rutis::Result {
        self.0.publish(change).await
    }

    /// How many subscribers there are.
    pub fn subscribers(&self) -> usize {
        self.0.subscribers()
    }
}

/// One session's notes: a service each instance of a session has its own
/// of.
pub struct Notes(pub rust_services_calendar::Notes);

#[rutis::service]
impl Notes {
    /// Whose notes these are.
    pub fn owner(&self) -> String {
        self.0.owner()
    }

    pub fn write(&self, line: String) -> usize {
        self.0.write(line)
    }

    pub fn read(&self) -> Vec<String> {
        self.0.read()
    }
}
