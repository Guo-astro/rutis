//! Runtime access: a peer that is a language runtime, as the
//! `RuntimeSession` a [`RuntimePlugin::remote`](crate::runtime::RuntimePlugin::remote)
//! runs its rows on.
//!
//! The runtime's calls into rutis (`host:<name>`, `service`, `events.*`,
//! `rows.ended`) are families on the peer, routed to whatever the runtime
//! plugin serves them with. The session stays the link's: the runtime
//! plugin neither opens nor closes it.

use std::sync::Arc;

use crate::channel::PeerId;
use crate::runtime::rpc::{Connection, Dispatch, Reply, Value};
use crate::runtime::{runtime_session_key, Error, RuntimeSession};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, ErrorKind, TypeKey};
use rutis::internal::CtxInternal;
use rutis::{Ctx, Error as RutisError};

use crate::{peer_key, Peer};

/// The families a runtime calls rutis with.
const FAMILIES: [&str; 4] = ["host", "service", "events", "rows"];

/// Provides `RuntimeSession#<runtime>` on `Peer#<peer>`.
pub struct RuntimeAccessPlugin {
    label: String,
    runtime: String,
    injects: [TypeKey; 1],
}

impl RuntimeAccessPlugin {
    /// The runtime instance `runtime` is the far end `peer`. The link to it
    /// should require the `runtime` contract.
    pub fn new(peer: PeerId, runtime: &str) -> Self {
        Self {
            label: format!("rutis-bridge/runtime#{runtime}"),
            runtime: runtime.to_owned(),
            injects: [peer_key(&peer).erase()],
        }
    }
}

impl DynPlugin for RuntimeAccessPlugin {
    fn platform(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.label
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, RutisError>> {
        Box::pin(async move {
            let peer = ctx
                .get_key::<Peer>(self.injects[0].clone())
                .ok_or_else(|| {
                    rutis::Error::from(ErrorKind::PluginFailed(
                        format!("{}: the peer is gone", self.label).into(),
                    ))
                })?;
            ctx.provide_at(runtime_session_key(&self.runtime), peer_session(peer))?;
            Ok(Effect::Done)
        })
    }
}

/// `peer` as a runtime session.
pub(crate) fn peer_session(peer: Arc<Peer>) -> Arc<dyn RuntimeSession> {
    Arc::new(PeerSession(peer))
}

struct PeerSession(Arc<Peer>);

struct Routed(Arc<dyn Dispatch>);
impl crate::Handler for Routed {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        self.0.invoke(peer, target, method, args)
    }

    fn arrived(&self, peer: &Connection, method: &str) {
        self.0.arrived(peer, method)
    }

    fn replied(&self, peer: &Connection, method: &str) {
        self.0.replied(peer, method)
    }
}

impl RuntimeSession for PeerSession {
    fn connection(&self) -> Connection {
        self.0.connection().clone()
    }

    fn route(
        &self,
        dispatch: Arc<dyn Dispatch>,
    ) -> Result<Box<dyn std::any::Any + Send + Sync>, Error> {
        let handler: Arc<dyn crate::Handler> = Arc::new(Routed(dispatch));
        let offered = FAMILIES
            .iter()
            .map(|family| self.0.register(family, handler.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::new(offered))
    }
}
