//! `pipe::dial`: it dials Unix sockets with newline framing, and classifies
//! failures by kind.
// Unix only (Q7.8): it dials Unix socket paths.
#![cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

use rutis_bridge::pipe;
use rutis_bridge::ConnectError;

#[tokio::test]
async fn dials_unix_sockets_with_newline_framing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("peer.sock");
    let listener = UnixListener::bind(&path).unwrap();

    let address = format!("unix:{}", path.display());
    let mut channel = pipe::dial(&address).await.unwrap();
    let (mut remote, _) = listener.accept().unwrap();
    channel.sender.send(b"{\"op\":\"hello\"}").unwrap();
    let mut line = String::new();
    BufReader::new(remote.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line, "{\"op\":\"hello\"}\n");
    remote.write_all(b"{}\n").unwrap();
    assert_eq!(channel.receiver.recv().unwrap().unwrap(), b"{}");

    // A bare path is a Unix socket too.
    let bare = pipe::dial(path.to_str().unwrap()).await.unwrap();
    drop(bare);

    channel.closer.close(rutis_bridge::Close::Done);
    assert!(matches!(channel.receiver.recv(), Ok(None) | Err(_)));
}

#[tokio::test]
async fn failures_carry_their_category() {
    let missing = tempfile::tempdir().unwrap().path().join("none.sock");
    assert!(matches!(
        pipe::dial(missing.to_str().unwrap()).await,
        Err(ConnectError::Retryable { .. })
    ));
    assert!(matches!(
        pipe::dial("wss://example.com/rutis").await,
        Err(ConnectError::Incompatible { .. })
    ));
    assert!(matches!(
        pipe::dial("fd:3").await,
        Err(ConnectError::Incompatible { .. })
    ));
}
