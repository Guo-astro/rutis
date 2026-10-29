//! WebSocket failures across implementations: Rust and Node, each dialing
//! the other, and Rust dialing Python, classify refusals the same way
//! (credentials and certificates are authentication failures, a subprotocol
//! mismatch is incompatible). The Python runtime only listens: it has no
//! dialer, so there is no Python-dialing-Rust column.
#![cfg(feature = "websocket")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rutis_bridge::channel::Channel;
use rutis_bridge::channel::ConnectError;
use rutis_bridge::websocket::Credential;
use rutis_bridge::websocket::{self, Limits, Listener, ListenerConfig, ServerTls, Trust};
use rutis_test_support::within;
use tokio::io::AsyncBufReadExt;

fn node_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime")
}

/// A CA and a certificate for localhost signed by it, as PEM.
fn pki() -> (String, String, String) {
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
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["localhost".into()]).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "localhost");
    let certificate = params.signed_by(&key, &issuer).unwrap();
    (ca.pem(), certificate.pem(), key.serialize_pem())
}

async fn node_dial(url: &str, protocol: &str, env: &[(&str, &str)]) -> String {
    let output = tokio::process::Command::new("node")
        .args(["--import", "tsx"])
        .arg(node_dir().join("test/fixtures/ws-probe.mjs"))
        .args(["dial", url, protocol])
        .envs(env.iter().copied())
        .current_dir(node_dir())
        .output()
        .await
        .unwrap();
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn node_listen(url: &str, env: &[(&str, &str)]) -> (tokio::process::Child, String) {
    let mut child = tokio::process::Command::new("node")
        .args(["--import", "tsx"])
        .arg(node_dir().join("test/fixtures/ws-probe.mjs"))
        .args(["listen", url])
        .envs(env.iter().copied())
        .current_dir(node_dir())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = loop {
        let line = lines
            .next_line()
            .await
            .unwrap()
            .expect("the listener's address");
        if let Some(address) = line.strip_prefix("rutis: listening on ") {
            break address.to_owned();
        }
    };
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    (child, address)
}

/// The Python interpreter: `RUTIS_PYTHON`, else `python3` (`python` on
/// Windows). It needs the `websockets` package (rutis[network]).
fn python() -> String {
    std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}

/// A Python runtime listening on `url` (`python -m rutis listen:<url>`),
/// with credentials, certificate and key in RUTIS_* variables; the address
/// it announces.
async fn python_listen(url: &str, env: &[(&str, &str)]) -> (tokio::process::Child, String) {
    let project = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new(python())
        .args(["-m", "rutis"])
        .arg(format!("listen:{url}"))
        .args(["--id", "python"])
        .arg(project.path())
        .env(
            "PYTHONPATH",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/rutis"),
        )
        .envs(env.iter().copied())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = within("the Python listener's address", async {
        loop {
            let line = lines
                .next_line()
                .await
                .unwrap()
                .expect("the listener's address");
            if let Some(address) = line.strip_prefix("rutis: listening on ") {
                break address.to_owned();
            }
            eprintln!("python: {line}");
        }
    })
    .await;
    tokio::spawn(async move {
        let _project = project;
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("python: {line}");
        }
    });
    (child, address)
}

/// A loopback listener `public` as `config` says, accepting token `good`
/// speaking `protocol`.
fn listen(config: ListenerConfig, protocol: &str) -> Listener {
    let protocol = protocol.to_owned();
    let admit: websocket::Admit = Arc::new(move |upgrade: websocket::Upgrade<'_>| {
        if upgrade.bearer != Some("good") {
            return Err(websocket::Refused::Forbidden("not accepted here".into()));
        }
        Ok(websocket::Admitted {
            protocol: protocol.clone(),
            deliver: Box::new(|_| {}),
        })
    });
    websocket::listen(&config, &Limits::default(), admit).unwrap()
}

