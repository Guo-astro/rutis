// What the connection plugins need from a session, and nothing else:
// shake hands on a channel ([`handshake`]), the far end's id
// ([`Greeted::id`]), install it under the connection plugin ([`install`],
// `ctx.plugin(greeted)`), and wait until it unloaded ([`Installed::unloaded`]).
// rutis-bridge's `Session` stands behind these names; the unit tests here use
// a double (`double`), whose far end's first message is
// `{"op":"hello","endpoint":"<id>"}`.

/// Why a session ended, or why its handshake failed: what a dialer does
/// next depends on `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ended {
    pub(crate) kind: Kind,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The channel ended: lost, closed, timed out.
    Lost,
    /// The far end refused this end (4003, or the handshake's identity
    /// check).
    Rejected,
    /// The handshake failed on the protocol: version, capabilities.
    Incompatible,
}

impl Ended {
    fn new(kind: Kind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
pub(crate) use double::*;
#[cfg(not(test))]
pub(crate) use real::*;

#[cfg(not(test))]
mod real {
    use super::{Ended, Kind};
    use rutis::host::{FiberState, FiberView};
    use rutis::Ctx;
    use rutis_bridge::session::{Error, Handshake};
    use rutis_bridge::Channel;

    pub(crate) use rutis_bridge::Greeted;

    /// Shake hands on `channel`. On failure the channel is closed.
    pub(crate) async fn handshake(ctx: &Ctx, channel: Channel) -> Result<Greeted, Ended> {
        rutis_bridge::Session::handshake(ctx, channel)
            .await
            .map_err(|error| {
                let kind = match &error {
                    Error::Handshake(Handshake::Incompatible(_)) => Kind::Incompatible,
                    Error::Handshake(Handshake::IdentityMismatch(_)) => Kind::Rejected,
                    _ => Kind::Lost,
                };
                Ended::new(kind, error.to_string())
            })
    }

    /// An installed session.
    pub(crate) struct Installed {
        view: FiberView,
    }

    /// Install `greeted` under `ctx`: from now on it registers the far
    /// end's plugins, and it unloads with `ctx`'s plugin.
    pub(crate) fn install(ctx: &Ctx, greeted: Greeted) -> Installed {
        Installed {
            view: ctx.plugin(greeted),
        }
    }

    impl Installed {
        /// Wait until the session unloaded, and say why.
        pub(crate) async fn unloaded(&self) -> Ended {
            let mut watch = self.view.watch();
            loop {
                let state = watch.borrow_and_update().state;
                if matches!(state, FiberState::Disposed | FiberState::Failed) {
                    break;
                }
                if watch.changed().await.is_err() {
                    break;
                }
            }
            let _ = self.view.dispose().await;
            let reason = self
                .view
                .state()
                .error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "connection lost".into());
            Ended::new(Kind::Lost, reason)
        }

        /// Unload the session and wait until it is unloaded.
        pub(crate) async fn unload(&self) {
            let _ = self.view.dispose().await;
        }
    }
}

#[cfg(test)]
mod double {
    use super::{Ended, Kind};
    use std::sync::{Arc, Mutex, OnceLock};

    use rutis::host::CtxHost;
    use rutis::host::{BoxFuture, DynPlugin, Effect, FiberState, FiberView};
    use rutis::{Ctx, Error};
    use rutis_bridge::channel::ChannelError;
    use rutis_bridge::{Channel, Close, PeerId};

    /// A channel whose far end finished the handshake; nothing of it is
    /// registered here until it is installed.
    pub(crate) struct Greeted {
        id: String,
        channel: Mutex<Option<Channel>>,
        ended: Arc<OnceLock<Ended>>,
    }

    impl Greeted {
        /// The id the far end gave in its handshake.
        pub(crate) fn id(&self) -> &str {
            &self.id
        }
    }

