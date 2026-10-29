//! A ready session with one far end, inside rutis-bridge: what a
//! [`Session`](crate::Session) and what runs on it (a far end's process,
//! a far kernel's rows and events) use.
//!
//! The session is its owner's; what runs on it is registered here by
//! family. A call from the far end goes to the handler of its family:
//! the part of a control method before its first `.` (`rows.load` →
//! `rows`), the bare control method (`service`), or the part of a target
//! before its first `:` (`host:clock` → `host`). A family nobody registered
//! answers with an error, and the session goes on. The families registered
//! here are announced to the far end as `link.offers`; what the far end
//! offers is observable.
//!
//! Each family offered carries the version at which it was registered
//! (`since`): a family withdrawn and registered again between two looks is
//! a new offer, which a follower must not mistake for the old one.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use crate::channel::PeerId;
use crate::session::rpc::{Connection, Dispatch, Reply, Value};
use crate::session::Error;
use rutis::host::Key;
use serde_json::json;
use tokio::sync::watch;

/// Serves one family of operations on a peer's session.
pub(crate) trait Handler: Send + Sync + 'static {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply;
}

impl<F> Handler for F
where
    F: Fn(&Connection, &str, &str, Value) -> Reply + Send + Sync + 'static,
{
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        self(peer, target, method, args)
    }
}

/// The families an end offers, ordered by `version`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Offers {
    pub(crate) families: BTreeSet<String>,
    pub(crate) version: u64,
    /// When each family was registered (its number in the end's sequence
    /// of offers, from 1). An end that does not say has every family at 0.
    pub(crate) since: BTreeMap<String, u64>,
}

impl Offers {
    /// Which offer of `family` this is, if it is offered: it changes
    /// whenever the family is registered anew, even if a follower never saw
    /// it withdrawn.
    pub(crate) fn epoch(&self, family: &str) -> Option<u64> {
        self.families
            .contains(family)
            .then(|| self.since.get(family).copied().unwrap_or(0))
    }
}

/// The family a call belongs to.
pub(crate) fn family<'a>(target: &'a str, method: &'a str) -> &'a str {
    match target {
        "" => method.split('.').next().unwrap_or(method),
        target => target.split(':').next().unwrap_or(target),
    }
}

/// A family registered on a session: its handler, and which offer of the
/// family it is.
struct Family {
    handler: Arc<dyn Handler>,
    since: u64,
}

/// The handlers of one session; its [`Dispatch`].
#[derive(Default)]
pub(crate) struct Operations {
    /// Each family with its offer, kept together: an announcement never
    /// sees a family without the offer it was registered as (#435).
    handlers: Mutex<HashMap<String, Family>>,
    /// What the far end offers.
    remote: watch::Sender<Offers>,
    /// The version of the last announcement of what this end offers.
    announced: Mutex<u64>,
    /// How many offers of families were made: each registration is a new
    /// one.
    offered: std::sync::atomic::AtomicU64,
    session: Mutex<Option<Connection>>,
    /// Versions of what features say about the session (services announced
    /// and withdrawn): one sequence for the session, so a feature that
    /// restarts keeps counting up and is never taken for an older message.
    versions: std::sync::atomic::AtomicU64,
}

impl Operations {
    /// Serve `session` and tell the far end what is offered.
    pub(crate) fn attach(&self, session: Connection) {
        *self.session.lock().unwrap() = Some(session);
        self.announce();
    }

    /// Forget the session, which holds this as its dispatch.
    pub(crate) fn detach(&self) {
        self.session.lock().unwrap().take();
        // Dropped outside the lock: a handler may hold what unregisters
        // families of its own (a process routed on this session).
        let handlers = std::mem::take(&mut *self.handlers.lock().unwrap());
        drop(handlers);
    }

    /// What this end offers now, as the announcement after `version`.
    fn next_offers(&self, version: &mut u64) -> Offers {
        *version += 1;
        let handlers = self.handlers.lock().unwrap();
        Offers {
            families: handlers.keys().cloned().collect(),
            version: *version,
            since: handlers
                .iter()
                .map(|(name, family)| (name.clone(), family.since))
                .collect(),
        }
    }

    /// Tell the far end what this end offers now. Fire and forget: a lost
    /// announcement is superseded by the next, and a closed session ends
    /// the question.
    fn announce(&self) {
        let Some(session) = self.session.lock().unwrap().clone() else {
            return;
        };
        let offers = self.next_offers(&mut self.announced.lock().unwrap());
        let args = json!([{
            "families": offers.families,
            "version": offers.version,
            "since": offers.since,
        }]);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = session.invoke_async("", "link.offers", args.into()).await;
            });
        }
    }
}

