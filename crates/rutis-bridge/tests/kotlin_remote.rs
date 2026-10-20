//! The Kotlin SDK's own WebSocket rules (feature `kotlin`, its jvm target:
//! the same code an Android application runs), beyond the conformance
//! columns (`session_matrix` listening, `dial_conformance`): what its
//! listener refuses and with which status, the session protocol refusal's
//! words, broken frames closed with their codes, the heartbeat, a newer
//! controller taking over, TLS on both ends (`RUTIS_CERT` / `RUTIS_KEY`
//! listening, `RUTIS_CA` dialing), and the addresses refused before
//! anything is opened. The same rules as Go's runtime
//! (`go/rutis/listen_test.go`, `dial_test.go`), checked from outside, on
//! the conformance jar.
#![cfg(all(feature = "kotlin", feature = "websocket", feature = "testing"))]

mod kotlin;

use std::sync::Arc;

use rutis_bridge::channel::PeerId;
use rutis_bridge::transport::websocket::{
    Config, ListenerConfig, ServerTls, Trust, WebSocketTransport,
};
use rutis_bridge::{Credential, Dial, Identity, StaticIdentity, Transport};
use rutis_test_support::within;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The runtime, started with `channel` and `env`, its stderr read line by
/// line.
struct Runtime {
    child: tokio::process::Child,
    lines: tokio::sync::mpsc::UnboundedReceiver<String>,
}

impl Runtime {
    fn start(channel: &str, env: &[(&str, &str)]) -> Self {
        let mut command = tokio::process::Command::new("java");
        command
            .args(kotlin::JVM_OPTIONS)
            .arg("-jar")
            .arg(kotlin::jar())
            .arg(channel)
            .args(["--id", "runtime", "--peer", "main"])
            .arg(std::env::temp_dir())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        let mut stderr = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        let (send, lines) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(Some(line)) = stderr.next_line().await {
                eprintln!("[runtime] {line}");
                let _ = send.send(line);
            }
        });
        Self { child, lines }
    }

    /// The next line starting with `prefix`, said within the hang guard
    /// (other lines, a dialer's retries, do not extend it).
    async fn expect(&mut self, prefix: &str) -> String {
        within(prefix, async {
            loop {
                let line = self
                    .lines
                    .recv()
                    .await
                    .unwrap_or_else(|| panic!("the runtime ended without saying {prefix:?}"));
                if line.starts_with(prefix) {
                    return line;
                }
            }
        })
        .await
    }

    /// Listening: the address it says it is bound to.
    async fn listening(channel: &str, env: &[(&str, &str)]) -> (Self, String) {
        let mut runtime = Self::start(channel, env);
        let line = runtime.expect("rutis: listening on ").await;
        let address = line["rutis: listening on ".len()..].to_owned();
        (runtime, address)
    }
}

/// `host:port` and the path of a ws:// address.
fn split(address: &str) -> (String, String) {
    let rest = address.split_once("://").unwrap().1;
    let (host, path) = rest.split_once('/').unwrap();
    (host.to_owned(), format!("/{path}"))
}

/// An HTTP request to the listener: its status and body.
async fn request(address: &str, path: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let (host, _) = split(address);
    let mut stream = TcpStream::connect(&host).await.unwrap();
    let mut text = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n"
    );
    for (name, value) in headers {
        text += &format!("{name}: {value}\r\n");
    }
    text += "\r\n";
    stream.write_all(text.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let _ = within("the response", stream.read_to_end(&mut response)).await;
    let response = String::from_utf8_lossy(&response).into_owned();
    let status = response.split(' ').nth(1).unwrap().parse().unwrap();
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.trim().to_owned())
        .unwrap_or_default();
    (status, body)
}

const TOKEN: &str = "secret";

/// A request: its path, its headers, and the status it is refused with.
type Refusal<'a> = (&'a str, &'a [(&'a str, &'a str)], u16);

