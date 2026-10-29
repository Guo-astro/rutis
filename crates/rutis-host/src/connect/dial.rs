//! `rutis-host/dial`: dial one address, install a session on the channel,
//! and once it unloaded dial again, backing off by why it ended
//! (`bridge/dial-out-rules`): retryable failures from 0.5 s doubling to
//! 30 s, with jitter; an authentication failure every 30 s; an
//! incompatible far end stops it. A session lasting 60 s resets the
//! backoff. One channel at a time; unloading the row stops dialing and
//! unloads the session.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, DynPlugin, Effect};
use rutis::{Ctx, Error};
use rutis_bridge::websocket::Credential;
use rutis_bridge::websocket::{Limits, Trust};
use rutis_bridge::{Channel, ConnectError};

use super::session::{self, Kind};
use super::{Connections, Print, Tokens};

/// The first wait after a retryable failure; doubled each time, up to
/// `MAX`.
const INITIAL: Duration = Duration::from_millis(500);
const MAX: Duration = Duration::from_secs(30);
/// ± this fraction of each wait, at random.
const JITTER: f64 = 0.2;
/// A session lasting this long resets the backoff.
const STABLE: Duration = Duration::from_secs(60);
/// The wait after an authentication failure.
const REJECTED: Duration = Duration::from_secs(30);

/// One attempt to reach the far end, presenting the row's token.
pub(crate) type Attempt = Arc<
    dyn Fn(Option<Credential>) -> BoxFuture<'static, Result<Channel, ConnectError>> + Send + Sync,
>;

/// Dial `address` over WebSocket, trusting `trust`.
pub(crate) fn websocket(address: String, trust: Trust) -> Attempt {
    Arc::new(move |credential| {
        let (address, trust) = (address.clone(), trust.clone());
        Box::pin(async move {
            let protocol = format!("rutis.{}", rutis_bridge::session::ENDPOINT_PROTOCOL);
            rutis_bridge::websocket::dial(
                &address,
                &protocol,
                credential.as_ref(),
                &trust,
                &Limits::default(),
            )
            .await
        })
    })
}

pub struct DialPlugin {
    attempt: Attempt,
    connections: Connections,
    tokens: Tokens,
    print: Print,
}

impl DialPlugin {
    pub(crate) fn new(
        attempt: Attempt,
        connections: Connections,
        tokens: Tokens,
        print: Print,
    ) -> Self {
        Self {
            attempt,
            connections,
            tokens,
            print,
        }
    }
}

impl DynPlugin for DialPlugin {
    fn name(&self) -> &str {
        super::DIAL
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        Box::pin(async move {
            let row = super::row_id(ctx, super::DIAL)?;
            let credential = (self.tokens)(&row).map(Credential::Bearer);
            self.connections.applied(&row, true);
            let dialing = Dialing {
                ctx: ctx.clone(),
                row: row.clone(),
                attempt: self.attempt.clone(),
                connections: self.connections.clone(),
                print: self.print.clone(),
            };
            let task = tokio::spawn(super::until_unloaded(ctx, dialing.run(credential)));
            let connections = self.connections.clone();
            ctx.effect(move || async move {
                // The session, a child of this row, unloads with it.
                task.abort();
                let _ = task.await;
                connections.removed(&row);
                Ok(())
            })?;
            Ok(Effect::Done)
        })
    }
}

/// Why the connection is not up, by what it means for retrying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    Retryable,
    AuthRejected,
    Incompatible,
}

impl Failure {
    fn of_connect(error: &ConnectError) -> Self {
        match error {
            ConnectError::Retryable { .. } => Self::Retryable,
            ConnectError::AuthRejected { .. } => Self::AuthRejected,
            ConnectError::Incompatible { .. } => Self::Incompatible,
        }
    }

    fn of_session(kind: Kind) -> Self {
        match kind {
            Kind::Lost => Self::Retryable,
            Kind::Rejected => Self::AuthRejected,
            Kind::Incompatible => Self::Incompatible,
        }
    }

    /// The reason as printed: the category in front, but for retryable.
    fn say(self, reason: &str) -> String {
        match self {
            Self::Retryable => reason.to_owned(),
            Self::AuthRejected => format!("auth-rejected: {reason}"),
            Self::Incompatible => format!("incompatible: {reason}"),
        }
    }
}

struct Dialing {
    ctx: Ctx,
    row: String,
    attempt: Attempt,
    connections: Connections,
    print: Print,
}

