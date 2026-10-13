//! The WebSocket transport against the channel contract and its binding:
//! TLS, authentication, routing, subprotocols, limits, heartbeats, close
//! codes and unloading.
#![cfg(all(feature = "websocket", feature = "testing"))]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use rutis::Ctx;
use rutis_bridge::channel::{Channel, ConnectError, PeerId};
use rutis_bridge::transport::websocket::{
    Config, Limits, ListenerConfig, ServerTls, Trust, WebSocketPlugin, WebSocketTransport,
};
use rutis_bridge::{
    fingerprint, transport_key, Credential, Dial, Identity, Registered, Registration,
    StaticIdentity, Transport,
};
use rutis_test_support::{eventually_blocking, recv_within};

const PROTOCOL: &str = "rutis.2";

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

/// A transport whose loopback listener `public` serves endpoint `main`.
fn server(limits: Limits) -> Arc<WebSocketTransport> {
    WebSocketTransport::start(
        Config::new()
            .listener(ListenerConfig::new("public", loopback(), id("main")))
            .limits(limits),
    )
    .unwrap()
}

/// A dialing transport that trusts only `ca` (or the system when `None`).
fn client(limits: Limits) -> Arc<WebSocketTransport> {
    WebSocketTransport::start(Config::new().limits(limits)).unwrap()
}

/// Accept `peer` with `token` on `public`; accepted channels arrive on the
/// returned receiver.
fn accept(
    transport: &WebSocketTransport,
    peer: &str,
    token: &str,
) -> (Registered, mpsc::Receiver<Channel>) {
    let (sender, accepted) = mpsc::channel();
    let sender = Mutex::new(sender);
    let identity = StaticIdentity::new(id("main")).accept_token(token, id(peer));
    let registered = transport
        .register(Registration {
            listener: "public".into(),
            peer: id(peer),
            identity: Arc::new(identity),
            protocol: PROTOCOL.into(),
            deliver: Box::new(move |channel| {
                let _ = sender.lock().unwrap().send(channel);
            }),
        })
        .unwrap();
    (registered, accepted)
}

/// `peer` dialing endpoint `main` with `token`.
fn dial_as(address: String, peer: &str, token: &str) -> Dial {
    let identity: Arc<dyn Identity> = Arc::new(
        StaticIdentity::new(id(peer)).present(id("main"), Credential::Bearer(token.into())),
    );
    Dial::address(address)
        .peer(id("main"))
        .identity(identity)
        .protocol(PROTOCOL)
}

