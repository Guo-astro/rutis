//! What the host says of the common mistakes, word for word (Q5.6.3): a
//! change here is a change users see, to be explained in review. Each says
//! where (the file, field, listener or runtime), why, and what to do
//! (Q5.6.1).

// Each test holds the environment until it ends, across its awaits: the
// variables it sets are for the host it starts.
#![allow(clippy::await_holding_lock)]

mod common;

use std::path::Path;

use common::{environment, normalize, repo, write};
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{Guard, HANG_GUARD};

/// Read `text` as `<dir>/rutis.json` and start what it names, up to the
/// runtimes running: the first error, as on any machine.
async fn first_error(dir: &Path, text: &str) -> String {
    write(dir, &[("rutis.json", text)]);
    let started = async {
        let config = HostConfig::read(&dir.join("rutis.json"))?;
        let host = Host::start(&config).await?;
        host.runtimes_ready().await
    };
    // Some start a runtime (Node, Python, Bun) cold; twice HANG_GUARD, only
    // against a hang.
    let error = Guard(HANG_GUARD * 2)
        .within(&format!("starting {text}"), started)
        .await
        .expect_err("an error");
    normalize(&error, dir)
}

/// risk: B6, A7, J3
/// guarantee: host/rutis-json-format (a newer format), host/error-messages
#[tokio::test(flavor = "multi_thread")]
async fn a_configuration_that_is_not_there_or_not_valid() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let missing = HostConfig::read(&dir.path().join("rutis.json")).unwrap_err();
    assert_eq!(
        normalize(&missing, dir.path()),
        "<dir>/rutis.json: <not found>: name the configuration, as `rutis-host run path/to/rutis.json`"
    );
    let cases = [
        (
            "{ \"id\": \"a\", }",
            "<dir>/rutis.json: trailing comma at line 1 column 14",
        ),
        (
            r#"{ "rows": {} }"#,
            "<dir>/rutis.json: invalid type: map, expected a sequence at line 1 column 10",
        ),
        (
            r#"{ "listen": [{ "name": "p", "address": "localhost:7443" }] }"#,
            "<dir>/rutis.json: invalid socket address syntax at line 1 column 55",
        ),
        (
            r#"{ "runtimes": { "go": { "dir": "g", "start": "lazy" } } }"#,
            "<dir>/rutis.json: unknown variant `lazy`, expected `on-demand` or `eager` at line 1 column 51",
        ),
        (
            r#"{ "format": 2, "services": {} }"#,
            "<dir>/rutis.json: this file is format 2, this rutis-host reads up to 1: upgrade rutis-host",
        ),
        (
            r#"{ "id": "Main Host" }"#,
            "id: invalid endpoint id \"Main Host\": use lowercase letters, digits and `-`",
        ),
        (
            r#"{ "runtimes": { "go": {} } }"#,
            "runtimes.go needs a dir or binaries",
        ),
        (
            r#"{ "runtimes": { "go": { "dir": "g", "start": "eager", "idle": 30 } } }"#,
            "runtimes.go: eager runtimes keep running, so idle does not apply to them",
        ),
        (
            r#"{ "runtimes": { "remote": [{ "name": "gpu", "language": "ruby" }] } }"#,
            "remote runtime gpu: the language is node, bun, python, go, swift or kotlin, not ruby",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(first_error(dir.path(), text).await, expected, "{text}");
    }
}

/// risk: B6
/// guarantee: host/error-messages
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_package_that_is_not_installed() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("package.json", "{}")]);
    let cases = [
        (
            r#"{ "runtimes": { "node": { "project": "web" } } }"#,
            "the Node runtime needs a package.json in <dir>/web: run `npm init -y` there",
        ),
        (
            r#"{ "runtimes": { "node": {} } }"#,
            "the Node runtime is not installed in <dir>: run `npm install @arcships/rutis-runtime` there",
        ),
        (
            r#"{ "runtimes": { "bun": {} } }"#,
            "the Bun runtime is not installed in <dir>: run `bun add -d @arcships/rutis-bun` there",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(first_error(dir.path(), text).await, expected, "{text}");
    }
}

/// An interpreter or program that is not there, and an interpreter
/// without the rutis package.
/// risk: B6
/// guarantee: host/error-messages
#[tokio::test(flavor = "multi_thread")]
async fn an_interpreter_that_is_not_there() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let bun = serde_json::json!({ "runtimes": { "bun": {
        "runtime": repo().join("bun/rutis-bun"), "program": "./no-bun"
    } } });
    let cases = [
        (
            r#"{ "runtimes": { "py": { "python": "./no-python" } } }"#.to_owned(),
            "the Python runtime cannot run its interpreter <dir>/no-python (<not found>): install Python, or set runtimes.py.python to an interpreter that has the rutis package",
        ),
        (
            r#"{ "runtimes": { "py": { "python": "no-python3" } } }"#.to_owned(),
            "the Python runtime cannot run its interpreter no-python3 (<not found>): install Python, or set runtimes.py.python to an interpreter that has the rutis package",
        ),
        (
            bun.to_string(),
            "the Bun runtime needs Bun: <dir>/no-bun does not run (install it from https://bun.com, or set `program`)",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(first_error(dir.path(), &text).await, expected, "{text}");
    }
}