impl Dialing {
    async fn run(self, credential: Option<Credential>) {
        let mut backoff = Backoff::new();
        loop {
            let (failure, reason) = match (self.attempt)(credential.clone()).await {
                Err(error) => (Failure::of_connect(&error), error.to_string()),
                Ok(channel) => match session::handshake(&self.ctx, channel).await {
                    Err(ended) => (Failure::of_session(ended.kind), ended.reason),
                    Ok(greeted) => {
                        let id = greeted.id().to_owned();
                        let since = tokio::time::Instant::now();
                        let installed = session::install(&self.ctx, greeted);
                        self.connections.connected(&self.row, &id);
                        (self.print)(format!("{}: connected", self.row));
                        let ended = installed.unloaded().await;
                        self.connections.disconnected(&self.row, &id);
                        if since.elapsed() >= STABLE {
                            backoff.reset();
                        }
                        // Only a handshake says rejected or incompatible:
                        // a session that ran and ended is retried.
                        (Failure::Retryable, ended.reason)
                    }
                },
            };
            let said = failure.say(&reason);
            let wait = match failure {
                Failure::Incompatible => {
                    (self.print)(format!("{}: disconnected ({said}); stopped", self.row));
                    return;
                }
                Failure::AuthRejected => REJECTED,
                Failure::Retryable => backoff.next(),
            };
            (self.print)(format!(
                "{}: disconnected ({said}); retrying in {}",
                self.row,
                crate::status::seconds(wait)
            ));
            tokio::time::sleep(wait).await;
        }
    }
}

/// Exponential backoff with jitter.
struct Backoff {
    next: Duration,
    seed: Mutex<u64>,
}

