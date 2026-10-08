//! `spawn:<name>` with `Handover::Loopback`, how processes start on Windows
//! (and anywhere): the process dials a loopback address and presents a
//! one-time token; anything else connecting there is turned away. The
//! channel still owns the process.
use rutis_bridge::channel::{ChannelError, PeerId};
use rutis_bridge::transport::local::{Handover, LocalTransport, Spawn, CHANNEL_TOKEN};
use rutis_bridge::{Dial, Transport};

fn python() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// A Python script on a loopback channel: `channel` is the `tcp:` address,
/// `token` the token to present, `rest` what follows.
fn script(body: &str) -> Spawn {
    let mut spawn = Spawn::new(python(), PeerId::new("child").unwrap());
    let prelude = format!(
        "import os, socket, sys, time\n\
         host, _, port = sys.argv[1][len('tcp:'):].rpartition(':')\n\
         token = os.environ['{CHANNEL_TOKEN}']\n\
         def connect(present):\n    \
             s = socket.create_connection((host, int(port)))\n    \
             s.sendall(present.encode() + b'\\n')\n    \
             return s\n"
    );
    spawn.args = vec!["-c".into(), format!("{prelude}{body}").into()];
    spawn.handover = Handover::Loopback;
    spawn.trailing = vec!["extra".into()];
    spawn
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_dials_back_with_its_token_and_its_exit_ends_the_channel() {
    let transport = LocalTransport::default();
    // An impostor without the token first: turned away. Then the process
    // answers one line with the line and its last argument, and exits 7.
    transport.spawner(
        "echo",
        script(
            "impostor = connect('not the token')\n\
             s = connect(token)\n\
             line = s.makefile('rb').readline().strip()\n\
             s.sendall(line + b' ' + sys.argv[2].encode() + b'\\n')\n\
             sys.exit(7)",
        ),
    );
    let mut channel = transport.dial(&Dial::address("spawn:echo")).await.unwrap();
    assert_eq!(channel.info.transport, "tcp");
    channel.sender.send(b"hello").unwrap();
    let (reply, end) = tokio::task::spawn_blocking(move || {
        let reply = channel.receiver.recv().unwrap().unwrap();
        (reply, channel.receiver.recv())
    })
    .await
    .unwrap();
    assert_eq!(reply, b"hello extra");
    match end {
        Err(ChannelError::Closed { reason }) => {
            assert!(reason.starts_with("the process exited with"), "{reason}");
            assert!(reason.ends_with('7'), "{reason}");
        }
        other => panic!("the exit should end the channel, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_channel_ends_the_process() {
    let transport = LocalTransport::default();
    transport.spawner("idle", script("s = connect(token)\ntime.sleep(30)"));
    let mut channel = transport.dial(&Dial::address("spawn:idle")).await.unwrap();
    transport.close_all();
    // It does not end by itself: killed after the grace period, and the
    // channel's end says so.
    let end = tokio::task::spawn_blocking(move || channel.receiver.recv())
        .await
        .unwrap();
    match end {
        Err(ChannelError::Closed { reason }) => {
            assert!(reason.starts_with("the process exited with"), "{reason}")
        }
        other => panic!("closing should end the process, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_that_exits_before_connecting_says_so() {
    let transport = LocalTransport::default();
    transport.spawner("quits", script("sys.exit(3)"));
    match transport.dial(&Dial::address("spawn:quits")).await {
        Err(error) => assert!(
            error.to_string().contains("exited before connecting"),
            "{error}"
        ),
        Ok(_) => panic!("a process that never connects has no channel"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_connection_does_not_hold_up_the_process() {
    let transport = LocalTransport::default();
    // Something connects first and never presents a token; the process,
    // right behind it, is still taken at once.
    transport.spawner(
        "behind",
        script(
            "silent = socket.create_connection((host, int(port)))\n\
             time.sleep(0.2)\n\
             s = connect(token)\n\
             time.sleep(30)",
        ),
    );
    let started = std::time::Instant::now();
    let channel = transport
        .dial(&Dial::address("spawn:behind"))
        .await
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
    drop(channel);
    transport.close_all();
}