    /// Shake hands on `channel`. On failure the channel is closed.
    pub(crate) async fn handshake(_ctx: &Ctx, channel: Channel) -> Result<Greeted, Ended> {
        let Channel {
            sender,
            mut receiver,
            closer,
        } = channel;
        let (receiver, first) = tokio::task::spawn_blocking(move || {
            let first = receiver.recv();
            (receiver, first)
        })
        .await
        .map_err(|error| Ended::new(Kind::Lost, error.to_string()))?;
        let failed = |ended: Ended| {
            closer.close(Close::Done);
            Err(ended)
        };
        let hello = match first {
            Ok(Some(hello)) => hello,
            Ok(None) => return failed(Ended::new(Kind::Lost, "connection lost")),
            Err(ChannelError::Rejected { reason }) => {
                return failed(Ended::new(Kind::Rejected, reason))
            }
            Err(ChannelError::Closed { reason }) => return failed(Ended::new(Kind::Lost, reason)),
        };
        let id = serde_json::from_slice::<serde_json::Value>(&hello)
            .ok()
            .filter(|hello| hello["op"] == "hello")
            .and_then(|hello| hello["endpoint"].as_str().map(str::to_owned))
            .and_then(|id| PeerId::new(id).ok());
        let Some(id) = id else {
            return failed(Ended::new(Kind::Incompatible, "the far end did not greet"));
        };
        Ok(Greeted {
            id: id.to_string(),
            channel: Mutex::new(Some(Channel {
                sender,
                receiver,
                closer,
            })),
            ended: Arc::default(),
        })
    }

    /// An installed session.
    pub(crate) struct Installed {
        view: FiberView,
        ended: Arc<OnceLock<Ended>>,
    }

    /// Install `greeted` under `ctx`: from now on it registers the far end's
    /// plugins, and it unloads with `ctx`'s plugin.
    pub(crate) fn install(ctx: &Ctx, greeted: Greeted) -> Installed {
        let ended = greeted.ended.clone();
        Installed {
            view: ctx.plugin(greeted),
            ended,
        }
    }

    impl Installed {
        /// Wait until the session unloaded, and say why.
        pub(crate) async fn unloaded(&self) -> Ended {
            let mut watch = self.view.watch();
            loop {
                let state = watch.borrow_and_update().state;
                if matches!(state, FiberState::Disposed | FiberState::Failed) {
                    break;
                }
                if watch.changed().await.is_err() {
                    break;
                }
            }
            // A failed session is unloaded too.
            let _ = self.view.dispose().await;
            let error = self.view.state().error.map(|error| error.to_string());
            self.ended.get().cloned().unwrap_or_else(|| {
                Ended::new(Kind::Lost, error.unwrap_or("connection lost".into()))
            })
        }

        /// Unload the session and wait until it is unloaded.
        pub(crate) async fn unload(&self) {
            let _ = self.view.dispose().await;
        }
    }

    impl DynPlugin for Greeted {
        fn name(&self) -> &str {
            "rutis-bridge/session"
        }

        fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
            Box::pin(async move {
                let Some(Channel {
                    sender,
                    mut receiver,
                    closer,
                }) = self.channel.lock().unwrap().take()
                else {
                    return Err(Error::msg("a session is installed once"));
                };
                #[cfg(test)]
                log(format!("installed {}", self.id));
                // The far end leaving unloads the session.
                let (ended, own) = (self.ended.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _sender = sender;
                    let why = loop {
                        match receiver.recv() {
                            Ok(Some(_)) => continue,
                            Ok(None) => break Ended::new(Kind::Lost, "connection lost"),
                            Err(ChannelError::Rejected { reason }) => {
                                break Ended::new(Kind::Rejected, reason)
                            }
                            Err(ChannelError::Closed { reason }) => {
                                break Ended::new(Kind::Lost, reason)
                            }
                        }
                    };
                    let _ = ended.set(why);
                    let _ = own.dispose_self();
                });
                #[cfg(test)]
                let id = self.id.clone();
                ctx.effect(move || async move {
                    // Where the far end's plugins unload, before the channel
                    // closes.
                    #[cfg(test)]
                    {
                        log(format!("unloading {id}"));
                        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                        log(format!("unloaded {id}"));
                    }
                    closer.close(Close::Done);
                    Ok(())
                })?;
                Ok(Effect::Done)
            })
        }
    }

    /// What the test double did, in order.
    #[cfg(test)]
    pub(crate) fn log(line: String) {
        events().lock().unwrap().push(line);
    }

    #[cfg(test)]
    pub(crate) fn events() -> &'static Mutex<Vec<String>> {
        static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
        &EVENTS
    }
}
