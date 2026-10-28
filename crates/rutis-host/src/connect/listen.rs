//! `rutis-host/listen`: the connections presenting the row's token on one
//! of the host's listeners, one session each.
//!
//! The host owns the listeners; their upgrades go through a [`Router`],
//! on which each applied listen row has its token. A newer connection of
//! an id already connected replaces the older once its handshake is done
//! (`docs/design-remote-session-2026-10-28.md` §4.2): the old session
//! unloads (its far plugins all unloaded), its channel is closed as
//! replaced (4002), then the new session is installed. A connection whose
//! handshake fails touches nothing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rutis::host::{BoxFuture, DynPlugin, Effect, ErrorKind};
use rutis::{Ctx, Error};
use rutis_bridge::channel::Closer;
use rutis_bridge::websocket::{Admit, Admitted, Refused, Upgrade};
use rutis_bridge::{Channel, Close};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::session::{self, Ended, Greeted, Installed};
use super::{Connections, Print, Tokens};

/// Routes the upgrades on the host's listeners to the listen rows, by
/// token.
pub struct Router {
    /// The host's listeners, by name.
    listeners: Vec<String>,
    entries: Mutex<Vec<Entry>>,
    next: AtomicU64,
}

struct Entry {
    serial: u64,
    listener: String,
    token: String,
    deliver: mpsc::UnboundedSender<Channel>,
}

impl Router {
    /// A router for the host's listeners named `listeners`.
    pub fn new(listeners: impl IntoIterator<Item = String>) -> Arc<Self> {
        Arc::new(Self {
            listeners: listeners.into_iter().collect(),
            entries: Mutex::default(),
            next: AtomicU64::new(0),
        })
    }

    /// What the listener `listener` lets through: an upgrade presenting
    /// the token of a listen row on it, its channel delivered to that row.
    /// One with no credentials is refused with 401 before this; one whose
    /// token no row on the listener has, with 403.
    pub fn admit(self: &Arc<Self>, listener: &str) -> Admit {
        let (router, listener) = (self.clone(), listener.to_owned());
        Arc::new(move |upgrade: Upgrade<'_>| {
            let refused = || Refused::Forbidden("not accepted here".into());
            let presented = upgrade.bearer.ok_or_else(refused)?;
            // Every entry is compared, each in constant time, so timing
            // tells nothing about which token came close.
            let mut found = None;
            for entry in router.entries.lock().unwrap().iter() {
                let same =
                    crate::host::constant_time_eq(entry.token.as_bytes(), presented.as_bytes());
                if same && entry.listener == listener {
                    found = Some(entry.deliver.clone());
                }
            }
            let deliver = found.ok_or_else(refused)?;
            Ok(Admitted {
                protocol: format!("rutis.{}", rutis_bridge::session::ENDPOINT_PROTOCOL),
                deliver: Box::new(move |channel| {
                    if let Err(refused) = deliver.send(channel) {
                        refused.0.closer.close(Close::Done);
                    }
                }),
            })
        })
    }

    /// Deliver the channels presenting `token` on `listener` to `deliver`
    /// while the result lives.
    fn register(
        self: &Arc<Self>,
        listener: &str,
        token: String,
        deliver: mpsc::UnboundedSender<Channel>,
    ) -> Result<Registered, Error> {
        if !self.listeners.iter().any(|name| name == listener) {
            return Err(Error::from(ErrorKind::PluginFailed(
                format!("no listener named {listener}: add it to listen").into(),
            )));
        }
        let serial = self.next.fetch_add(1, Ordering::Relaxed);
        self.entries.lock().unwrap().push(Entry {
            serial,
            listener: listener.to_owned(),
            token,
            deliver,
        });
        Ok(Registered {
            router: self.clone(),
            serial,
        })
    }
}

/// A row's entry on the router; dropping it removes it.
struct Registered {
    router: Arc<Router>,
    serial: u64,
}

impl Drop for Registered {
    fn drop(&mut self) {
        let serial = self.serial;
        self.router
            .entries
            .lock()
            .unwrap()
            .retain(|entry| entry.serial != serial);
    }
}

