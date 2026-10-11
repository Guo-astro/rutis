//! The session protocol versions the SDKs greet with are this crate's
//! (risk P8). The Python and Node runtimes write them as literals; a change
//! to one side only would be found at the handshake of a released pair.
//! The constants are read from the source, so the check needs no
//! interpreter and runs wherever this crate's tests do.

use std::path::{Path, PathBuf};

use rutis_bridge::session::{ENDPOINT_PROTOCOL, PROTOCOL};

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

// guarantee: bridge/version-refusal (proposed); risk P8.
#[test]
fn the_python_runtime_greets_with_this_crates_protocol_versions() {
    let path = "python/rutis/rutis/peer.py";
    let source = read(path);
    assert_eq!(constant(&source, path, "PROTOCOL = "), PROTOCOL, "{path}");
    assert_eq!(
        constant(&source, path, "ENDPOINT_PROTOCOL = "),
        ENDPOINT_PROTOCOL,
        "{path}"
    );
}

// guarantee: bridge/version-refusal (proposed); risk P8.
#[test]
fn the_node_runtime_greets_with_this_crates_protocol_versions() {
    // The compat version is the package's `rutisProtocol`, which builds of
    // a Cordis mount check too (`cordis::build`).
    let path = "node/rutis-runtime/package.json";
    let package: serde_json::Value = serde_json::from_str(&read(path)).unwrap();
    assert_eq!(
        package["rutisProtocol"].as_u64(),
        Some(PROTOCOL.into()),
        "{path}"
    );
    let path = "node/rutis-runtime/src/session.mjs";
    assert_eq!(
        constant(&read(path), path, "export const ENDPOINT_PROTOCOL = "),
        ENDPOINT_PROTOCOL,
        "{path}"
    );
}