fn address(transport: &WebSocketTransport) -> String {
    format!("ws://{}/rutis", transport.local_addr("public").unwrap())
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn meets_the_channel_contract() {
    let server = server(Limits::default());
    let client = client(Limits::default());
    let count = AtomicUsize::new(0);
    let kept = Mutex::new(Vec::new());
    rutis_bridge::channel::testing::contract(|| {
        let peer = format!("peer-{}", count.fetch_add(1, Ordering::SeqCst));
        let (registered, accepted) = accept(&server, &peer, &peer);
        let dialed = block_on(client.dial(&dial_as(address(&server), &peer, &peer))).unwrap();
        let accepted = recv_within("the accepted connection", &accepted);
        assert_eq!(dialed.info.peer, Some(id("main")));
        assert_eq!(accepted.info.peer, Some(id(&peer)));
        kept.lock().unwrap().push(registered);
        (dialed, accepted)
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn authentication_routing_and_compatibility_failures_carry_their_category() {
    let server = server(Limits::default());
    let client = client(Limits::default());
    let (_mac, _) = accept(&server, "mac", "mac-token");

    let wrong_token = client
        .dial(&dial_as(address(&server), "mac", "guess"))
        .await;
    assert!(
        matches!(wrong_token, Err(ConnectError::AuthRejected { .. })),
        "{wrong_token:?}"
    );
    let no_credential = client
        .dial(&Dial::address(address(&server)).protocol(PROTOCOL))
        .await;
    assert!(matches!(
        no_credential,
        Err(ConnectError::AuthRejected { .. })
    ));
    // A valid token of a far end nobody registered is refused the same way.
    let (pi, _) = accept(&server, "pi", "pi-token");
    drop(pi);
    let revoked = client
        .dial(&dial_as(address(&server), "pi", "pi-token"))
        .await;
    assert!(matches!(revoked, Err(ConnectError::AuthRejected { .. })));

    let other_protocol = client
        .dial(&dial_as(address(&server), "mac", "mac-token").protocol("rutis.99"))
        .await;
    // The refusal says which end to upgrade (standard Q6.2.4).
    // guarantee: bridge/version-refusal
    match &other_protocol {
        Err(ConnectError::Incompatible { reason }) => assert!(
            reason.ends_with(&format!(
                "this endpoint speaks {PROTOCOL}, the dialer offered rutis.99: \
                 upgrade this endpoint (Rust crates rutis-host / rutis-bridge)"
            )),
            "{reason}"
        ),
        other => panic!("{other:?}"),
    }
    let older = client
        .dial(&dial_as(address(&server), "mac", "mac-token").protocol("rutis.1"))
        .await;
    match &older {
        Err(ConnectError::Incompatible { reason }) => assert!(
            reason.ends_with(&format!(
                "this endpoint speaks {PROTOCOL}, the dialer offered rutis.1: upgrade the dialer"
            )),
            "{reason}"
        ),
        other => panic!("{other:?}"),
    }
    let wrong_path = client
        .dial(&Dial {
            address: address(&server).replace("/rutis", "/other"),
            ..dial_as(String::new(), "mac", "mac-token")
        })
        .await;
    assert!(
        matches!(wrong_path, Err(ConnectError::Incompatible { .. })),
        "{wrong_path:?}"
    );
    let plain_remote = client
        .dial(&dial_as(
            "ws://example.com/rutis".into(),
            "mac",
            "mac-token",
        ))
        .await;
    assert!(matches!(
        plain_remote,
        Err(ConnectError::Incompatible { .. })
    ));
    let free = std::net::TcpListener::bind(loopback())
        .unwrap()
        .local_addr()
        .unwrap();
    let nobody = client
        .dial(&dial_as(format!("ws://{free}/rutis"), "mac", "mac-token"))
        .await;
    assert!(
        matches!(nobody, Err(ConnectError::Retryable { .. })),
        "{nobody:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn one_registration_per_far_end_and_unknown_listeners_are_refused() {
    let server = server(Limits::default());
    let (_mac, _) = accept(&server, "mac", "a");
    let identity: Arc<dyn Identity> = Arc::new(StaticIdentity::new(id("main")));
    let duplicate = server.register(Registration {
        listener: "public".into(),
        peer: id("mac"),
        identity: identity.clone(),
        protocol: PROTOCOL.into(),
        deliver: Box::new(|_| {}),
    });
    assert!(matches!(
        duplicate,
        Err(rutis_bridge::RegistrationError::Duplicate(_))
    ));
    let nowhere = server.register(Registration {
        listener: "private".into(),
        peer: id("pi"),
        identity,
        protocol: PROTOCOL.into(),
        deliver: Box::new(|_| {}),
    });
    assert!(matches!(
        nowhere,
        Err(rutis_bridge::RegistrationError::NoListener(_))
    ));
}

/// A CA, a server certificate for `localhost` and a client certificate.
struct Pki {
    ca_pem: String,
    server: ServerTls,
    client_chain: String,
    client_key: String,
    client_der: Vec<u8>,
}

fn pki() -> Pki {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    // Distinct names: OpenSSL takes a certificate whose issuer is its own
    // subject for self-signed.
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "rutis test CA");
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);
    let server_key = KeyPair::generate().unwrap();
    let server = CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .signed_by(&server_key, &issuer)
        .unwrap();
    let client_key = KeyPair::generate().unwrap();
    let client = CertificateParams::new(vec!["mac".into()])
        .unwrap()
        .signed_by(&client_key, &issuer)
        .unwrap();
    Pki {
        ca_pem: ca.pem(),
        server: ServerTls {
            certificate_pem: server.pem().into_bytes(),
            key_pem: server_key.serialize_pem().into_bytes(),
            client_ca_pem: Some(ca.pem().into_bytes()),
        },
        client_chain: client.pem(),
        client_key: client_key.serialize_pem(),
        client_der: client.der().to_vec(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn wss_verifies_the_server_and_accepts_client_certificates() {
    let pki = pki();
    let server =
        WebSocketTransport::start(Config::new().listener(
            ListenerConfig::new("public", loopback(), id("main")).tls(pki.server.clone()),
        ))
        .unwrap();
    let address = format!(
        "wss://localhost:{}/rutis",
        server.local_addr("public").unwrap().port()
    );

    // A bearer token over TLS, the server verified against our CA.
    let (_mac, accepted) = accept(&server, "mac", "mac-token");
    let trusting = WebSocketTransport::start(
        Config::new().trust(Trust::only(pki.ca_pem.clone().into_bytes())),
    )
    .unwrap();
    let mut dialed = trusting
        .dial(&dial_as(address.clone(), "mac", "mac-token"))
        .await
        .unwrap();
    let mut accepted = recv_within("the accepted connection", &accepted);
    dialed.sender.send(b"{\"over\":\"tls\"}").unwrap();
    assert_eq!(
        accepted.receiver.recv().unwrap().unwrap(),
        b"{\"over\":\"tls\"}"
    );

    // A server certificate from an unknown CA is an authentication failure.
    let system = WebSocketTransport::start(Config::new().trust(Trust {
        system: true,
        ca_pem: Vec::new(),
    }))
    .unwrap();
    let untrusted = system
        .dial(&dial_as(address.clone(), "mac", "mac-token"))
        .await;
    assert!(
        matches!(untrusted, Err(ConnectError::AuthRejected { .. })),
        "{untrusted:?}"
    );

    // A client certificate proves the far end without a token.
    let (sender, accepted) = mpsc::channel();
    let sender = Mutex::new(sender);
    let _pi = server
        .register(Registration {
            listener: "public".into(),
            peer: id("pi"),
            identity: Arc::new(
                StaticIdentity::new(id("main"))
                    .accept_certificate(fingerprint(&pki.client_der), id("pi")),
            ),
            protocol: PROTOCOL.into(),
            deliver: Box::new(move |channel| {
                let _ = sender.lock().unwrap().send(channel);
            }),
        })
        .unwrap();
    let identity: Arc<dyn Identity> = Arc::new(StaticIdentity::new(id("pi")).present(
        id("main"),
        Credential::ClientCertificate {
            chain_pem: pki.client_chain.clone().into_bytes(),
            key_pem: pki.client_key.clone().into_bytes(),
        },
    ));
    let _dialed = trusting
        .dial(
            &Dial::address(address)
                .peer(id("main"))
                .identity(identity)
                .protocol(PROTOCOL),
        )
        .await
        .unwrap();
    let accepted = recv_within("the accepted connection", &accepted);
    assert_eq!(accepted.info.peer, Some(id("pi")));
}

// Negative TLS (risk E6): a certificate that is expired, not valid yet or
// for another name is refused like one from an unknown CA, as an
// authentication failure, and the bearer token never shows in the reason.
// Validity periods are fixed dates far from any real clock, so these tests
// do not depend on when they run.

/// A token the refusals must never repeat (B7).
const SECRET: &str = "e6-token-must-not-leak";

type Validity = ((i32, u8, u8), (i32, u8, u8));
/// Long expired, and not valid until long from now.
const EXPIRED: Validity = ((2000, 1, 1), (2001, 1, 1));
const NOT_YET_VALID: Validity = ((2100, 1, 1), (2101, 1, 1));

/// A CA for the negative cases.
struct TestCa {
    pem: String,
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
}

fn test_ca() -> TestCa {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    // What strict verifiers (OpenSSL's X509_STRICT, the default in Python
    // 3.13) require of a CA.
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "rutis negative TLS CA");
    let pem = params.self_signed(&key).unwrap().pem();
    TestCa {
        pem,
        issuer: Issuer::new(params, key),
    }
}

/// A certificate and its key.
struct Leaf {
    chain_pem: String,
    key_pem: String,
    der: Vec<u8>,
}

/// A certificate for `names` signed by `ca`, valid over `validity` (rcgen's
/// default, 1975 to 4096, when `None`).
fn leaf(ca: &TestCa, names: &[&str], validity: Option<Validity>) -> Leaf {
    let key = rcgen::KeyPair::generate().unwrap();
    let names = names
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    let mut params = rcgen::CertificateParams::new(names).unwrap();
    params.use_authority_key_identifier_extension = true;
    if let Some(((y0, m0, d0), (y1, m1, d1))) = validity {
        params.not_before = rcgen::date_time_ymd(y0, m0, d0);
        params.not_after = rcgen::date_time_ymd(y1, m1, d1);
    }
    let certificate = params.signed_by(&key, &ca.issuer).unwrap();
    Leaf {
        chain_pem: certificate.pem(),
        key_pem: key.serialize_pem(),
        der: certificate.der().to_vec(),
    }
}

/// A TLS listener serving `certificate`, client certificates checked
/// against `ca`, and the address to dial it by through `host`.
fn tls_server(ca: &TestCa, certificate: &Leaf, host: &str) -> (Arc<WebSocketTransport>, String) {
    let tls = ServerTls {
        certificate_pem: certificate.chain_pem.clone().into_bytes(),
        key_pem: certificate.key_pem.clone().into_bytes(),
        client_ca_pem: Some(ca.pem.clone().into_bytes()),
    };
    let server = WebSocketTransport::start(
        Config::new().listener(ListenerConfig::new("public", loopback(), id("main")).tls(tls)),
    )
    .unwrap();
    let address = format!(
        "wss://{host}:{}/rutis",
        server.local_addr("public").unwrap().port()
    );
    (server, address)
}

/// A dialing transport that trusts only `ca`.
fn trusting(ca: &TestCa) -> Arc<WebSocketTransport> {
    WebSocketTransport::start(Config::new().trust(Trust::only(ca.pem.clone().into_bytes())))
        .unwrap()
}

/// The reason of an authentication failure, which must not contain the
/// token; anything else fails the test.
fn auth_rejected(result: Result<Channel, ConnectError>) -> String {
    match result {
        Err(ConnectError::AuthRejected { reason }) => {
            assert!(!reason.contains(SECRET), "the token leaked: {reason}");
            reason
        }
        other => panic!("expected AuthRejected, got {other:?}"),
    }
}

/// A server certificate outside its validity period is refused (before
/// the token is sent), with a reason that names the problem.
/// risk: E6
/// guarantee: bridge/tls-server-verified
#[tokio::test(flavor = "multi_thread")]
async fn wss_refuses_an_expired_or_not_yet_valid_server_certificate() {
    let ca = test_ca();
    let client = trusting(&ca);
    for (validity, expected) in [(EXPIRED, "expired"), (NOT_YET_VALID, "not valid yet")] {
        let certificate = leaf(&ca, &["localhost"], Some(validity));
        let (server, address) = tls_server(&ca, &certificate, "localhost");
        let (_mac, _accepted) = accept(&server, "mac", SECRET);
        let reason = auth_rejected(client.dial(&dial_as(address, "mac", SECRET)).await);
        assert!(reason.contains(expected), "{expected}: {reason}");
    }
}

/// A server certificate for another name is refused: one for
/// `other.example` when dialing `localhost`, and one for `localhost` when
/// dialing `127.0.0.1` (an IP address matches only an IP address entry).
/// risk: E6
/// guarantee: bridge/tls-server-verified
#[tokio::test(flavor = "multi_thread")]
async fn wss_refuses_a_server_certificate_for_another_name() {
    let ca = test_ca();
    let client = trusting(&ca);
    for (name, host) in [("other.example", "localhost"), ("localhost", "127.0.0.1")] {
        let certificate = leaf(&ca, &[name], None);
        let (server, address) = tls_server(&ca, &certificate, host);
        let (_mac, _accepted) = accept(&server, "mac", SECRET);
        let reason = auth_rejected(client.dial(&dial_as(address, "mac", SECRET)).await);
        assert!(reason.contains("not valid for name"), "{host}: {reason}");
    }
}

/// An expired client certificate does not authenticate, even with the
/// accepted fingerprint: the first connection the listener delivers is
/// the one dialed afterwards with a token.
/// risk: E6
/// guarantee: bridge/tls-client-certificate-verified
#[tokio::test(flavor = "multi_thread")]
async fn wss_refuses_an_expired_client_certificate() {
    let ca = test_ca();
    let (server, address) = tls_server(&ca, &leaf(&ca, &["localhost"], None), "localhost");
    let client = trusting(&ca);
    let expired = leaf(&ca, &["pi"], Some(EXPIRED));

    // Both registrations deliver to one queue, in order.
    let (sender, delivered) = mpsc::channel();
    let sender = Arc::new(Mutex::new(sender));
    let deliver = |sender: Arc<Mutex<mpsc::Sender<Channel>>>| -> rutis_bridge::Deliver {
        Box::new(move |channel| {
            let _ = sender.lock().unwrap().send(channel);
        })
    };
    let pi =
        StaticIdentity::new(id("main")).accept_certificate(fingerprint(&expired.der), id("pi"));
    let _pi = server
        .register(Registration {
            listener: "public".into(),
            peer: id("pi"),
            identity: Arc::new(pi),
            protocol: PROTOCOL.into(),
            deliver: deliver(sender.clone()),
        })
        .unwrap();
    let mac = StaticIdentity::new(id("main")).accept_token(SECRET, id("mac"));
    let _mac = server
        .register(Registration {
            listener: "public".into(),
            peer: id("mac"),
            identity: Arc::new(mac),
            protocol: PROTOCOL.into(),
            deliver: deliver(sender),
        })
        .unwrap();

    let identity: Arc<dyn Identity> = Arc::new(StaticIdentity::new(id("pi")).present(
        id("main"),
        Credential::ClientCertificate {
            chain_pem: expired.chain_pem.clone().into_bytes(),
            key_pem: expired.key_pem.clone().into_bytes(),
        },
    ));
    let refused = client
        .dial(
            &Dial::address(address.clone())
                .peer(id("main"))
                .identity(identity)
                .protocol(PROTOCOL),
        )
        .await;
    let reason = auth_rejected(refused);
    assert!(reason.contains("CertificateExpired"), "{reason}");

    let _dialed = client.dial(&dial_as(address, "mac", SECRET)).await.unwrap();
    let first = recv_within("the accepted connection", &delivered);
    assert_eq!(first.info.peer, Some(id("mac")));
}

/// Writes the certificates the Python WebSocket tests serve
/// (`python/rutis/tests/tls/`, see its README): Python has no certificate
/// generator without a new dependency, so they are committed. Run with
/// `cargo test -p rutis-bridge --all-features --test websocket -- --ignored
/// regenerate_python_tls_fixtures`. Keys are new on every run; the Python
/// tests check the properties that matter (who signed, names, validity).
#[test]
#[ignore = "writes the Python test certificates; run by hand to regenerate them"]
fn regenerate_python_tls_fixtures() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/rutis/tests/tls");
    std::fs::create_dir_all(&directory).unwrap();
    let ca = test_ca();
    std::fs::write(directory.join("ca.pem"), &ca.pem).unwrap();
    for (name, names, validity) in [
        ("localhost", &["localhost"][..], None),
        ("expired", &["localhost"][..], Some(EXPIRED)),
        ("not-yet-valid", &["localhost"][..], Some(NOT_YET_VALID)),
        ("other-name", &["other.example"][..], None),
    ] {
        let certificate = leaf(&ca, names, validity);
        std::fs::write(directory.join(format!("{name}.pem")), certificate.chain_pem).unwrap();
        std::fs::write(directory.join(format!("{name}.key")), certificate.key_pem).unwrap();
    }
}

/// guarantee: host/listen-tls
#[test]
fn listeners_off_loopback_need_tls() {
    let open = Config::new().listener(ListenerConfig::new(
        "public",
        "0.0.0.0:0".parse().unwrap(),
        id("main"),
    ));
    assert!(open.validate().unwrap_err().contains("without TLS"));
}

/// Starting in an async context (as a plugin does), a CA that is not a
/// certificate and a port another listener holds are errors, not panics.
/// risk: B6
#[test]
fn starting_with_a_bad_ca_or_a_taken_port_fails_without_panicking() {
    let started = std::thread::spawn(|| {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            WebSocketTransport::start(Config::new().trust(Trust::only(b"not a PEM".to_vec()))).err()
        })
    })
    .join()
    .expect("no panic");
    assert_eq!(started.as_deref(), Some("no certificate in PEM"));

    let taken = std::net::TcpListener::bind(loopback()).unwrap();
    let address = taken.local_addr().unwrap();
    let error = WebSocketTransport::start(Config::new().listener(ListenerConfig::new(
        "public",
        address,
        id("main"),
    )))
    .err()
    .unwrap();
    assert!(
        error.starts_with(&format!("listener public: cannot bind {address}: "))
            && error.ends_with("another program listens there: stop it, or change the address"),
        "{error}"
    );
}

/// A pair of channels over a real connection between two transports.
fn connected(
    server: &WebSocketTransport,
    client: &WebSocketTransport,
) -> (Channel, Channel, Registered) {
    let (registered, accepted) = accept(server, "mac", "mac-token");
    let dialed = block_on(client.dial(&dial_as(address(server), "mac", "mac-token"))).unwrap();
    let accepted = recv_within("the accepted connection", &accepted);
    (dialed, accepted, registered)
}

#[test]
fn messages_over_the_limit_close_the_channel_either_way() {
    let small = Limits {
        max_message: 1024,
        ..Limits::default()
    };
    // Sending too much: refused, and the channel closes (1009).
    let big_server = server(Limits::default());
    let small_client = client(small.clone());
    let (mut dialed, mut accepted, _r) = connected(&big_server, &small_client);
    assert!(dialed.sender.send(&[b'x'; 2048]).is_err());
    let ended = accepted.receiver.recv();
    assert!(
        matches!(&ended, Err(rutis_bridge::channel::ChannelError::Closed { reason }) if reason.contains("1009")),
        "{ended:?}"
    );

    // Receiving too much: the receiving side closes with 1009.
    let small_server = server_with(small);
    let big_client = client(Limits::default());
    let (mut dialed, mut accepted, _r) = connected(&small_server, &big_client);
    dialed.sender.send(&[b'y'; 4096]).unwrap();
    assert!(accepted.receiver.recv().is_err());
    let ended = dialed.receiver.recv();
    assert!(
        matches!(&ended, Err(rutis_bridge::channel::ChannelError::Closed { reason }) if reason.contains("1009")),
        "{ended:?}"
    );
}

/// Q6.3.2: the shared size-limit check, both ends at the same limit.
#[test]
fn meets_the_size_limit_check() {
    let small = Limits {
        max_message: 1024,
        ..Limits::default()
    };
    let server = server(small.clone());
    let client = client(small);
    let (dialed, accepted, _r) = connected(&server, &client);
    rutis_bridge::channel::testing::size_limit((dialed, accepted), 1024);
}

/// The local transport frames lines with the WebSocket transport's default
/// limit.
#[test]
fn the_local_limit_is_the_websocket_default() {
    assert_eq!(
        rutis_bridge::transport::local::MAX_MESSAGE,
        Limits::default().max_message
    );
}

fn server_with(limits: Limits) -> Arc<WebSocketTransport> {
    server(limits)
}

#[test]
fn a_takeover_closes_the_old_connection_with_4002() {
    let server = server(Limits::default());
    let client = client(Limits::default());
    let (mut dialed, accepted, _r) = connected(&server, &client);
    accepted.closer.replaced();
    let ended = dialed.receiver.recv();
    assert!(
        matches!(&ended, Err(rutis_bridge::channel::ChannelError::Closed { reason }) if reason == "replaced by a new connection"),
        "{ended:?}"
    );
}

#[test]
fn an_orderly_close_reads_as_a_normal_end() {
    let server = server(Limits::default());
    let client = client(Limits::default());
    let (mut dialed, accepted, _r) = connected(&server, &client);
    accepted.closer.close("bye");
    assert!(matches!(dialed.receiver.recv(), Ok(None)));
}

/// A TCP relay that can stop carrying bytes without closing anything: a
/// half-open connection, as a vanished network leaves it. It counts what it
/// carried (each read it passed on).
fn freezable_relay(target: SocketAddr) -> (SocketAddr, Arc<AtomicBool>, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind(loopback()).unwrap();
    let address = listener.local_addr().unwrap();
    let frozen = Arc::new(AtomicBool::new(false));
    let freeze = frozen.clone();
    let carried = Arc::new(AtomicUsize::new(0));
    let count = carried.clone();
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            let upstream = std::net::TcpStream::connect(target).unwrap();
            for (mut from, mut to) in [
                (client.try_clone().unwrap(), upstream.try_clone().unwrap()),
                (upstream, client),
            ] {
                let frozen = freeze.clone();
                let count = count.clone();
                std::thread::spawn(move || {
                    use std::io::{Read, Write};
                    let mut buffer = [0u8; 16 * 1024];
                    loop {
                        let Ok(n) = from.read(&mut buffer) else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        if frozen.load(Ordering::SeqCst) {
                            // Swallow: nothing more reaches the far side.
                            continue;
                        }
                        if to.write_all(&buffer[..n]).is_err() {
                            return;
                        }
                        count.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        }
    });
    (address, frozen, carried)
}

/// Timing is what this tests, across the transport's own threads: by
/// tolerance (docs/design-test-clocks-2026-10-11.md §3.3), ping and timeout
/// ten times apart, "still connected" after a count of pings rather than a
/// time, no bound on how fast the timeout is found (only a hang guard).
#[test]
fn heartbeats_find_a_half_open_connection() {
    let quick = Limits {
        ping: Duration::from_millis(100),
        timeout: Duration::from_millis(1000),
        ..Limits::default()
    };
    let server = server(quick.clone());
    let client = client(quick);
    let (relay, frozen, carried) = freezable_relay(server.local_addr("public").unwrap());
    let (_r, accepted) = accept(&server, "mac", "mac-token");
    let mut dialed =
        block_on(client.dial(&dial_as(format!("ws://{relay}/rutis"), "mac", "mac-token"))).unwrap();
    let mut accepted = recv_within("the accepted connection", &accepted);
    // Alive while pings flow: through more pings and pongs than fit in two
    // timeouts.
    let before = carried.load(Ordering::SeqCst);
    eventually_blocking(
        || (carried.load(Ordering::SeqCst) >= before + 40).then_some(()),
        "pings through the relay",
    );
    dialed.sender.send(b"still here").unwrap();
    assert_eq!(accepted.receiver.recv().unwrap().unwrap(), b"still here");

    frozen.store(true, Ordering::SeqCst);
    for channel in [dialed, accepted] {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut channel = channel;
            let _ = tx.send(channel.receiver.recv());
        });
        let ended = recv_within("the heartbeat timeout", &rx);
        assert!(
            matches!(&ended, Err(rutis_bridge::channel::ChannelError::Closed { reason }) if reason.contains("heartbeat timeout")),
            "{ended:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_plugin_provides_the_transport_and_unloading_closes_everything() {
    let plugin = WebSocketPlugin::new(Config::new().listener(ListenerConfig::new(
        "public",
        loopback(),
        id("main"),
    )))
    .unwrap();
    let handle = plugin.clone();
    let root = Ctx::root().unwrap();
    let view = root.plugin(plugin);
    (&view).await.unwrap();
    let transport = root
        .get_as::<dyn Transport>(transport_key("websocket"))
        .expect("Transport#websocket is provided");
    assert_eq!(transport.kind(), "websocket");
    let running = handle.transport().unwrap();
    let address = address(&running);
    let (_registered, accepted) = accept(&running, "mac", "mac-token");
    let client = client(Limits::default());
    let mut dialed = client
        .dial(&dial_as(address.clone(), "mac", "mac-token"))
        .await
        .unwrap();
    let mut accepted = recv_within("the accepted connection", &accepted);
    drop((transport, running));

    view.dispose().await.unwrap();
    assert!(root
        .get_as::<dyn Transport>(transport_key("websocket"))
        .is_none());
    assert!(handle.transport().is_none());
    // Its channels closed, and the far end saw it.
    assert!(!matches!(accepted.receiver.recv(), Ok(Some(_))));
    assert!(tokio::task::spawn_blocking(move || dialed.receiver.recv())
        .await
        .unwrap()
        .is_ok_and(|end| end.is_none()));
    // Nothing listens any more.
    let refused = client.dial(&dial_as(address, "mac", "mac-token")).await;
    assert!(
        matches!(refused, Err(ConnectError::Retryable { .. })),
        "{refused:?}"
    );
}
