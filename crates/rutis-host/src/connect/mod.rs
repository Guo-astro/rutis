//! The connection plugins rutis-host ships
//! (`docs/design-remote-session-2026-10-28.md` §六): each gets a
//! `Channel` and installs a session on it.
//!
//! - `rutis-host/dial`: dials one address, one channel at a time, backing
//!   off between attempts ([`dial`]).
//! - `rutis-host/listen`: takes the connections presenting its token on one
//!   of the host's listeners, one session per connection, a newer
//!   connection of an id replacing the older ([`listen`]).
//!
//! Each row is named by its id: its output lines start with it, and its
//! token is `RUTIS_TOKEN_<row id>`.

use std::sync::{Arc, Mutex};

use rutis::host::CtxHost;
use rutis::host::{DynPlugin, ErrorKind, PluginFactory};
use rutis::{Ctx, Error};
use rutis_bridge::websocket::Trust;
use rutis_loader::Builtins;
use serde_json::{json, Value};

pub mod dial;
pub mod listen;
pub(crate) mod session;

pub use listen::Router;

pub const DIAL: &str = "rutis-host/dial";
pub const LISTEN: &str = "rutis-host/listen";

/// Prints one output line.
pub(crate) type Print = Arc<dyn Fn(String) + Send + Sync>;
/// The token of a row id: [`crate::config::token`].
pub(crate) type Tokens = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

fn print() -> Print {
    Arc::new(|line| crate::errln!("{line}"))
}

fn tokens() -> Tokens {
    Arc::new(crate::config::token)
}

/// What the connection plugins of one host share.
#[derive(Clone)]
pub struct Connect {
    /// Who dial trusts (`RUTIS_CA` besides the system's roots).
    pub trust: Trust,
    /// Routes the host's listeners' upgrades to the listen rows.
    pub router: Arc<Router>,
    /// What each connection row has connected, for `rutis-host status`.
    pub connections: Connections,
}

/// Register `rutis-host/dial` and `rutis-host/listen`.
pub fn register(builtins: &mut Builtins, connect: &Connect) {
    builtins.register_raw(
        DIAL,
        Factory {
            name: DIAL,
            connect: connect.clone(),
        },
        Some(json!({
            "type": "object",
            "required": ["address"],
            "properties": {
                "address": { "type": "string", "description": "wss:// (or ws:// to a loopback address)" }
            },
            "additionalProperties": false
        })),
    );
    builtins.register_raw(
        LISTEN,
        Factory {
            name: LISTEN,
            connect: connect.clone(),
        },
        Some(json!({
            "type": "object",
            "required": ["listener"],
            "properties": {
                "listener": { "type": "string", "description": "an entry of listen, by name" }
            },
            "additionalProperties": false
        })),
    );
}

struct Factory {
    name: &'static str,
    connect: Connect,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DialConfig {
    address: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ListenConfig {
    listener: String,
}

fn decode<C: serde::de::DeserializeOwned>(name: &str, config: &Value) -> Result<C, Error> {
    serde_json::from_value(config.clone())
        .map_err(|error| Error::from(ErrorKind::PluginFailed(format!("{name}: {error}").into())))
}

impl PluginFactory<Value> for Factory {
    fn name(&self) -> &str {
        self.name
    }

    fn validate_config(&self, config: &Value) -> Result<(), Error> {
        self.build(config).map(drop)
    }

    fn build(&self, config: &Value) -> Result<Box<dyn DynPlugin>, Error> {
        let connect = &self.connect;
        Ok(match self.name {
            DIAL => {
                let DialConfig { address } = decode(self.name, config)?;
                Box::new(dial::DialPlugin::new(
                    dial::websocket(address, connect.trust.clone()),
                    connect.connections.clone(),
                    tokens(),
                    print(),
                ))
            }
            _ => {
                let ListenConfig { listener } = decode(self.name, config)?;
                Box::new(listen::ListenPlugin::new(
                    listener,
                    connect.router.clone(),
                    connect.connections.clone(),
                    tokens(),
                    print(),
                ))
            }
        })
    }
}

/// The id of the row `ctx` is a plugin of.
pub(crate) fn row_id(ctx: &Ctx, plugin: &str) -> Result<String, Error> {
    ctx.row().ok_or_else(|| {
        Error::from(ErrorKind::PluginFailed(
            format!("{plugin} runs as a row: its id names it").into(),
        ))
    })
}

/// Run `work` until the row of `ctx` starts unloading: its sessions,
/// unloading with it, are not said to have disconnected.
fn until_unloaded(
    ctx: &Ctx,
    work: impl std::future::Future<Output = ()> + Send + 'static,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    let unloading = ctx.cancelled();
    async move {
        tokio::select! {
            biased;
            () = unloading => {}
            () = work => {}
        }
    }
}

/// What each connection row has connected now, for `rutis-host status`.
#[derive(Clone, Default)]
pub struct Connections(Arc<Mutex<Vec<Row>>>);

/// One connection row while applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    /// A dial row (one far end), not a listen row.
    pub dials: bool,
    pub connected: Vec<Connected>,
}

/// One far end connected now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connected {
    /// The id it gave in its handshake.
    pub id: String,
    /// The remote plugins it registered here (`<id>:<plugin>`).
    pub plugins: Vec<String>,
}