/// Dial `url` offering `protocol`, with bearer `token`, trusting `trust`.
async fn dial(
    url: &str,
    token: Option<&str>,
    protocol: &str,
    trust: &Trust,
) -> Result<Channel, ConnectError> {
    let credential = token.map(|token| Credential::Bearer(token.into()));
    websocket::dial(
        url,
        protocol,
        credential.as_ref(),
        trust,
        &Limits::default(),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn node_dialing_rust_is_refused_by_category() {
    let (ca, certificate, key) = pki();
    let plain = listen(
        ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap()),
        "rutis.4",
    );
    let url = format!("ws://{}/rutis", plain.local_addr());
    assert_eq!(
        node_dial(&url, "rutis.4", &[("RUTIS_TOKEN", "good")]).await,
        "connected"
    );
    assert_eq!(
        node_dial(&url, "rutis.4", &[("RUTIS_TOKEN", "bad")]).await,
        "auth-rejected"
    );
    assert_eq!(node_dial(&url, "rutis.4", &[]).await, "auth-rejected");
    assert_eq!(
        node_dial(&url, "rutis.9", &[("RUTIS_TOKEN", "good")]).await,
        "incompatible"
    );

    // wss: verified against the CA, refused without it.
    let secure = listen(
        ListenerConfig::new("public", "127.0.0.1:0".parse().unwrap()).tls(ServerTls {
            certificate_pem: certificate.into_bytes(),
            key_pem: key.into_bytes(),
            client_ca_pem: None,
        }),
        "rutis.4",
    );
    let url = format!("wss://localhost:{}/rutis", secure.local_addr().port());
    let dir = tempfile::tempdir().unwrap();
    let ca_file = dir.path().join("ca.pem");
    std::fs::write(&ca_file, ca).unwrap();
    let ca_path = ca_file.to_str().unwrap();
    assert_eq!(
        node_dial(
            &url,
            "rutis.4",
            &[("RUTIS_TOKEN", "good"), ("RUTIS_CA", ca_path)]
        )
        .await,
        "connected"
    );
    assert_eq!(
        node_dial(&url, "rutis.4", &[("RUTIS_TOKEN", "good")]).await,
        "auth-rejected"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rust_dialing_node_is_refused_by_category() {
    let (ca, certificate, key) = pki();
    let (system, trusting) = (Trust::default(), Trust::only(ca.into_bytes()));
    let (_plain, url) = node_listen("ws://127.0.0.1:0/rutis", &[("RUTIS_TOKEN", "good")]).await;
    assert!(dial(&url, Some("good"), "rutis.4", &system).await.is_ok());
    assert!(matches!(
        dial(&url, Some("bad"), "rutis.4", &system).await,
        Err(ConnectError::AuthRejected { .. })
    ));
    assert!(matches!(
        dial(&url, Some("good"), "rutis.9", &system).await,
        Err(ConnectError::Incompatible { .. })
    ));

    let dir = tempfile::tempdir().unwrap();
    let (cert_file, key_file) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    std::fs::write(&cert_file, certificate).unwrap();
    std::fs::write(&key_file, key).unwrap();
    let (_secure, url) = node_listen(
        "wss://localhost:0/rutis",
        &[
            ("RUTIS_TOKEN", "good"),
            ("RUTIS_CERT", cert_file.to_str().unwrap()),
            ("RUTIS_KEY", key_file.to_str().unwrap()),
        ],
    )
    .await;
    assert!(dial(&url, Some("good"), "rutis.4", &trusting).await.is_ok());
    assert!(matches!(
        dial(&url, Some("good"), "rutis.4", &system).await,
        Err(ConnectError::AuthRejected { .. })
    ));
}

/// Risk E8: a Python listener refuses as the Node one does, by category.
/// guarantee: bridge/websocket-refusal-categories
#[tokio::test(flavor = "multi_thread")]
async fn rust_dialing_python_is_refused_by_category() {
    let (ca, certificate, key) = pki();
    let (system, trusting) = (Trust::default(), Trust::only(ca.into_bytes()));
    let (_plain, url) = python_listen("ws://127.0.0.1:0/rutis", &[("RUTIS_TOKEN", "good")]).await;
    assert!(dial(&url, Some("good"), "rutis.4", &system).await.is_ok());
    assert!(matches!(
        dial(&url, Some("bad"), "rutis.4", &system).await,
        Err(ConnectError::AuthRejected { .. })
    ));
    assert!(matches!(
        dial(&url, None, "rutis.4", &system).await,
        Err(ConnectError::AuthRejected { .. })
    ));
    assert!(matches!(
        dial(&url, Some("good"), "rutis.9", &system).await,
        Err(ConnectError::Incompatible { .. })
    ));

    // wss: verified against the CA, refused without it.
    let dir = tempfile::tempdir().unwrap();
    let (cert_file, key_file) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    std::fs::write(&cert_file, certificate).unwrap();
    std::fs::write(&key_file, key).unwrap();
    let (_secure, announced) = python_listen(
        "wss://localhost:0/rutis",
        &[
            ("RUTIS_TOKEN", "good"),
            ("RUTIS_CERT", cert_file.to_str().unwrap()),
            ("RUTIS_KEY", key_file.to_str().unwrap()),
        ],
    )
    .await;
    // The listener announces the address it bound; the certificate names
    // localhost.
    let port = announced
        .rsplit_once(':')
        .and_then(|(_, rest)| rest.split('/').next())
        .unwrap();
    let url = format!("wss://localhost:{port}/rutis");
    assert!(dial(&url, Some("good"), "rutis.4", &trusting).await.is_ok());
    assert!(matches!(
        dial(&url, Some("good"), "rutis.4", &system).await,
        Err(ConnectError::AuthRejected { .. })
    ));
}
