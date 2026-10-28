//! The channel contract (`rutis_bridge::channel::testing::contract`) through
//! the Bun runtime's channel: a Bun process dials two Unix sockets with it and
//! relays between them, so the two Rust ends see each other through Bun's
//! framing, its message limit, backpressure and how it ends.
// Unix only (Q7.8): Bun on Windows is not a declared platform, and the
// relay dials Unix sockets.
#![cfg(all(unix, feature = "bun", feature = "testing"))]

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis_bridge::channel::{Channel, Closer};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Shut(UnixStream);

impl Closer for Shut {
    fn close(&self, _: rutis_bridge::Close) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

fn framed(stream: UnixStream) -> Channel {
    let reader = stream.try_clone().unwrap();
    let closer = Arc::new(Shut(stream.try_clone().unwrap()));
    rutis_bridge::pipe::framed(reader, stream, closer)
}

/// Two Rust channels joined through a Bun relay.
fn pair() -> (Channel, Channel) {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("a.sock"), dir.path().join("b.sock"));
    let (a, b) = (
        UnixListener::bind(&first).unwrap(),
        UnixListener::bind(&second).unwrap(),
    );
    let relay = std::process::Command::new("bun")
        .args(["--no-install", "--no-env-file"])
        .arg(repo().join("bun/rutis-bun/test/fixtures/channel-relay.ts"))
        .args([&first, &second])
        .current_dir(repo().join("bun/rutis-bun"))
        .spawn()
        .unwrap();
    let (a, _) = a.accept().unwrap();
    let (b, _) = b.accept().unwrap();
    // The relay ends with its channels; its directory goes with it.
    std::thread::spawn(move || {
        let mut relay = relay;
        let _ = relay.wait();
        drop(dir);
    });
    (framed(a), framed(b))
}

#[test]
fn bun_channels_meet_the_channel_contract() {
    rutis_bridge::channel::testing::contract(pair);
}
