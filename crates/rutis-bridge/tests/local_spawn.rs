//! `pipe::spawn`: it starts a process on a channel it owns. The
//! process takes the channel as fd 3, or dials a socket path back; the
//! channel's end says how the process ended, and closing it ends the process.
// Unix only (Q7.8): the Inherit (fd:3) and DialBack (socket path)
// handovers, driven by `sh` scripts; Windows has only the loopback
// handover, covered by local_loopback.rs.
#![cfg(unix)]
use rutis_bridge::pipe::{self, Handover, Processes, Spawn};
use rutis_bridge::ConnectError;

fn shell(script: &str) -> Spawn {
    let mut spawn = Spawn::new("sh");
    // `sh -c script <channel> <trailing>`: the channel is $0, the rest $1….
    spawn.args = vec!["-c".into(), script.into()];
    spawn.trailing = vec!["extra".into()];
    spawn
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spawned_process_talks_on_fd_3_and_its_exit_ends_the_channel() {
    // Answers one line with the line and its arguments, then exits 7.
    let echo = shell(r#"read line <&3; echo "$line $0 $1" >&3; exit 7"#);
    let mut channel = pipe::spawn(&echo, &Processes::default()).await.unwrap();

    channel.sender.send(b"hello").unwrap();
    let reply = tokio::task::spawn_blocking(move || {
        let reply = channel.receiver.recv().unwrap().unwrap();
        (reply, channel.receiver.recv())
    })
    .await
    .unwrap();
    assert_eq!(reply.0, b"hello fd:3 extra");
    match reply.1 {
        Err(rutis_bridge::channel::ChannelError::Closed { reason }) => {
            assert_eq!(reason, "the process exited with exit status: 7")
        }
        other => panic!("the exit should end the channel, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_channel_ends_the_process() {
    let mut channel = pipe::spawn(&shell("exec sleep 30"), &Processes::default())
        .await
        .unwrap();
    channel.closer.close(rutis_bridge::Close::Done);
    // It does not end by itself: killed after the grace period, and the
    // channel's end says so.
    let end = tokio::task::spawn_blocking(move || channel.receiver.recv())
        .await
        .unwrap();
    match end {
        Err(rutis_bridge::channel::ChannelError::Closed { reason }) => {
            assert!(reason.contains("signal: 9"), "{reason}")
        }
        other => panic!("closing should end the process, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_without_fd_3_dials_back() {
    let mut spawn = Spawn::new("python3");
    spawn.args = vec![
        "-c".into(),
        "import socket, sys\n\
         s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])\n\
         s.sendall(b'from ' + sys.argv[2].encode() + b'\\n')\n\
         s.recv(1)"
            .into(),
    ];
    spawn.handover = Handover::DialBack;
    spawn.trailing = vec!["anchor".into()];
    let mut channel = pipe::spawn(&spawn, &Processes::default()).await.unwrap();
    let message = tokio::task::spawn_blocking(move || channel.receiver.recv())
        .await
        .unwrap();
    assert_eq!(message.unwrap().unwrap(), b"from anchor");
}

#[tokio::test]
async fn what_cannot_start_is_incompatible() {
    assert!(matches!(
        pipe::spawn(&Spawn::new("/nonexistent/program"), &Processes::default()).await,
        Err(ConnectError::Incompatible { .. })
    ));
}

/// Every `Close` ends a process's channel the same way: its end of the
/// channel closes, and it sees the end of what it reads (its own exit
/// here, before the grace period would kill it).
#[tokio::test(flavor = "multi_thread")]
async fn each_close_ends_the_childs_end_of_the_channel() {
    for why in [
        rutis_bridge::Close::Done,
        rutis_bridge::Close::Replaced,
        rutis_bridge::Close::Rejected("bad id".into()),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("ended");
        // Reads its channel to the end, then leaves the marker.
        let mut spawn = shell(r#"cat <&3 >/dev/null; touch "$1""#);
        spawn.trailing = vec![marker.clone().into()];
        let processes = Processes::default();
        let channel = pipe::spawn(&spawn, &processes).await.unwrap();
        channel.closer.close(why.clone());
        processes.ended().await;
        assert!(marker.exists(), "{why:?}: the process saw its channel end");
    }
}
