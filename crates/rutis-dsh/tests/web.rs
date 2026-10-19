//! The dsh web UI started by the launcher in a rutis host.
#![cfg(all(unix, dsh_installed))]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::Scripted;
use rutis::{BoxFuture, CordisError, Ctx, Event, EventKey, Listener};
use rutis_dsh::web;
use rutis_test_support::{within, Guard, HANG_GUARD};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

/// Starting dsh: a Node process loading the whole dsh profile cold, on a
/// busy CI machine. Six times [`HANG_GUARD`], as before; only against a hang.
const DSH_START: Guard = Guard(Duration::from_secs(6 * HANG_GUARD.as_secs()));

/// Hands the first event of its kind to the test.
struct First<E>(Mutex<Option<oneshot::Sender<E>>>);

impl<E: Event + Clone> Listener<E> for First<E> {
    fn call<'a>(
        &'a self,
        _: &'a Ctx,
        event: &'a E,
        _: rutis::Call<'a, E::Value>,
    ) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>> {
        if let Some(sender) = self.0.lock().unwrap().take() {
            let _ = sender.send(event.clone());
        }
        Box::pin(async { Ok(None) })
    }
}

fn first<E: Event + Clone>(ctx: &Ctx) -> oneshot::Receiver<E> {
    let (sender, receiver) = oneshot::channel();
    ctx.events()
        .on(ctx, &EventKey::<E>::of(), First(Mutex::new(Some(sender))))
        .unwrap();
    receiver
}

/// Mounts the web profile `profile` on `port` in `ctx`.
async fn start(ctx: &Ctx, profile: &str, port: u16, workspace: &std::path::Path) {
    rutis_dsh::provide_web_aimux(ctx, Arc::new(Scripted::default())).unwrap();
    let view = ctx.plugin(web::Plugin::new(web::Config {
        profile: Some(profile.into()),
        args: Some(vec!["--no-open".into(), "--port".into(), port.to_string()]),
        cwd: Some(workspace.to_string_lossy().into_owned()),
    }));
    (&view).await.unwrap();
}

/// The status line of `GET /`, or `None` when nothing listens.
async fn status(port: u16) -> Option<String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .ok()?;
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await.ok()?;
    response.lines().next().map(str::to_owned)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// How many ports [`started`] tries.
const ATTEMPTS: usize = 5;

/// Mounts the web profile `profile` in a new root context on a free port
/// and waits until it is ready: the context, the port, what it reported.
///
/// dsh takes the port to listen on and reports no address, so the test
/// picks a free one; another process may take it before dsh binds it
/// (#455). A start that fails on a port in use (`EADDRINUSE`, or
/// `EACCES` for a port Windows reserved) is retried on another.
async fn started(
    profile: &str,
    workspace: &std::path::Path,
) -> (Ctx, u16, rutis_dsh::events::Ready) {
    let mut failures = Vec::new();
    for _ in 0..ATTEMPTS {
        let port = free_port();
        let ctx = Ctx::root().unwrap();
        let ready = first::<rutis_dsh::events::Ready>(&ctx);
        let failed = first::<rutis_dsh::events::StartupFailed>(&ctx);
        start(&ctx, profile, port, workspace).await;
        let failure = DSH_START
            .within("dsh started", async {
                tokio::select! {
                    ready = ready => Ok(ready.unwrap()),
                    failed = failed => Err(failed.unwrap().0),
                }
            })
            .await;
        match failure {
            Ok(ready) => return (ctx, port, ready),
            Err(failure) if failure.contains("EADDRINUSE") || failure.contains("EACCES") => {
                ctx.shutdown().await.unwrap();
                failures.push(format!("port {port}: {failure}"));
            }
            Err(failure) => panic!("dsh failed to start: {failure}"),
        }
    }
    panic!("dsh found no free port in {ATTEMPTS} tries: {failures:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_web_ui_starts_reports_failures_and_stops_with_its_mount() {
    // dsh keeps profiles and settings under DSH_HOME; the Node process
    // inherits it. This test binary holds only this test.
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::env::set_var("DSH_HOME", home.path());

    // Starts: the profile is up with the aimux route, and the UI answers
    // (without the startup token it refuses the request).
    let (ctx, port, ready) = started("rutis-web", workspace.path()).await;
    assert!(
        ready.0.iter().any(|provider| provider == "aimux"),
        "{:?}",
        ready.0
    );
    assert!(home.path().join("profiles/rutis-web").is_dir());
    let line = status(port).await.expect("the web UI listens");
    assert!(line.contains(" 401 "), "{line}");

    // A second profile on the same port fails to start, and says why.
    let busy = Ctx::root().unwrap();
    let failed = first::<rutis_dsh::events::StartupFailed>(&busy);
    start(&busy, "rutis-web-busy", port, workspace.path()).await;
    let failed = DSH_START.within("failure reported", failed).await.unwrap();
    assert!(failed.0.contains("EADDRINUSE"), "{}", failed.0);
    busy.shutdown().await.unwrap();

    // An exit request while starting (help, a usage error) reaches rutis
    // with its code, and the mount then stops promptly.
    for (arg, code) in [("--help", 0.0), ("--no-such-option", 1.0)] {
        let quitting = Ctx::root().unwrap();
        let exit = first::<rutis_dsh::events::Exit>(&quitting);
        rutis_dsh::provide_web_aimux(&quitting, Arc::new(Scripted::default())).unwrap();
        let view = quitting.plugin(web::Plugin::new(web::Config {
            profile: Some("rutis-web-quit".into()),
            args: Some(vec!["--no-open".into(), arg.into()]),
            cwd: Some(workspace.path().to_string_lossy().into_owned()),
        }));
        (&view).await.unwrap();
        let exit = DSH_START.within("exit requested", exit).await.unwrap();
        assert_eq!(exit.0, code, "{arg}");
        // Without the launcher disposing the tree at once, the startup it
        // waits for never comes and the mount never stops: a hang, not a
        // slow stop.
        within(&format!("{arg}: the mount stops"), quitting.shutdown())
            .await
            .unwrap();
    }

    // Disposing the mount stops dsh.
    ctx.shutdown().await.unwrap();
    assert_eq!(status(port).await, None);
}
