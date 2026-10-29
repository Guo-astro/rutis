//! Language runtimes: processes that run plugins written in another
//! language, whose plugins rutis-loader manages as rows. How each
//! language's process starts is the application's (rutis-host's
//! `rutis-host/spawn`); a [`Launcher`] says it.
//!
//! A runtime is the far end of a session ([`crate::Session`]): on this
//! machine its process is started with [`Launcher::start`], elsewhere it is
//! reached over a connection. [`RuntimeStates`] keeps its state for those
//! that watch it ([`RuntimeHandle`]).

// Inside the crate, runtime code names session items through this module.
#[allow(unused_imports)]
pub(crate) use crate::session::*;

mod calls;
mod definitions;
mod process;
mod projection;
mod rows;
mod state;
#[cfg(feature = "testing")]
pub mod testing;

/// Where a runtime process's standard streams go ([`Launcher::stdin`],
/// [`Launcher::stdout`]).
pub use crate::pipe::Stdio;
pub use calls::InFlight;
pub(crate) use definitions::read as read_definition;
pub use definitions::{mounter_key, returned, ForeignPlugin, PluginMounter};
pub(crate) use process::calling as calling_process;
pub use process::{
    scoped_id, Host, HostLease, Launcher, Mount, PluginSchema, Process, RowSchema, ServiceEvents,
};
pub use projection::Projection;
pub use rows::{row_projection, row_projection_of, row_projection_with, RowService};
pub use state::{ended_status, RuntimeHandle, RuntimeState, RuntimeStates};

use std::sync::Arc;

use crate::session::{Connection, Dispatch, Error};

/// A session with a runtime that something else owns (a link's peer, a
/// session plugin's connection): a [`Process`] runs over it.
pub(crate) trait Carried: Send + Sync + 'static {
    /// The session, ready.
    fn connection(&self) -> Connection;

    /// Route the runtime's calls into rutis (`host:<name>`, `service`,
    /// `events.*`, `rows.ended`) to `dispatch` while the returned guard lives.
    fn route(
        &self,
        dispatch: Arc<dyn Dispatch>,
    ) -> Result<Box<dyn std::any::Any + Send + Sync>, Error>;
}

/// The families a runtime calls rutis with.
const FAMILIES: [&str; 4] = ["host", "service", "events", "rows"];

/// `peer` as a session a process runs over.
pub(crate) fn peer_session(peer: Arc<crate::peer::Peer>) -> Arc<dyn Carried> {
    Arc::new(PeerSession(peer))
}

struct PeerSession(Arc<crate::peer::Peer>);

/// A dispatch as a peer's handler.
pub(crate) struct Routed(pub(crate) Arc<dyn Dispatch>);

impl crate::peer::Handler for Routed {
    fn invoke(
        &self,
        peer: &Connection,
        target: &str,
        method: &str,
        args: crate::session::Value,
    ) -> crate::session::Reply {
        self.0.invoke(peer, target, method, args)
    }

    fn arrived(&self, peer: &Connection, method: &str) {
        self.0.arrived(peer, method)
    }

    fn replied(&self, peer: &Connection, method: &str) {
        self.0.replied(peer, method)
    }
}

impl Carried for PeerSession {
    fn connection(&self) -> Connection {
        self.0.connection().clone()
    }

    fn route(
        &self,
        dispatch: Arc<dyn Dispatch>,
    ) -> Result<Box<dyn std::any::Any + Send + Sync>, Error> {
        let handler: Arc<dyn crate::peer::Handler> = Arc::new(Routed(dispatch));
        let offered = FAMILIES
            .iter()
            .map(|family| self.0.register(family, handler.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::new(offered))
    }
}