/// guarantee: bridge/version-refusal
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_connection_says_why_with_its_status() {
    let (_runtime, address) =
        Runtime::listening("listen:ws://127.0.0.1:0/rutis", &[("RUTIS_TOKEN", TOKEN)]).await;
    let bearer = format!("Bearer {TOKEN}");
    let cases: [Refusal; 4] = [
        (
            "/other",
            &[
                ("Authorization", &bearer),
                ("Sec-WebSocket-Protocol", "rutis.3"),
            ],
            404,
        ),
        ("/rutis", &[("Sec-WebSocket-Protocol", "rutis.3")], 401),
        (
            "/rutis",
            &[
                ("Authorization", "Bearer wrong"),
                ("Sec-WebSocket-Protocol", "rutis.3"),
            ],
            403,
        ),
        ("/rutis", &[("Authorization", &bearer)], 400),
    ];
    for (path, headers, status) in cases {
        assert_eq!(
            request(&address, path, headers).await.0,
            status,
            "{path} {headers:?}"
        );
    }
    for (offered, said) in [
        ("rutis.99", "this endpoint speaks rutis.3, the dialer offered rutis.99: upgrade this endpoint (the Kotlin SDK io.github.arcships:rutis)"),
        ("rutis.1", "this endpoint speaks rutis.3, the dialer offered rutis.1: upgrade the dialer"),
    ] {
        let response = request(&address, "/rutis", &[("Authorization", &bearer), ("Sec-WebSocket-Protocol", offered)]).await;
        assert_eq!(response, (400, said.to_owned()));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn addresses_that_cannot_work_are_refused_at_start() {
    for (channel, said) in [
        (
            "listen:ws://0.0.0.0:0/rutis",
            "only a loopback listener may go without TLS",
        ),
        (
            "listen:wss://127.0.0.1:0/rutis",
            "wss needs a certificate and key (RUTIS_CERT, RUTIS_KEY)",
        ),
        (
            "dial:ws://example.com/rutis",
            "a non-loopback address needs wss://",
        ),
        (
            "dial:http://127.0.0.1/rutis",
            "is not a ws:// or wss:// address",
        ),
    ] {
        let mut runtime = Runtime::start(channel, &[("RUTIS_TOKEN", TOKEN)]);
        let line = runtime.expect("rutis: ").await;
        assert!(line.contains(said), "{channel}: {line}");
        let status = within("the exit", runtime.child.wait()).await.unwrap();
        assert_eq!(status.code(), Some(1), "{channel}");
    }
}

/// A controller's end of a connection, framed by hand.
struct Client {
    stream: TcpStream,
}

impl Client {
    async fn connect(address: &str) -> Self {
        let (host, path) = split(address);
        let mut stream = TcpStream::connect(&host).await.unwrap();
        let text = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Protocol: rutis.3\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
        );
        stream.write_all(text.as_bytes()).await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0u8];
            within("the upgrade", stream.read_exact(&mut byte))
                .await
                .unwrap();
            head.push(byte[0]);
        }
        let head = String::from_utf8(head).unwrap();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        assert!(
            head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "{head}"
        );
        assert!(head.contains("Sec-WebSocket-Protocol: rutis.3"), "{head}");
        Self { stream }
    }

    async fn send(&mut self, opcode: u8, payload: &[u8], masked: bool) {
        let mut frame = vec![0x80 | opcode];
        let bit = if masked { 0x80 } else { 0 };
        match payload.len() {
            n if n < 126 => frame.push(bit | n as u8),
            n if n <= 0xFFFF => frame.extend([bit | 126, (n >> 8) as u8, n as u8]),
            n => {
                frame.push(bit | 127);
                frame.extend((n as u64).to_be_bytes());
            }
        }
        let mut body = payload.to_vec();
        if masked {
            let mask = [1, 2, 3, 4];
            frame.extend(mask);
            for (index, byte) in body.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        frame.extend(body);
        self.stream.write_all(&frame).await.unwrap();
    }

    /// The next frame the runtime sends.
    async fn next(&mut self) -> (u8, Vec<u8>) {
        within("a frame", async {
            let mut head = [0u8; 2];
            self.stream.read_exact(&mut head).await.unwrap();
            let mut length = (head[1] & 0x7F) as u64;
            if length == 126 {
                let mut n = [0u8; 2];
                self.stream.read_exact(&mut n).await.unwrap();
                length = u16::from_be_bytes(n) as u64;
            } else if length == 127 {
                let mut n = [0u8; 8];
                self.stream.read_exact(&mut n).await.unwrap();
                length = u64::from_be_bytes(n);
            }
            let mut payload = vec![0u8; length as usize];
            self.stream.read_exact(&mut payload).await.unwrap();
            (head[0] & 0x0F, payload)
        })
        .await
    }

    /// The code of the runtime's close frame.
    async fn closed(&mut self) -> u16 {
        loop {
            let (opcode, payload) = self.next().await;
            if opcode == 0x8 {
                return u16::from_be_bytes([payload[0], payload[1]]);
            }
        }
    }
}

/// guarantee: bridge/frame-limits
#[tokio::test(flavor = "multi_thread")]
async fn broken_frames_close_with_their_code() {
    let (_runtime, address) =
        Runtime::listening("listen:ws://127.0.0.1:0/rutis", &[("RUTIS_TOKEN", TOKEN)]).await;
    let mut unmasked = Client::connect(&address).await;
    unmasked.send(0x1, b"x", false).await;
    assert_eq!(unmasked.closed().await, 1002);
    let mut binary = Client::connect(&address).await;
    binary.send(0x2, b"x", true).await;
    assert_eq!(binary.closed().await, 1003);
    let mut big = Client::connect(&address).await;
    let mut frame = vec![0x81, 0x80 | 127];
    frame.extend(((16 * 1024 * 1024 + 1) as u64).to_be_bytes());
    frame.extend([1, 2, 3, 4]);
    big.stream.write_all(&frame).await.unwrap();
    assert_eq!(big.closed().await, 1009);
    let mut invalid = Client::connect(&address).await;
    invalid.send(0x1, &[0xC3, 0x28], true).await;
    assert_eq!(invalid.closed().await, 1007);
}