pub struct ListenPlugin {
    listener: String,
    router: Arc<Router>,
    connections: Connections,
    tokens: Tokens,
    print: Print,
}

impl ListenPlugin {
    pub(crate) fn new(
        listener: String,
        router: Arc<Router>,
        connections: Connections,
        tokens: Tokens,
        print: Print,
    ) -> Self {
        Self {
            listener,
            router,
            connections,
            tokens,
            print,
        }
    }
}

impl DynPlugin for ListenPlugin {
    fn name(&self) -> &str {
        super::LISTEN
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let row = super::row_id(ctx, super::LISTEN)?;
            let token = (self.tokens)(&row).ok_or_else(|| {
                Error::from(ErrorKind::PluginFailed(
                    format!(
                        "{row}: set RUTIS_TOKEN_{}: the token the far ends present when they connect",
                        row.to_uppercase().replace('-', "_")
                    )
                    .into(),
                ))
            })?;
            let (deliver, delivered) = mpsc::unbounded_channel();
            // Registered before apply returns: once applied, its far ends
            // reach it.
            let registered = self.router.register(&self.listener, token, deliver)?;
            self.connections.applied(&row, false);
            let listening = Listening {
                ctx: ctx.clone(),
                row: row.clone(),
                connections: self.connections.clone(),
                print: self.print.clone(),
            };
            let task = tokio::spawn(super::until_unloaded(
                ctx,
                listening.run(registered, delivered),
            ));
            let connections = self.connections.clone();
            ctx.effect(move || async move {
                // The sessions, children of this row, unload with it.
                task.abort();
                let _ = task.await;
                connections.removed(&row);
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

/// A channel's closer, which, once its session is replaced, closes it as
/// replaced: the session closes its channel last when it unloads, so the
/// far end learns it was replaced only after every far plugin unloaded.
struct Replaceable {
    closer: Arc<dyn Closer>,
    replaced: AtomicBool,
}

impl Closer for Replaceable {
    fn close(&self, why: Close) {
        let why = match why {
            Close::Done if self.replaced.load(Ordering::SeqCst) => Close::Replaced,
            why => why,
        };
        self.closer.close(why);
    }
}

/// One connected far end.
struct Live {
    serial: u64,
    installed: Arc<Installed>,
    closer: Arc<Replaceable>,
}

struct Listening {
    ctx: Ctx,
    row: String,
    connections: Connections,
    print: Print,
}

impl Listening {
    async fn run(self, _registered: Registered, mut delivered: mpsc::UnboundedReceiver<Channel>) {
        let mut live: HashMap<String, Live> = HashMap::new();
        let mut serial = 0u64;
        // Handshakes run side by side; one slow far end holds up no other.
        let mut handshakes: JoinSet<(Result<Greeted, Ended>, Arc<Replaceable>)> = JoinSet::new();
        let (ended, mut endings) = mpsc::unbounded_channel::<(String, u64, Ended)>();
        loop {
            tokio::select! {
                Some(channel) = delivered.recv() => {
                    let closer = Arc::new(Replaceable {
                        closer: channel.closer,
                        replaced: AtomicBool::new(false),
                    });
                    let channel = Channel {
                        sender: channel.sender,
                        receiver: channel.receiver,
                        closer: closer.clone(),
                    };
                    let ctx = self.ctx.clone();
                    handshakes.spawn(async move { (session::handshake(&ctx, channel).await, closer) });
                }
                Some(Ok((greeted, closer))) = handshakes.join_next() => {
                    let Ok(greeted) = greeted else { continue };
                    let id = greeted.id().to_owned();
                    if let Some(old) = live.remove(&id) {
                        // §4.2: the old session unloads, every cleanup
                        // finished; its channel closes as replaced; then
                        // the new one is installed.
                        old.closer.replaced.store(true, Ordering::SeqCst);
                        old.installed.unload().await;
                        old.closer.close(Close::Replaced);
                        self.connections.disconnected(&self.row, &id);
                        (self.print)(format!(
                            "{}: disconnected {id} (replaced by a new connection)",
                            self.row
                        ));
                    }
                    serial += 1;
                    let installed = Arc::new(session::install(&self.ctx, greeted));
                    let (watched, ended, at) = (installed.clone(), ended.clone(), serial);
                    let watching = id.clone();
                    tokio::spawn(async move {
                        let why = watched.unloaded().await;
                        let _ = ended.send((watching, at, why));
                    });
                    live.insert(id.clone(), Live { serial, installed, closer });
                    self.connections.connected(&self.row, &id);
                    (self.print)(format!("{}: connected {id}", self.row));
                }
                Some((id, at, why)) = endings.recv() => {
                    // A replaced session's end was said already.
                    if live.get(&id).is_some_and(|live| live.serial == at) {
                        live.remove(&id);
                        self.connections.disconnected(&self.row, &id);
                        (self.print)(format!("{}: disconnected {id} ({})", self.row, why.reason));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::session::events;
    use crate::connect::testing::{lines, until, AsRow};
    use rutis::host::CtxHost;
    use rutis_bridge::channel::ChannelError;

    const TOKEN: &str = "phone-token";

    struct Listened {
        root: Ctx,
        view: rutis::host::FiberView,
        router: Arc<Router>,
        lines: Arc<Mutex<Vec<String>>>,
        connections: Connections,
    }

    async fn start(row: &str) -> Listened {
        let root = Ctx::root().unwrap();
        let router = Router::new(["public".to_owned()]);
        let (lines, print) = lines();
        let connections = Connections::default();
        let tokens: Tokens = Arc::new(|row| match row {
            "phone" => Some(TOKEN.to_owned()),
            "watch" => Some("watch-token".to_owned()),
            _ => None,
        });
        let view = root.plugin(AsRow {
            row: row.into(),
            plugin: ListenPlugin::new(
                "public".into(),
                router.clone(),
                connections.clone(),
                tokens,
                print,
            ),
        });
        until(|| !connections.rows().is_empty()).await;
        Listened {
            root,
            view,
            router,
            lines,
            connections,
        }
    }

    impl Listened {
        /// Connect with `token`, the far end greeting with `hello`; the
        /// far end, or the refusal.
        fn connect(&self, token: Option<&str>, hello: &str) -> Result<Channel, Refused> {
            let admitted = (self.router.admit("public"))(Upgrade {
                bearer: token,
                certificate: None,
            })?;
            assert_eq!(admitted.protocol, "rutis.4");
            let (near, mut far) = rutis_bridge::memory::pair();
            far.sender.send(hello.as_bytes()).unwrap();
            (admitted.deliver)(near);
            Ok(far)
        }

        fn greet(&self, id: &str) -> Channel {
            self.connect(
                Some(TOKEN),
                &format!(r#"{{"op":"hello","endpoint":"{id}"}}"#),
            )
            .unwrap()
        }

        async fn printed(&self, count: usize) -> Vec<String> {
            until(|| self.lines.lock().unwrap().len() >= count).await;
            self.lines.lock().unwrap().clone()
        }

        fn connected(&self) -> Vec<String> {
            self.connections
                .rows()
                .iter()
                .flat_map(|row| row.connected.iter().map(|c| c.id.clone()))
                .collect()
        }
    }

    /// The far end's next receive, on a thread.
    fn far_end(far: Channel) -> std::thread::JoinHandle<Result<Option<Vec<u8>>, ChannelError>> {
        // The whole channel moves: its sender dropped would end it.
        std::thread::spawn(move || {
            let mut far = far;
            far.receiver.recv()
        })
    }

    fn position(line: &str) -> usize {
        events()
            .lock()
            .unwrap()
            .iter()
            .position(|event| event == line)
            .unwrap_or_else(|| panic!("no {line:?}"))
    }

    #[tokio::test]
    async fn upgrades_are_routed_by_token_and_a_token_of_no_row_is_refused() {
        let phone = start("phone").await;
        let refused = phone.connect(Some("wrong"), "").err();
        assert_eq!(
            refused,
            Some(Refused::Forbidden("not accepted here".into()))
        );
        let refused = phone.connect(None, "").err();
        assert_eq!(
            refused,
            Some(Refused::Forbidden("not accepted here".into()))
        );
        let _far = phone.greet("routed-1");
        assert_eq!(phone.printed(1).await, ["phone: connected routed-1"]);
        assert_eq!(phone.connected(), ["routed-1"]);
        // Unloaded, its token is no longer let through.
        phone.view.dispose().await.unwrap();
        let refused = phone.connect(Some(TOKEN), "").err();
        assert_eq!(
            refused,
            Some(Refused::Forbidden("not accepted here".into()))
        );
        phone.root.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn two_rows_on_one_listener_each_get_their_own_connections() {
        let root = Ctx::root().unwrap();
        let router = Router::new(["public".to_owned()]);
        let (lines, print) = lines();
        let connections = Connections::default();
        let tokens: Tokens = Arc::new(|row| Some(format!("{row}-token")));
        for row in ["phone", "watch"] {
            root.plugin(AsRow {
                row: row.into(),
                plugin: ListenPlugin::new(
                    "public".into(),
                    router.clone(),
                    connections.clone(),
                    tokens.clone(),
                    print.clone(),
                ),
            });
        }
        until(|| connections.rows().len() == 2).await;
        let mut fars = Vec::new();
        for (token, id) in [("watch-token", "two-w"), ("phone-token", "two-p")] {
            let admitted = (router.admit("public"))(Upgrade {
                bearer: Some(token),
                certificate: None,
            })
            .unwrap();
            let (near, mut far) = rutis_bridge::memory::pair();
            far.sender
                .send(format!(r#"{{"op":"hello","endpoint":"{id}"}}"#).as_bytes())
                .unwrap();
            (admitted.deliver)(near);
            fars.push(far);
        }
        until(|| lines.lock().unwrap().len() == 2).await;
        let mut printed = lines.lock().unwrap().clone();
        printed.sort();
        assert_eq!(
            printed,
            ["phone: connected two-p", "watch: connected two-w"]
        );
        // Another listener's name lets nothing through.
        let other = (router.admit("private"))(Upgrade {
            bearer: Some("phone-token"),
            certificate: None,
        });
        assert!(other.is_err());
        root.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_newer_connection_replaces_the_older_after_the_older_fully_unloaded() {
        let phone = start("phone").await;
        let old = phone.greet("swap-1");
        assert_eq!(phone.printed(1).await, ["phone: connected swap-1"]);
        let old_end = far_end(old);
        let _new = phone.greet("swap-1");
        let lines = phone.printed(3).await;
        assert_eq!(
            lines,
            [
                "phone: connected swap-1",
                "phone: disconnected swap-1 (replaced by a new connection)",
                "phone: connected swap-1",
            ]
        );
        // The old far end was told it was replaced (4002 on WebSocket).
        until(|| old_end.is_finished()).await;
        assert_eq!(
            old_end.join().unwrap(),
            Err(ChannelError::Closed {
                reason: "replaced by a new connection".into()
            })
        );
        // The old session's far plugins all unloaded before the new one
        // was installed.
        let installed: Vec<usize> = events()
            .lock()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, event)| *event == "installed swap-1")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(installed.len(), 2);
        assert!(position("unloading swap-1") < position("unloaded swap-1"));
        assert!(position("unloaded swap-1") < installed[1]);
        assert_eq!(phone.connected(), ["swap-1"]);
        phone.root.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_connection_whose_handshake_fails_leaves_the_older_alone() {
        let phone = start("phone").await;
        let old = phone.greet("kept-1");
        assert_eq!(phone.printed(1).await, ["phone: connected kept-1"]);
        let old_end = far_end(old);
        let bad = phone.connect(Some(TOKEN), "not a hello").unwrap();
        let bad_end = far_end(bad);
        until(|| bad_end.is_finished()).await;
        assert_eq!(bad_end.join().unwrap(), Ok(None));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!old_end.is_finished());
        assert_eq!(phone.connected(), ["kept-1"]);
        assert_eq!(phone.lines.lock().unwrap().len(), 1);
        phone.root.shutdown().await.unwrap();
        until(|| old_end.is_finished()).await;
    }

    #[tokio::test]
    async fn a_far_end_leaving_is_said_and_unloading_the_row_unloads_its_sessions() {
        let phone = start("phone").await;
        let first = phone.greet("left-1");
        let second = phone.greet("left-2");
        until(|| phone.connected().len() == 2).await;
        first.closer.close(Close::Done);
        let lines = phone.printed(3).await;
        assert_eq!(lines[2], "phone: disconnected left-1 (connection lost)");
        assert_eq!(phone.connected(), ["left-2"]);
        let second_end = far_end(second);
        phone.view.dispose().await.unwrap();
        // Unloaded with the row: its far end sees the channel close.
        assert!(position("unloaded left-2") > 0);
        until(|| second_end.is_finished()).await;
        assert_eq!(second_end.join().unwrap(), Ok(None));
        assert!(phone.connections.rows().is_empty());
        assert_eq!(phone.lines.lock().unwrap().len(), 3);
        phone.root.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_row_needs_its_token_and_a_listener_of_the_host() {
        let root = Ctx::root().unwrap();
        let router = Router::new(["public".to_owned()]);
        let (_, print) = lines();
        let tokens: Tokens = Arc::new(|row| (row == "phone").then(|| TOKEN.to_owned()));
        for (row, listener, error) in [
            (
                "tablet",
                "public",
                "tablet: set RUTIS_TOKEN_TABLET: the token the far ends present when they connect",
            ),
            (
                "phone",
                "private",
                "no listener named private: add it to listen",
            ),
        ] {
            let view = root.plugin(AsRow {
                row: row.into(),
                plugin: ListenPlugin::new(
                    listener.into(),
                    router.clone(),
                    Connections::default(),
                    tokens.clone(),
                    print.clone(),
                ),
            });
            let _ = (&view).await;
            let state = view.state();
            let said = state
                .error
                .map(|error| error.to_string())
                .unwrap_or_default();
            assert!(said.contains(error), "{said}");
        }
        root.shutdown().await.unwrap();
    }

    /// Over the host's real listener: no credentials 401, a token of no
    /// row 403, as the dialer reads them.
    #[tokio::test]
    async fn over_websocket_no_credentials_is_401_and_an_unknown_token_403() {
        use rutis_bridge::websocket::{Limits, ListenerConfig, Trust};
        let phone = start("phone").await;
        let config = ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap());
        let listener = rutis_bridge::websocket::listen(
            &config,
            &Limits::default(),
            phone.router.admit("public"),
        )
        .unwrap();
        let address = format!("ws://{}/rutis", listener.local_addr());
        let protocol = format!("rutis.{}", rutis_bridge::session::ENDPOINT_PROTOCOL);
        let dial = |token: Option<&str>| {
            let credential = token.map(|token| rutis_bridge::Credential::Bearer(token.into()));
            let (address, protocol) = (address.clone(), protocol.clone());
            async move {
                rutis_bridge::websocket::dial(
                    &address,
                    &protocol,
                    credential.as_ref(),
                    &Trust::default(),
                    &Limits::default(),
                )
                .await
            }
        };
        let none = dial(None).await.unwrap_err().to_string();
        assert!(none.contains("401 Unauthorized"), "{none}");
        let wrong = dial(Some("wrong")).await.unwrap_err().to_string();
        assert!(wrong.contains("403 Forbidden"), "{wrong}");
        let mut far = dial(Some(TOKEN)).await.unwrap();
        far.sender
            .send(br#"{"op":"hello","endpoint":"ws-1"}"#)
            .unwrap();
        assert_eq!(phone.printed(1).await, ["phone: connected ws-1"]);
        phone.root.shutdown().await.unwrap();
    }
}
