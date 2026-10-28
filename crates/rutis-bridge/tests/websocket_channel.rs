//! `websocket::dial` and `websocket::listen`: a channel from each end, the
//! listener's admission deciding 401 / 403 / 503, and each `Close` read by
//! the far end as its close code says (1000, 4002, 4003).
#![cfg(feature = "websocket")]

use std::sync::{mpsc, Arc, Mutex};

use rutis_bridge::channel::{Channel, ChannelError, PeerId};
use rutis_bridge::websocket::{
    self, Admit, Admitted, Limits, Listener, ListenerConfig, Refused, Trust, Upgrade,
};
use rutis_bridge::{Close, ConnectError, Credential};
use rutis_test_support::recv_within;

const PROTOCOL: &str = "rutis.3";

/// A loopback listener letting through token `good` (its channels arrive
/// on the receiver), turning token `later` away with 503 and any other
/// with 403.
fn listening() -> (Listener, String, mpsc::Receiver<Channel>) {
    let (deliver, delivered) = mpsc::channel();
    let deliver = Mutex::new(deliver);
    let admit: Admit = Arc::new(move |upgrade: Upgrade<'_>| match upgrade.bearer {
        Some("good") => {
            let deliver = deliver.lock().unwrap().clone();
            Ok(Admitted {
                protocol: PROTOCOL.into(),
                deliver: Box::new(move |channel| {
                    let _ = deliver.send(channel);
                }),
            })
        }
        Some("later") => Err(Refused::Unavailable("not yet".into())),
        _ => Err(Refused::Forbidden("not accepted here".into())),
    });
    let config = ListenerConfig::new(
        "public",
        "127.0.0.1:0".parse().unwrap(),
        PeerId::new("main").unwrap(),
    );
    let listener = websocket::listen(&config, &Limits::default(), admit).unwrap();
    let address = format!("ws://{}/rutis", listener.local_addr());
    (listener, address, delivered)
}

async fn dial(address: &str, token: Option<&str>) -> Result<Channel, ConnectError> {
    let credential = token.map(|token| Credential::Bearer(token.into()));
    websocket::dial(
        address,
        PROTOCOL,
        credential.as_ref(),
        &Trust::default(),
        &Limits::default(),
    )
    .await
}

#[tokio::test]
async fn a_dialed_channel_reaches_the_listener() {
    let (_listener, address, delivered) = listening();
    let mut dialed = dial(&address, Some("good")).await.unwrap();
    let mut accepted = recv_within("the accepted channel", &delivered);
    dialed.sender.send(b"hello").unwrap();
    let received = tokio::task::spawn_blocking(move || accepted.receiver.recv())
        .await
        .unwrap();
    assert_eq!(received, Ok(Some(b"hello".to_vec())));
}

#[tokio::test]
async fn the_admission_decides_how_an_upgrade_is_refused() {
    let (_listener, address, _delivered) = listening();
    assert!(
        matches!(dial(&address, None).await, Err(ConnectError::AuthRejected { reason }) if reason.contains("401")),
        "no credentials: 401"
    );
    assert!(
        matches!(dial(&address, Some("bad")).await, Err(ConnectError::AuthRejected { reason }) if reason.contains("403")),
        "refused credentials: 403"
    );
    assert!(
        matches!(dial(&address, Some("later")).await, Err(ConnectError::Retryable { reason }) if reason.contains("503")),
        "nobody takes them yet: 503"
    );
}

/// What the dialing side's next receive gives once the accepted side
/// closed with `why`.
async fn far_end_reads(why: Close) -> Result<Option<Vec<u8>>, ChannelError> {
    let (_listener, address, delivered) = listening();
    let mut dialed = dial(&address, Some("good")).await.unwrap();
    let accepted = recv_within("the accepted channel", &delivered);
    accepted.closer.close(why);
    tokio::task::spawn_blocking(move || dialed.receiver.recv())
        .await
        .unwrap()
}

#[tokio::test]
async fn each_close_reaches_the_far_end_as_its_close_code() {
    assert_eq!(far_end_reads(Close::Done).await, Ok(None), "1000");
    assert_eq!(
        far_end_reads(Close::Replaced).await,
        Err(ChannelError::Closed {
            reason: "replaced by a new connection".into()
        }),
        "4002"
    );
    assert_eq!(
        far_end_reads(Close::Rejected("bad id".into())).await,
        Err(ChannelError::Rejected {
            reason: "rejected: bad id".into()
        }),
        "4003"
    );
}
