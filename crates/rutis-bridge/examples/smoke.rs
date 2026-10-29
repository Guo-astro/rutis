//! A two-machine smoke test of the network stack, run by hand: one side
//! listens (wss) and provides `clock`; the other dials, mounts a session on
//! each connection and calls the clock, which the session brings over,
//! every second, dialing again a second after a session ends. Both print
//! every session that starts and ends, so cutting the network, restarting
//! either side or presenting a wrong token shows what a session does about
//! it.
//!
//! ```text
//! # on the listening machine (certificate and key for its host name)
//! cargo run -p rutis-bridge --features websocket --example smoke -- \
//!     listen 0.0.0.0:7443 --cert server.pem --key server.key --token secret
//!
//! # on the dialing machine (the CA that signed the server's certificate)
//! cargo run -p rutis-bridge --features websocket --example smoke -- \
//!     dial wss://server.example:7443/rutis --ca ca.pem --token secret
//! ```
//!
//! On one machine, `listen 127.0.0.1:7443 --token secret` and
//! `dial ws://127.0.0.1:7443/rutis --token secret` need no certificates.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, FiberView};
use rutis::Ctx;
use rutis_bridge::channel::{Channel, Close, PeerId};
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{Reply, Value};
use rutis_bridge::websocket::{
    self, Admitted, Credential, Limits, ListenerConfig, Refused, ServerTls, Trust, Upgrade,
};
use rutis_bridge::{LocalId, Session};
use serde_json::{json, Value as Json};

struct Clock(AtomicU64);
impl HostDispatch for Clock {
    fn invoke(&self, _: &str, _: Value) -> Reply {
        Ok(json!(self.0.fetch_add(1, Ordering::SeqCst)).into())
    }
    fn methods(&self) -> Option<Json> {
        Some(json!({ "now": "sync" }))
    }
}

struct Args {
    mode: String,
    address: String,
    cert: Option<String>,
    key: Option<String>,
    ca: Option<String>,
    token: String,
}

fn args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: smoke listen <bind> [--cert pem --key pem] --token t | dial <url> [--ca pem] --token t";
    let mode = args.next().ok_or(usage)?;
    let address = args.next().ok_or(usage)?;
    let (mut cert, mut key, mut ca, mut token) = (None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(usage)?;
        match flag.as_str() {
            "--cert" => cert = Some(value),
            "--key" => key = Some(value),
            "--ca" => ca = Some(value),
            "--token" => token = Some(value),
            _ => return Err(usage.into()),
        }
    }
    Ok(Args {
        mode,
        address,
        cert,
        key,
        ca,
        token: token.ok_or(usage)?,
    })
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// Provides `clock` (a plugin's service is shared with far ends; the
/// root's would not be).
struct ClockPlugin;

impl DynPlugin for ClockPlugin {
    fn name(&self) -> &str {
        "clock"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, rutis::Error>> {
        Box::pin(async move {
            ctx.provide_at(host_key("clock"), Arc::new(Clock(AtomicU64::new(0))))?;
            Ok(Effect::Done)
        })
    }
}

/// Wait until the session `view` unloads; why it ended.
async fn ended(view: &FiberView) -> String {
    let mut watch = view.watch();
    loop {
        if matches!(
            watch.borrow_and_update().state,
            FiberState::Disposed | FiberState::Failed
        ) {
            break;
        }
        if watch.changed().await.is_err() {
            break;
        }
    }
    view.state().error.map_or_else(
        || "the connection ended".to_owned(),
        |error| error.to_string(),
    )
}

/// Mount a session on `channel` and print when it starts and ends.
fn mount(root: &Ctx, channel: Channel, started: Instant) -> FiberView {
    let view = root.plugin(Session::new(channel));
    let watched = view.clone();
    tokio::spawn(async move {
        println!("[{:>7.1?}] session: started", started.elapsed());
        let why = ended(&watched).await;
        println!("[{:>7.1?}] session: ended ({why})", started.elapsed());
    });
    view
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = args()?;
    let started = Instant::now();
    let root = Ctx::root()?;
    let protocol = format!("rutis.{}", rutis_bridge::session::ENDPOINT_PROTOCOL);

    match args.mode.as_str() {
        "listen" => {
            root.provide(LocalId(PeerId::new("main")?))?;
            let mut config = ListenerConfig::new("public", args.address.parse()?);
            if let (Some(cert), Some(key)) = (&args.cert, &args.key) {
                config = config.tls(ServerTls {
                    certificate_pem: read(cert),
                    key_pem: read(key),
                    client_ca_pem: None,
                });
            }
            let (sender, mut accepted) = tokio::sync::mpsc::unbounded_channel::<Channel>();
            let token = args.token.clone();
            let admit: websocket::Admit = Arc::new(move |upgrade: Upgrade<'_>| {
                if upgrade.bearer != Some(token.as_str()) {
                    return Err(Refused::Forbidden("not accepted here".into()));
                }
                let sender = sender.clone();
                Ok(Admitted {
                    protocol: protocol.clone(),
                    deliver: Box::new(move |channel| {
                        if let Err(refused) = sender.send(channel) {
                            refused.0.closer.close(Close::Done);
                        }
                    }),
                })
            });
            let _listener = websocket::listen(&config, &Limits::default(), admit)?;
            root.plugin(ClockPlugin);
            println!(
                "listening on {}; providing clock (Ctrl-C ends it)",
                args.address
            );
            // A newer connection replaces the older one.
            let mut current: Option<FiberView> = None;
            while let Some(channel) = accepted.recv().await {
                if let Some(old) = current.take() {
                    let _ = old.dispose().await;
                }
                current = Some(mount(&root, channel, started));
            }
        }
        "dial" => {
            root.provide(LocalId(PeerId::new("mac")?))?;
            let trust = match &args.ca {
                Some(ca) => Trust::only(read(ca)),
                None => Trust::default(),
            };
            let (address, credential) =
                (args.address.clone(), Credential::Bearer(args.token.clone()));
            let dialing = root.clone();
            tokio::spawn(async move {
                loop {
                    match websocket::dial(
                        &address,
                        &protocol,
                        Some(&credential),
                        &trust,
                        &Limits::default(),
                    )
                    .await
                    {
                        Ok(channel) => {
                            let view = mount(&dialing, channel, started);
                            ended(&view).await;
                            let _ = view.dispose().await;
                        }
                        Err(error) => println!("[{:>7.1?}] dial: {error}", started.elapsed()),
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });
            // Until Ctrl-C ends it.
            let mut ticks = tokio::time::interval(Duration::from_secs(1));
            loop {
                ticks.tick().await;
                let Some(clock) = root.get_at::<dyn HostDispatch>(&host_key("clock")) else {
                    println!("[{:>7.1?}] clock: not here", started.elapsed());
                    continue;
                };
                let sent = Instant::now();
                let now =
                    tokio::task::spawn_blocking(move || clock.invoke("now", Value::List(vec![])))
                        .await?;
                match now.and_then(Value::json) {
                    Ok(now) => println!(
                        "[{:>7.1?}] clock: {now} in {:.1?}",
                        started.elapsed(),
                        sent.elapsed()
                    ),
                    Err(error) => println!("[{:>7.1?}] clock: {error}", started.elapsed()),
                }
            }
        }
        _ => return Err("the mode is listen or dial".into()),
    }
    Ok(())
}
