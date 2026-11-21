//! The session protocol version the SDKs greet with is this crate's
//! (risk P8). The Python, Node, Bun and Go runtimes write them as literals; a change
//! to one side only would be found at the handshake of a released pair.
//! The constants are read from the source, so the check needs no
//! interpreter and runs wherever this crate's tests do.

use std::path::{Path, PathBuf};

use rutis_bridge::session::ENDPOINT_PROTOCOL;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The value of the top-level assignment `<prefix><number>` in `source`.
fn constant(source: &str, path: &str, prefix: &str) -> u32 {
    let mut found = source
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .map(|value| {
            value
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("{path}: `{prefix}{value}` is not a number"))
        });
    let value = found
        .next()
        .unwrap_or_else(|| panic!("{path}: no line starts with `{prefix}`"));
    assert!(found.next().is_none(), "{path}: `{prefix}` twice");
    value
}

// guarantee: bridge/version-refusal; risk P8.
#[test]
fn the_python_runtime_greets_with_this_crates_protocol_version() {
    let path = "python/rutis/rutis/peer.py";
    let source = read(path);
    assert_eq!(
        constant(&source, path, "ENDPOINT_PROTOCOL = "),
        ENDPOINT_PROTOCOL,
        "{path}"
    );
}

// guarantee: bridge/version-refusal; risk P8.
#[test]
fn the_node_runtime_greets_with_this_crates_protocol_version() {
    // The package's `rutisProtocol` too, which builds of a Cordis mount
    // check (`cordis::build`).
    let path = "node/rutis-runtime/package.json";
    let package: serde_json::Value = serde_json::from_str(&read(path)).unwrap();
    assert_eq!(
        package["rutisProtocol"].as_u64(),
        Some(ENDPOINT_PROTOCOL.into()),
        "{path}"
    );
    let path = "node/rutis-runtime/src/session.mjs";
    assert_eq!(
        constant(&read(path), path, "export const ENDPOINT_PROTOCOL = "),
        ENDPOINT_PROTOCOL,
        "{path}"
    );
}

// guarantee: bridge/version-refusal; risk P8.
#[test]
fn the_bun_runtime_greets_with_this_crates_protocol_version() {
    let path = "bun/rutis-bun/package.json";
    let package: serde_json::Value = serde_json::from_str(&read(path)).unwrap();
    assert_eq!(
        package["rutisProtocol"].as_u64(),
        Some(ENDPOINT_PROTOCOL.into()),
        "{path}"
    );
    let path = "bun/rutis-bun/src/session.ts";
    assert_eq!(
        constant(&read(path), path, "export const ENDPOINT_PROTOCOL = "),
        ENDPOINT_PROTOCOL,
        "{path}"
    );
}

/// The value of `<name> = <number>` in a Go `const ( … )` block of `source`.
fn go_constant(source: &str, path: &str, name: &str) -> u32 {
    let mut found = source.lines().filter_map(|line| {
        let (left, value) = line.trim().split_once('=')?;
        (left.trim() == name).then(|| {
            value
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("{path}: `{name} ={value}` is not a number"))
        })
    });
    let value = found
        .next()
        .unwrap_or_else(|| panic!("{path}: no constant {name}"));
    assert!(found.next().is_none(), "{path}: {name} twice");
    value
}

// guarantee: bridge/version-refusal; risk P8.
#[test]
fn the_go_runtime_greets_with_this_crates_protocol_version() {
    let path = "go/rutis/internal/peer/peer.go";
    let source = read(path);
    assert_eq!(
        go_constant(&source, path, "EndpointProtocol"),
        ENDPOINT_PROTOCOL,
        "{path}"
    );
}