/// The runtime greets each controller; a newer one takes over, the older
/// closed with 4002; a silent one is pinged, then dropped (1001).
#[tokio::test(flavor = "multi_thread")]
async fn a_newer_controller_takes_over_and_a_silent_one_is_dropped() {
    let (_runtime, address) = Runtime::listening(
        "listen:ws://127.0.0.1:0/rutis",
        &[("RUTIS_TOKEN", TOKEN), ("RUTIS_HEARTBEAT", "100,1000")],
    )
    .await;
    let mut first = Client::connect(&address).await;
    let (opcode, hello) = first.next().await;
    assert_eq!(opcode, 0x1);
    assert!(
        String::from_utf8_lossy(&hello).contains("\"op\":\"hello\""),
        "{hello:?}"
    );
    let mut second = Client::connect(&address).await;
    assert_eq!(first.closed().await, 4002);
    // The second says nothing: pinged, then dropped.
    let mut pinged = false;
    loop {
        let (opcode, payload) = second.next().await;
        match opcode {
            0x9 => pinged = true,
            0x8 => {
                assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1001);
                break;
            }
            _ => {}
        }
    }
    assert!(pinged);
}

/// A CA and a certificate for `localhost` and 127.0.0.1 it signed.
fn pki() -> (String, String, String) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "rutis kotlin test CA");
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);
    let key = KeyPair::generate().unwrap();
    let certificate = CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    (ca.pem(), certificate.pem(), key.serialize_pem())
}

/// `listen:wss://`: the runtime serves TLS with RUTIS_CERT and RUTIS_KEY;
/// a controller trusting the CA greets it.
#[tokio::test(flavor = "multi_thread")]
async fn a_listener_serves_tls() {
    let (ca, certificate, key) = pki();
    let dir = tempfile::tempdir().unwrap();
    let (cert_file, key_file) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    std::fs::write(&cert_file, certificate).unwrap();
    std::fs::write(&key_file, key).unwrap();
    let (_runtime, address) = Runtime::listening(
        "listen:wss://127.0.0.1:0/rutis",
        &[
            ("RUTIS_TOKEN", TOKEN),
            ("RUTIS_CERT", cert_file.to_str().unwrap()),
            ("RUTIS_KEY", key_file.to_str().unwrap()),
        ],
    )
    .await;
    assert!(address.starts_with("wss://127.0.0.1:"), "{address}");
    let address = address.replace("127.0.0.1", "localhost");
    let transport =
        WebSocketTransport::start(Config::new().trust(Trust::only(ca.into_bytes()))).unwrap();
    let runtime = PeerId::new("runtime").unwrap();
    let identity: Arc<dyn Identity> = Arc::new(
        StaticIdentity::new(PeerId::new("main").unwrap())
            .present(runtime.clone(), Credential::Bearer(TOKEN.into())),
    );
    let mut channel = within(
        "the TLS connection",
        transport.dial(
            &Dial::address(address)
                .peer(runtime)
                .identity(identity)
                .protocol("rutis.3"),
        ),
    )
    .await
    .unwrap();
    let hello = within("the hello", async { channel.receiver.recv() })
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&hello).contains("\"op\":\"hello\""));
    std::mem::forget(transport);
}

/// `dial:wss://`: a certificate this side does not trust is an
/// authentication failure, retried; RUTIS_CA trusts its CA.
#[tokio::test(flavor = "multi_thread")]
async fn a_dialer_trusts_rutis_ca() {
    let (ca, certificate, key) = pki();
    let server = WebSocketTransport::start(
        Config::new().listener(
            ListenerConfig::new(
                "public",
                "127.0.0.1:0".parse().unwrap(),
                PeerId::new("main").unwrap(),
            )
            .tls(ServerTls {
                certificate_pem: certificate.into_bytes(),
                key_pem: key.into_bytes(),
                client_ca_pem: None,
            }),
        ),
    )
    .unwrap();
    let address = format!(
        "wss://localhost:{}/rutis",
        server.local_addr("public").unwrap().port()
    );
    let channel = format!("dial:{address}");
    let mut untrusting = Runtime::start(
        &channel,
        &[("RUTIS_TOKEN", TOKEN), ("RUTIS_RETRY", "100,400,60000,300")],
    );
    let line = untrusting.expect("rutis: refused: auth-rejected (").await;
    assert!(line.contains("certificate"), "{line}");
    drop(untrusting);
    let dir = tempfile::tempdir().unwrap();
    let ca_file = dir.path().join("ca.pem");
    std::fs::write(&ca_file, ca).unwrap();
    let mut trusting = Runtime::start(
        &channel,
        &[
            ("RUTIS_TOKEN", TOKEN),
            ("RUTIS_RETRY", "100,400,60000,300"),
            ("RUTIS_CA", ca_file.to_str().unwrap()),
        ],
    );
    // Past TLS: no access point is registered on this listener, so the
    // upgrade is refused (503) and retried; not a certificate failure.
    let line = trusting.expect("rutis: ").await;
    assert!(
        line.starts_with("rutis: cannot connect (localhost: upgrade refused with 503"),
        "{line}"
    );
}
