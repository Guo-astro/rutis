//! S5 (#189), the loopback version: two separate processes on one
//! machine, as `docs/guide/nodes.md` sets them up. Node B is a Python
//! runtime listening for its controller (`python -m rutis listen:…`) with
//! the plugin `infer`; host A is `rutis-host run`, whose `rutis.json`
//! declares B as a remote runtime, links to it and runs the row
//! `gpu:infer` there. A probe on A calls `infer`.
//!
//! The network between them breaks and comes back through the harness's
//! TCP proxy ([`rutis_e2e::Proxy`]): A dials the proxy, the proxy dials B.
//! The plan is `docs/design-quality-step2-2026-10-11.md` §3.3; risks E11
//! (two separate host processes), E1 (a call in progress when the link
//! breaks), E6 (TLS refusals between processes), B7 (credentials in
//! output) of `docs/quality-status.md` §4.
//!
//! rutis-host prints neither its link's state nor why a dial failed
//! (#321), so what A's link did is read from its rows' states, the calls'
//! outcomes, B's output and the proxy's connections.
//!
//! Waits: the link redials after 0.5 s, doubling each failure up to 30 s,
//! ±20 % (`docs/guide/nodes.md`), and a session only resets that after
//! 60 s. Each test starts its own A, so a reconnect comes after at most a
//! few failures: those while the network is cut (the time to see the
//! call fail and the row wait) or B is down (the time Python takes to
//! start B again). Failing at most 0.5 + 1 + 2 s while down, the next
//! attempt comes within 4 × 1.2 = 4.8 s, well within the hang guard
//! (30 s); a B that takes T seconds to start delays it to about 2.4 T.
//! An authentication or certificate refusal is retried only after 30 s,
//! so each refused case waits for one refusal, never for a retry.
//!
//! Not here: revoking A's token while it is connected has no mechanism
//! (#273, Q2 of the plan): the refusal is tested by restarting B with
//! another token. The nightly version with containers, real addresses
//! and netem (E12) is step three of #183.

use std::net::SocketAddr;
use std::path::PathBuf;

use rutis_e2e::{repo, Host, Lang, Probe, Scenario};
use serde_json::json;

/// B's plugin: `run(job)` prints one line per execution, which the
/// scenario counts; the job `gated` then waits until the gate file
/// exists, to be in progress while the network breaks.
const INFER: &str = r#"
import asyncio
import os

provides = {"infer": {"run": "async"}}


class Infer:
    def __init__(self, gate):
        self.gate = gate

    async def run(self, job):
        print(f"infer: executing {job}", flush=True)
        if job == "gated":
            # Polls the file the scenario creates; the scenario itself
            # waits on output only.
            while not os.path.exists(self.gate):
                await asyncio.sleep(0.05)
        return f"inferred {job}"


def apply(ctx, config):
    ctx.provide("infer", Infer(config["gate"]))
"#;

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