impl Connections {
    /// Every applied connection row, in the order they were applied.
    pub fn rows(&self) -> Vec<Row> {
        self.0.lock().unwrap().clone()
    }

    /// Say which remote plugins the far end `id` of `row` has registered.
    pub fn plugins(&self, row: &str, id: &str, plugins: Vec<String>) {
        self.edit(row, |row| {
            if let Some(connected) = row.connected.iter_mut().find(|c| c.id == id) {
                connected.plugins = plugins;
            }
        });
    }

    /// Whether `plugin` is a remote plugin, `<id>:<plugin>` of a far end
    /// connected now.
    pub fn is_far_plugin(&self, plugin: &str) -> bool {
        let Some((id, _)) = plugin.split_once(':') else {
            return false;
        };
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|row| row.connected.iter().any(|c| c.id == id))
    }

    fn applied(&self, id: &str, dials: bool) {
        let mut rows = self.0.lock().unwrap();
        rows.retain(|row| row.id != id);
        rows.push(Row {
            id: id.to_owned(),
            dials,
            connected: Vec::new(),
        });
    }

    fn removed(&self, id: &str) {
        self.0.lock().unwrap().retain(|row| row.id != id);
    }

    fn connected(&self, row: &str, id: &str) {
        self.edit(row, |row| {
            row.connected.retain(|c| c.id != id);
            row.connected.push(Connected {
                id: id.to_owned(),
                plugins: Vec::new(),
            });
        });
    }

    fn disconnected(&self, row: &str, id: &str) {
        self.edit(row, |row| row.connected.retain(|c| c.id != id));
    }

    fn edit(&self, id: &str, edit: impl FnOnce(&mut Row)) {
        if let Some(row) = self.0.lock().unwrap().iter_mut().find(|row| row.id == id) {
            edit(row);
        }
    }
}

#[cfg(test)]
mod testing {
    use rutis::host::{BoxFuture, DynPlugin, Effect};
    use rutis::internal::CtxInternal;
    use rutis::{Ctx, Error};
    use std::sync::{Arc, Mutex};

    /// `plugin` as the row `row`, as the loader mounts it.
    pub(super) struct AsRow<P> {
        pub(super) row: String,
        pub(super) plugin: P,
    }

    impl<P: DynPlugin> DynPlugin for AsRow<P> {
        fn name(&self) -> &str {
            self.plugin.name()
        }

        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                ctx.mark_row(&self.row)?;
                self.plugin.apply(ctx).await
            })
        }
    }

    /// The lines printed, and what prints them.
    pub(super) fn lines() -> (Arc<Mutex<Vec<String>>>, super::Print) {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let into = lines.clone();
        (lines, Arc::new(move |line| into.lock().unwrap().push(line)))
    }

    /// Wait until `done` holds: on the paused clock too, where the
    /// sessions' threads still take real time.
    pub(super) async fn until(mut done: impl FnMut() -> bool) {
        for _ in 0..5000 {
            if done() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("timed out waiting");
    }
}