impl Dispatch for Operations {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        if target.is_empty() && method == "link.offers" {
            let announced = args
                .list()?
                .into_iter()
                .next()
                .ok_or_else(|| Error::refused("link.offers needs its offers".into()))?
                .json()?;
            let families: BTreeSet<String> = crate::session::decode(announced["families"].clone())?;
            let version: u64 = crate::session::decode(announced["version"].clone())?;
            let since: BTreeMap<String, u64> = match announced.get("since") {
                Some(since) => crate::session::decode(since.clone())?,
                None => BTreeMap::new(),
            };
            // An older announcement never overrides a newer one.
            self.remote.send_if_modified(|offers| {
                if version <= offers.version {
                    return false;
                }
                *offers = Offers {
                    families,
                    version,
                    since,
                };
                true
            });
            return Ok(Value::Undefined);
        }
        let family = family(target, method);
        let handler =
            (self.handlers.lock().unwrap().get(family)).map(|family| family.handler.clone());
        match handler {
            Some(handler) => handler.invoke(peer, target, method, args),
            None => Err(Error::refused(format!(
                "{} is not offered here",
                if target.is_empty() { method } else { target }
            ))),
        }
    }
}

/// A session with one far end, ready: the far end greeted as `id`.
pub(crate) struct Peer {
    id: PeerId,
    connection: Connection,
    operations: Arc<Operations>,
}

/// The key the peer `id` is provided under (`Peer#mac`).
pub(crate) fn peer_key(id: &PeerId) -> Key<Peer> {
    Key::dynamic(id.to_string())
}

/// A family registered on a peer; dropping it unregisters the family and
/// tells the far end.
#[must_use = "dropping the registration unregisters the family"]
pub(crate) struct Offered {
    operations: Weak<Operations>,
    family: String,
    since: u64,
}

impl Offered {
    /// Which offer of the family this is: what the far end reads as its
    /// epoch ([`Offers::epoch`]). A later registration of the family on the
    /// session has a higher one.
    pub(crate) fn epoch(&self) -> u64 {
        self.since
    }
}

impl Drop for Offered {
    fn drop(&mut self) {
        if let Some(operations) = self.operations.upgrade() {
            if operations
                .handlers
                .lock()
                .unwrap()
                .remove(&self.family)
                .is_some()
            {
                operations.announce();
            }
        }
    }
}

impl Peer {
    pub(crate) fn new(id: PeerId, connection: Connection, operations: Arc<Operations>) -> Self {
        Self {
            id,
            connection,
            operations,
        }
    }

    /// The far end's endpoint id.
    pub(crate) fn id(&self) -> &PeerId {
        &self.id
    }

    /// The session, to call the far end.
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Serve `family` on this session. A family has one handler at a time.
    pub(crate) fn register(
        &self,
        family: &str,
        handler: Arc<dyn Handler>,
    ) -> Result<Offered, Error> {
        if family.is_empty() || family == "link" || family.contains(['.', ':']) {
            return Err(Error::refused(format!("{family:?} cannot be registered")));
        }
        let since = {
            let mut handlers = self.operations.handlers.lock().unwrap();
            if handlers.contains_key(family) {
                return Err(Error::refused(format!("{family} is already registered")));
            }
            // A new offer of the family, whatever was offered before,
            // registered with its handler in one step.
            let since =
                (self.operations.offered).fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            handlers.insert(family.to_owned(), Family { handler, since });
            since
        };
        self.operations.announce();
        Ok(Offered {
            operations: Arc::downgrade(&self.operations),
            family: family.to_owned(),
            since,
        })
    }

    /// The next version in this session's sequence: higher than any taken
    /// before on it, whichever feature took it.
    pub(crate) fn next_version(&self) -> u64 {
        self.operations
            .versions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    /// What the far end offers, as it announces it.
    pub(crate) fn offers(&self) -> watch::Receiver<Offers> {
        self.operations.remote.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A family registered while an announcement is being made is in it
    /// only with its offer. It was announced at first without one (epoch
    /// 0), then with it: to the far end, two offers of the family, and a
    /// follower restarted what it ran on it (#435).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_family_is_announced_only_with_its_offer() {
        let (channel, _far) = crate::memory::pair();
        let operations = Arc::new(Operations::default());
        let connection = Connection::open(
            channel,
            operations.clone(),
            crate::session::rpc::Endpoint::rust(PeerId::new("main").unwrap()),
        )
        .unwrap();
        let peer = Peer::new(PeerId::new("mac").unwrap(), connection, operations.clone());
        let peer = Arc::new(peer);

        // An announcement in progress (another family's registration, on
        // another thread), while "plugins" registers.
        let mut announcing = operations.announced.lock().unwrap();
        let registering = {
            let peer = peer.clone();
            std::thread::spawn(move || {
                let handler =
                    Arc::new(|_: &Connection, _: &str, _: &str, _: Value| Ok(Value::Undefined));
                peer.register("plugins", handler).unwrap()
            })
        };
        // Once the family can be seen (or is registered), what is announced
        // carries its offer.
        rutis_test_support::eventually_blocking(
            || {
                let visible = operations.handlers.lock().unwrap().contains_key("plugins");
                (visible || registering.is_finished()).then_some(())
            },
            "plugins registering",
        );
        let offers = operations.next_offers(&mut announcing);
        drop(announcing);
        let _offered = registering.join().unwrap();
        assert!(offers.families.contains("plugins"), "{offers:?}");
        assert_ne!(offers.epoch("plugins"), Some(0), "{offers:?}");
        let next = operations.next_offers(&mut operations.announced.lock().unwrap());
        assert_eq!(next.epoch("plugins"), offers.epoch("plugins"), "one offer");
    }
}