/// An interpreter that runs but cannot import rutis: here a script that
/// fails whatever it is asked (a shell script, so not on Windows).
/// risk: B6
/// guarantee: host/error-messages
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn an_interpreter_without_the_rutis_package() {
    use std::os::unix::fs::PermissionsExt;
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("python");
    std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        first_error(dir.path(), r#"{ "runtimes": { "py": { "python": "./python" } } }"#).await,
        "the Python runtime needs the rutis package in <dir>/python: run `<dir>/python -m pip install rutis` (or `uv add rutis`)"
    );
}

/// risk: B6
/// guarantee: host/error-messages
#[tokio::test(flavor = "multi_thread")]
async fn a_port_another_program_holds() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap();
    let in_use = std::net::TcpListener::bind(address)
        .unwrap_err()
        .to_string();
    let text = format!(r#"{{ "listen": [{{ "name": "public", "address": "{address}" }}] }}"#);
    let error = first_error(dir.path(), &text).await;
    assert_eq!(
        error.replace(&address.to_string(), "<address>").replace(&in_use, "<in use>"),
        "the WebSocket transport: listener public: cannot bind <address>: <in use>: another program listens there: stop it, or change the address"
    );
}

/// Certificates and keys, from the file or from the environment: missing,
/// not PEM, or one without the other.
/// risk: B6
/// guarantee: host/error-messages, host/listen-tls, host/credentials-from-env
#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_that_is_wrong() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), &[("text.pem", "not a certificate")]);
    let listener = |tls: &str| {
        format!(r#"{{ "listen": [{{ "name": "public", "address": "127.0.0.1:0"{tls} }}] }}"#)
    };
    let cases = [
        (
            listener(r#", "cert": "tls/server.pem", "key": "tls/server.key""#),
            "listener public: cert <dir>/tls/server.pem: <not found>",
        ),
        (
            listener(r#", "cert": "text.pem", "key": "text.pem""#),
            "listener public: cert <dir>/text.pem: no certificate in it: give a PEM file (-----BEGIN CERTIFICATE-----)",
        ),
        (
            listener(r#", "cert": "text.pem""#),
            "listener public: cert is set alone: TLS needs a certificate and its key, set both cert and key (or RUTIS_CERT and RUTIS_KEY)",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(first_error(dir.path(), &text).await, expected, "{text}");
    }

    std::env::set_var("RUTIS_KEY", dir.path().join("text.pem"));
    assert_eq!(
        first_error(dir.path(), &listener("")).await,
        "listener public: RUTIS_KEY is set alone: TLS needs a certificate and its key, set both cert and key (or RUTIS_CERT and RUTIS_KEY)"
    );
    std::env::set_var("RUTIS_CERT", dir.path().join("server.pem"));
    assert_eq!(
        first_error(dir.path(), &listener("")).await,
        "listener public: RUTIS_CERT <dir>/server.pem: <not found>"
    );
    // The file's own come first.
    assert_eq!(
        first_error(dir.path(), &listener(r#", "cert": "own.pem""#)).await,
        "listener public: cert <dir>/own.pem: <not found>"
    );
    std::env::remove_var("RUTIS_CERT");
    std::env::remove_var("RUTIS_KEY");

    std::env::set_var("RUTIS_CA", dir.path().join("ca.pem"));
    assert_eq!(
        first_error(dir.path(), "{}").await,
        "RUTIS_CA <dir>/ca.pem: <not found>"
    );
    // (Given to the transport as it was, this panicked; rutis-bridge's
    // websocket tests keep it from coming back.)
    std::env::set_var("RUTIS_CA", dir.path().join("text.pem"));
    assert_eq!(
        first_error(dir.path(), "{}").await,
        "RUTIS_CA <dir>/text.pem: no certificate in it: give a PEM file (-----BEGIN CERTIFICATE-----)"
    );
    std::env::remove_var("RUTIS_CA");
}

/// A runtime that dials in (`listen` and `instances`): the listener, the
/// group and the token it needs.
/// risk: B6
/// guarantee: host/error-messages
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_dials_in_without_what_it_needs() {
    let _environment = environment();
    let dir = tempfile::tempdir().unwrap();
    let config = |remote: &str, rows: &str| {
        format!(
            r#"{{ "listen": [{{ "name": "public", "address": "127.0.0.1:0" }}],
                 "runtimes": {{ "remote": [{{ "name": "phone", "language": "python"{remote} }}] }},
                 "rows": [{rows}] }}"#
        )
    };
    let group = r#"{ "id": "phone", "group": true, "instanced": true, "config": [] }"#;
    let both = r#", "listen": "public", "instances": "phone""#;
    let cases = [
        (
            config(r#", "listen": "public""#, group),
            "remote runtime phone: listen needs instances: name the instanced group each connection is an instance of",
        ),
        (
            config(r#", "instances": "phone""#, group),
            "remote runtime phone: instances needs listen: name the listener its runtimes dial in to",
        ),
        (
            config(r#", "listen": "lan", "instances": "phone""#, group),
            "remote runtime phone: no listener named lan: add it to listen",
        ),
        (
            config(r#", "listen": "public", "instances": "phones""#, group),
            r#"remote runtime phone: no top-level row phones: instances names a group with "instanced": true"#,
        ),
        (
            config(both, r#"{ "id": "phone", "group": true, "config": [] }"#),
            r#"remote runtime phone: row phone is not an instanced group: give it "group": true and "instanced": true"#,
        ),
        (
            config(both, group),
            "remote runtime phone: set RUTIS_TOKEN_PHONE (or RUTIS_TOKEN): the token its runtimes present when they dial in",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(first_error(dir.path(), &text).await, expected, "{text}");
    }
}