impl Backoff {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9_7f4a_7c15, |elapsed| elapsed.as_nanos() as u64)
            | 1;
        Self {
            next: INITIAL,
            seed: Mutex::new(seed),
        }
    }

    fn reset(&mut self) {
        self.next = INITIAL;
    }

    /// The next wait: the current step ± jitter; the step doubles up to
    /// `MAX`.
    fn next(&mut self) -> Duration {
        let base = self.next;
        self.next = (self.next * 2).min(MAX);
        let unit = {
            // xorshift64: no cryptographic need, only spreading reconnects.
            let mut seed = self.seed.lock().unwrap();
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            (*seed >> 11) as f64 / (1u64 << 53) as f64
        };
        base.mul_f64(1.0 + JITTER * (2.0 * unit - 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::testing::{lines, until, AsRow};
    use rutis::host::CtxHost;
    use std::collections::VecDeque;

    #[test]
    fn backoff_doubles_to_its_cap_within_jitter_and_resets() {
        let mut backoff = Backoff::new();
        let steps: Vec<f64> = (0..10).map(|_| backoff.next().as_secs_f64()).collect();
        let bases = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0, 30.0, 30.0];
        for (step, base) in steps.iter().zip(bases) {
            assert!(
                (base * 0.8..=base * 1.2).contains(step),
                "{step} is not within 20% of {base}"
            );
        }
        backoff.reset();
        assert!(backoff.next() <= Duration::from_millis(600));
    }

    /// What each attempt gives, in order; the far ends of the channels it
    /// gave, and the credentials it was given.
    #[derive(Default)]
    struct Script {
        results: Mutex<VecDeque<Result<Channel, ConnectError>>>,
        presented: Mutex<Vec<Option<Credential>>>,
    }

    fn attempt(script: &Arc<Script>) -> Attempt {
        let script = script.clone();
        Arc::new(move |credential| {
            script.presented.lock().unwrap().push(credential);
            let next = script.results.lock().unwrap().pop_front();
            Box::pin(async move {
                match next {
                    Some(result) => result,
                    None => std::future::pending().await,
                }
            })
        })
    }

    fn refused() -> Result<Channel, ConnectError> {
        Err(ConnectError::Retryable {
            reason: "gpu.example.com:7443: Connection refused".into(),
        })
    }

    /// A channel whose far end greets as `id`; the far end.
    fn greeting(id: &str) -> (Channel, Channel) {
        let (near, mut far) = rutis_bridge::memory::pair();
        let hello = format!(r#"{{"op":"hello","endpoint":"{id}"}}"#);
        far.sender.send(hello.as_bytes()).unwrap();
        (near, far)
    }

    struct Dialed {
        root: Ctx,
        view: rutis::host::FiberView,
        lines: Arc<Mutex<Vec<String>>>,
        connections: Connections,
    }

    fn start(row: &str, script: &Arc<Script>) -> Dialed {
        let root = Ctx::root().unwrap();
        let (lines, print) = lines();
        let connections = Connections::default();
        let tokens: Tokens = Arc::new(|row| (row == "gpu").then(|| "secret".to_owned()));
        let view = root.plugin(AsRow {
            row: row.into(),
            plugin: DialPlugin::new(attempt(script), connections.clone(), tokens, print),
        });
        Dialed {
            root,
            view,
            lines,
            connections,
        }
    }

    impl Dialed {
        fn lines(&self) -> Vec<String> {
            self.lines.lock().unwrap().clone()
        }

        async fn printed(&self, count: usize) -> Vec<String> {
            until(|| self.lines.lock().unwrap().len() >= count).await;
            self.lines()
        }
    }

    /// The wait a line says, in seconds.
    fn waited(line: &str) -> f64 {
        let wait = line.rsplit("retrying in ").next().unwrap();
        wait.trim_end_matches('s').parse().unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn retryable_failures_back_off_doubling_and_present_the_rows_token() {
        let script = Arc::new(Script::default());
        script
            .results
            .lock()
            .unwrap()
            .extend([refused(), refused(), refused()]);
        let dialed = start("gpu", &script);
        let lines = dialed.printed(3).await;
        for (line, base) in lines.iter().zip([0.5, 1.0, 2.0]) {
            assert!(
                line.starts_with(
                    "gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in "
                ),
                "{line}"
            );
            let wait = waited(line);
            assert!(
                (base * 0.8 - 0.05..=base * 1.2 + 0.05).contains(&wait),
                "{line}"
            );
        }
        let presented = script.presented.lock().unwrap().clone();
        assert!(presented.iter().all(
            |credential| matches!(credential, Some(Credential::Bearer(token)) if token == "secret")
        ));
        dialed.root.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_auth_rejection_retries_every_30s_and_an_incompatible_far_end_stops() {
        let script = Arc::new(Script::default());
        script.results.lock().unwrap().extend([
            Err(ConnectError::AuthRejected {
                reason: "gpu.example.com: upgrade refused with 403 Forbidden".into(),
            }),
            Err(ConnectError::Incompatible {
                reason: "this endpoint speaks rutis.3, the dialer offered rutis.4".into(),
            }),
            refused(),
        ]);
        let dialed = start("gpu", &script);
        let lines = dialed.printed(2).await;
        assert_eq!(
            lines,
            [
                "gpu: disconnected (auth-rejected: gpu.example.com: upgrade refused with 403 Forbidden); retrying in 30s",
                "gpu: disconnected (incompatible: this endpoint speaks rutis.3, the dialer offered rutis.4); stopped",
            ]
        );
        // Stopped: no third attempt.
        tokio::time::sleep(Duration::from_secs(120)).await;
        assert_eq!(script.presented.lock().unwrap().len(), 2);
        assert_eq!(dialed.lines().len(), 2);
        dialed.root.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_lasting_60s_resets_the_backoff_and_a_shorter_one_does_not() {
        for (lasting, base) in [(61, 0.5), (5, 2.0)] {
            let script = Arc::new(Script::default());
            let (near, far) = greeting("gpu");
            script
                .results
                .lock()
                .unwrap()
                .extend([refused(), refused(), Ok(near)]);
            let dialed = start("gpu", &script);
            let lines = dialed.printed(3).await;
            assert_eq!(lines[2], "gpu: connected");
            assert_eq!(dialed.connections.rows()[0].connected[0].id, "gpu");
            tokio::time::sleep(Duration::from_secs(lasting)).await;
            far.closer.close(rutis_bridge::Close::Done);
            let lines = dialed.printed(4).await;
            assert!(
                lines[3].starts_with("gpu: disconnected (connection lost); retrying in "),
                "{lines:?}"
            );
            assert!(dialed.connections.rows()[0].connected.is_empty());
            // Two waits came before (0.5 s, 1 s): reset, the next is 0.5 s
            // again; else it goes on to 2 s.
            let wait = waited(&lines[3]);
            assert!(
                (base * 0.8 - 0.05..=base * 1.2 + 0.05).contains(&wait),
                "{lines:?}"
            );
            dialed.root.shutdown().await.unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn unloading_the_row_stops_dialing_and_unloads_the_session() {
        let script = Arc::new(Script::default());
        let (near, far) = greeting("gpu-1");
        script.results.lock().unwrap().extend([Ok(near)]);
        let dialed = start("gpu", &script);
        assert_eq!(dialed.printed(1).await, ["gpu: connected"]);
        dialed.view.dispose().await.unwrap();
        // The far end sees the session close.
        let far_end = std::thread::spawn(move || {
            let mut far = far;
            far.receiver.recv()
        });
        until(|| far_end.is_finished()).await;
        assert_eq!(far_end.join().unwrap(), Ok(None));
        assert!(dialed.connections.rows().is_empty());
        tokio::time::sleep(Duration::from_secs(120)).await;
        assert_eq!(script.presented.lock().unwrap().len(), 1);
        assert_eq!(dialed.lines(), ["gpu: connected"]);
        dialed.root.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_row_without_a_token_dials_without_one() {
        let script = Arc::new(Script::default());
        script.results.lock().unwrap().extend([refused()]);
        let dialed = start("office", &script);
        dialed.printed(1).await;
        assert!(script.presented.lock().unwrap()[0].is_none());
        dialed.root.shutdown().await.unwrap();
    }
}