/// A server certificate of `python/rutis/tests/tls` (fixed names and
/// validity, signed by its `ca.pem`): its certificate and key.
fn tls(name: &str) -> (PathBuf, PathBuf) {
    let dir = repo().join("python/rutis/tests/tls");
    (
        dir.join(format!("{name}.pem")),
        dir.join(format!("{name}.key")),
    )
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Start node B on `port` with `token`: `ws://`, or `wss://` with the
/// certificate `cert` (a name for [`tls`]). Returns once it listens.
fn node(s: &Scenario, port: u16, token: &str, cert: Option<&str>) -> Host {
    s.write("gpu/infer.py", INFER);
    let scheme = if cert.is_some() { "wss" } else { "ws" };
    let pythonpath = repo().join("python/rutis").display().to_string();
    let (cert, key) = match cert {
        Some(name) => {
            let (cert, key) = tls(name);
            (cert.display().to_string(), key.display().to_string())
        }
        None => (String::new(), String::new()),
    };
    let mut env = vec![("RUTIS_TOKEN", token), ("PYTHONPATH", pythonpath.as_str())];
    if !cert.is_empty() {
        env.extend([("RUTIS_CERT", cert.as_str()), ("RUTIS_KEY", key.as_str())]);
    }
    let mut b = s.program(
        "",
        python(),
        [
            "-m".into(),
            "rutis".into(),
            format!("listen:{scheme}://127.0.0.1:{port}/rutis"),
            "--id".into(),
            "gpu".into(),
            "--peer".into(),
            "main".into(),
            s.path("gpu").display().to_string(),
        ],
        &env,
    );
    b.expect("rutis: listening on");
    b
}

/// Start host A dialing B at `dial` with `token` (and, for `wss://`,
/// trusting the test CA); not waiting for anything.
fn main_host(s: &Scenario, dial: &str, token: &str) -> (Host, Probe) {
    if !s.path("node_modules").exists() {
        s.link_node_sdk();
    }
    let probe = s.probe("probe", Lang::Node, &["infer"]);
    s.write_json(
        "rutis.json",
        &json!({
            "id": "main",
            "runtimes": {
                "node": {},
                "remote": [{ "name": "gpu", "language": "python" }],
            },
            "rows": [
                {
                    "id": "gpu",
                    "name": "rutis-bridge/peer",
                    "config": { "peer": "gpu", "dial": dial, "runtime": "gpu" },
                },
                {
                    "id": "infer",
                    "name": "gpu:infer",
                    "config": { "gate": s.path("gate") },
                },
                probe.row(),
            ],
        }),
    );
    let (ca, _) = tls("ca");
    let ca = ca.display().to_string();
    let a = s.host_with(
        ["run", "rutis.json"],
        &[("RUTIS_TOKEN_GPU", token), ("RUTIS_CA", ca.as_str())],
    );
    (a, probe)
}

/// [`main_host`], returning once `infer` runs on B and the probe started.
fn main_ready(s: &Scenario, dial: &str, token: &str) -> (Host, Probe) {
    let (mut a, probe) = main_host(s, dial, token);
    a.expect("infer: running");
    probe.started(&mut a);
    (a, probe)
}

/// Stop A as from a terminal (exit code 0), then B.
fn stop(mut a: Host, mut b: Host) -> Host {
    a.ctrl_c();
    let status = a.wait_exit();
    assert_eq!(status.code(), Some(0), "A after Ctrl-C: {status}");
    b.kill();
    b.wait_exit();
    b.wait_output_end();
    b
}

/// Steps 1–3: a call through the link succeeds; the network breaks while
/// B runs a call; it comes back.
///
/// risk: E11, E1 (requirements-protocol-plugins §6: a call cut off fails
/// explicitly, no false success, no automatic retry of side effects)
/// guarantee: bridge/in-flight-call-fails-once (proposed),
/// bridge/no-call-on-a-dead-session (proposed); Q3.1 core promise
/// "interrupted calls fail explicitly and are not retried"
#[test]
fn a_cut_fails_the_call_in_progress_once_and_the_link_comes_back() {
    let s = Scenario::new("node-cut");
    let token = s.secret();
    let port = s.port();
    let mut b = node(&s, port, &token, None);
    let proxy = s.proxy(loopback(port));
    let (mut a, probe) = main_ready(&s, &format!("ws://{}/rutis", proxy.addr()), &token);

    // 1. A call on B, through the link.
    assert_eq!(
        probe.call(&mut a, "infer", "run", json!(["first"])),
        Ok(json!("inferred first"))
    );
    b.expect("infer: executing first");

    // 2. The network breaks while B runs a call.
    a.skip_written();
    let gated = probe.send("infer", "run", json!(["gated"]));
    b.expect("infer: executing gated");
    proxy.cut();
    let error = probe
        .answer(&mut a, gated)
        .expect_err("a call cut off by the network fails, it does not succeed");
    assert!(!error.is_empty(), "the failure says why");
    // The row on B waits for its runtime; what uses it stops.
    a.expect("infer: waiting for its services");
    probe.event(&mut a, "stopped");

    // 3. The network comes back: the link redials, the row runs again.
    proxy.restore();
    a.expect("infer: running");
    probe.started(&mut a);
    // The call cut off may now finish on B; the link must not send it again.
    s.write("gate", "");
    assert_eq!(
        probe.call(&mut a, "infer", "run", json!(["after"])),
        Ok(json!("inferred after"))
    );
    b.expect("infer: executing after");

    // Everything B ran, with A gone: the call cut off ran once (E1).
    let b = stop(a, b);
    assert_eq!(
        b.count("infer: executing gated"),
        1,
        "the call cut off ran once on B, not again after the reconnect"
    );
    assert_eq!(b.count("infer: executing after"), 1);
    s.finish();
}

/// Step 4: B is killed (`kill -9`) and started again; A's link takes a new
/// lease on the new process, and its row runs there.
///
/// risk: E11
#[test]
fn a_node_killed_and_started_again_gives_a_new_lease() {
    let s = Scenario::new("node-restart");
    let token = s.secret();
    let port = s.port();
    let mut b = node(&s, port, &token, None);
    let (mut a, probe) = main_ready(&s, &format!("ws://127.0.0.1:{port}/rutis"), &token);
    assert_eq!(
        probe.call(&mut a, "infer", "run", json!(["first"])),
        Ok(json!("inferred first"))
    );

    a.skip_written();
    b.kill();
    b.wait_exit();
    a.expect("infer: waiting for its services");
    probe.event(&mut a, "stopped");

    let mut b2 = node(&s, port, &token, None);
    a.expect("infer: running");
    probe.started(&mut a);
    assert_eq!(
        probe.call(&mut a, "infer", "run", json!(["again"])),
        Ok(json!("inferred again"))
    );
    b2.expect("infer: executing again");

    let b2 = stop(a, b2);
    assert_eq!(
        b2.count("infer: executing"),
        1,
        "the new B ran the new call"
    );
    b.wait_output_end();
    assert_eq!(b.count("infer: executing again"), 0);
    s.finish();
}

/// Step 5 up to the refusal: B is started again with another token; A's
/// redial reaches it and is refused (its connection closes, where an
/// accepted session stays open), and the row does not run again. Neither
/// token appears in any output (B7, checked by `finish`).
fn refused_by_another_token(s: &Scenario) -> (Host, Host) {
    let token = s.secret();
    let other = s.secret();
    let port = s.port();
    let mut b = node(s, port, &token, None);
    let proxy = s.proxy(loopback(port));
    let (mut a, probe) = main_ready(s, &format!("ws://{}/rutis", proxy.addr()), &token);
    assert_eq!(
        probe.call(&mut a, "infer", "run", json!(["first"])),
        Ok(json!("inferred first"))
    );

    b.kill();
    b.wait_exit();
    let b2 = node(s, port, &other, None);
    // Connection 1 was the session with the first B; 2 is the first one
    // to reach the new B, which refuses it.
    proxy.wait_closed(2);
    assert_eq!(
        a.count("infer: running"),
        1,
        "the row ran only on the first B"
    );
    (a, b2)
}

/// Step 5: a node restarted with another token refuses A's reconnect.
///
/// risk: E11, B7
/// guarantee: bridge/websocket-refusal-categories (proposed)
#[test]
fn a_node_restarted_with_another_token_refuses_the_reconnect() {
    let s = Scenario::new("node-token");
    let (a, b) = refused_by_another_token(&s);
    let b = stop(a, b);
    assert_eq!(b.count("infer: executing"), 0);
    s.finish();
}

/// Step 5, the rest: A's output says why it cannot reconnect, by a
/// readable category (credentials refused), without the token.
///
/// risk: E11, B7
#[test]
#[ignore = "bug #321: rutis-host prints no link state, nor why a dial is refused"]
fn a_refused_reconnect_is_reported_by_its_category() {
    let s = Scenario::new("node-token-category");
    let (mut a, b) = refused_by_another_token(&s);
    a.wait_for("A reporting the refusal of gpu's credentials", |line| {
        line.contains("gpu") && line.to_lowercase().contains("rejected")
    });
    stop(a, b);
    s.finish();
}

/// Step 6 for one certificate of B's (a name for [`tls`]): A dials
/// `wss://localhost` trusting the test CA. `refused`: A's TLS handshake
/// fails (its connection closes) and the row does not run; otherwise the
/// row runs and a call succeeds, which shows that the setup is right and
/// a refusal is the certificate's. A certificate accepted where it should
/// be refused keeps the connection open: the wait fails at the hang guard.
fn over_tls(s: &Scenario, cert: &str, refused: bool) -> (Host, Host) {
    let token = s.secret();
    let port = s.port();
    let b = node(s, port, &token, Some(cert));
    let proxy = s.proxy(loopback(port));
    let dial = format!("wss://localhost:{}/rutis", proxy.addr().port());
    let (mut a, probe) = main_host(s, &dial, &token);
    if refused {
        proxy.wait_closed(1);
        assert_eq!(
            a.count("infer: running"),
            0,
            "A accepted B's certificate {cert}"
        );
    } else {
        a.expect("infer: running");
        probe.started(&mut a);
        assert_eq!(
            probe.call(&mut a, "infer", "run", json!(["secure"])),
            Ok(json!("inferred secure"))
        );
    }
    (a, b)
}

/// Step 6: over `wss://`, A refuses B's certificate when it names another
/// host, and when it has expired; it accepts a valid one for `localhost`.
///
/// risk: E6 (between processes), B7
/// guarantee: bridge/tls-server-verified (proposed)
#[test]
fn a_certificate_for_another_name_or_expired_is_refused() {
    let s = Scenario::new("node-tls");
    for (cert, refused) in [
        ("localhost", false),
        ("other-name", true),
        ("expired", true),
    ] {
        let (a, b) = over_tls(&s, cert, refused);
        stop(a, b);
    }
    s.finish();
}

/// Step 6, the rest: A's output says why, by a readable category (the
/// certificate), for each refused certificate.
///
/// risk: E6
#[test]
#[ignore = "bug #321: rutis-host prints no link state, nor why a dial is refused"]
fn a_refused_certificate_is_reported_by_its_category() {
    let s = Scenario::new("node-tls-category");
    for cert in ["other-name", "expired"] {
        let (mut a, b) = over_tls(&s, cert, true);
        a.wait_for("A reporting the refusal of gpu's certificate", |line| {
            line.contains("gpu") && line.to_lowercase().contains("certificate")
        });
        stop(a, b);
    }
    s.finish();
}
