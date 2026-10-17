//! S5 across two containers (#341, risk E12): the nightly version of
//! `nodes.rs`, on a real network instead of loopback and a TCP proxy.
//!
//! Topology: two Docker containers, `a` and `b`, on a user-defined bridge
//! network of their own. In `b`, node B is a Python runtime listening on
//! `wss://0.0.0.0:7443/rutis` (every address of the container, not
//! loopback), with a certificate for its network name `gpu.netem.test`
//! signed by a CA the scenario makes and throws away. In `a`, host A is
//! `rutis-host run`, whose `rutis.json` declares B as a remote runtime,
//! dials `wss://gpu.netem.test:7443/rutis` trusting that CA, and runs the
//! row `gpu:infer` on B; a Node probe on A calls `infer` (as in
//! `nodes.rs`: a call's failure is reported even when the probe is being
//! stopped, where a Python probe cancels its calls in flight). The network
//! misbehaves through `tc qdisc … netem` on the containers' `eth0`
//! (`--cap-add NET_ADMIN`). Both sides run the programs of this checkout
//! (the `rutis-host` under test, `python/rutis`), mounted read-only at the
//! same paths; the scenario directory is mounted read-write, so the probe's
//! requests, the gate file and the residue checks of the scenario's `tmp/`
//! work as in the other scenarios.
//!
//! Phases, and what each asserts:
//!
//! 1. Baseline: A's link connects (`gpu: link connected`), a call on B
//!    succeeds, B runs it once.
//! 2. 200 ms delay and 5 % loss on B's side, for longer than a heartbeat
//!    timeout (30 s): every call succeeds, takes at least the delay, and
//!    runs once on B; the link does not go down.
//! 3. A full outage (100 % loss both ways) while B runs a call: A says
//!    `gpu: link down (<reason>)`, the call in progress fails on A (an
//!    error, not a success), the row on A waits for its runtime and B
//!    says `rutis: session ended (heartbeat timeout); waiting for a
//!    controller` and unloads the plugin (B's own heartbeat). A's redials
//!    during the outage fail and back off within the rules (0.5 s doubling
//!    to 30 s, ±20 %).
//! 4. Recovery: the link reconnects within the backoff bound (the longest
//!    wait, 36 s, plus a handshake's 10 s), the row runs again, a new call
//!    succeeds; the call cut off ran once on B, not again.
//! 5. Wrong token: B is stopped (Ctrl-C) and started again with another
//!    token; A's redial is refused and A says `gpu: link refused:
//!    auth-rejected (…); retrying in 30s`; the row does not run again.
//! 6. End: Ctrl-C to both. A exits with status 0; nothing of either is
//!    left running in its container and nothing listens in B's; no output
//!    contains a token, nor B's a traceback; the scenario's residue checks
//!    pass.
//!
//! Revoking A's identity while it is connected (#273) has no trigger in
//! `rutis-host` (its tokens come from the environment), so it is not a
//! phase here; the TLA+ model `specs/remote-lease` checks it.
//!
//! It needs Linux with Docker (and the `sch_netem` module): ignored by
//! `cargo test`, run by the nightly netem job (`.github/workflows/netem.yml`,
//! report only). On Linux:
//!
//! ```sh
//! npm --prefix node/rutis-runtime ci && npm --prefix node/rutis ci
//! cargo build --release -p rutis-host
//! RUTIS_E2E_HOST=$PWD/target/release/rutis-host \
//!   cargo test -p rutis-e2e --test netem -- --ignored --nocapture
//! ```
//!
//! The containers run the host's binary, so the image (`ubuntu:24.04`
//! with Node 22, Python, `websockets`, `iproute2`, `procps`, `openssl`) needs a
//! glibc at least as new as the build machine's. Elsewhere than Linux the
//! test says it is skipped and passes.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rutis_e2e::{repo, Host, Lang, Probe, Scenario};
use serde_json::json;

/// B's name on the network, in its certificate and in A's `dial`.
const B_NAME: &str = "gpu.netem.test";
const PORT: u16 = 7443;
/// The image both containers run, built from [`DOCKERFILE`].
const IMAGE: &str = "rutis-e2e-netem:1";
const DOCKERFILE: &str = r#"FROM node:22-bookworm-slim AS node
FROM ubuntu:24.04
COPY --from=node /usr/local/bin/node /usr/local/bin/node
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      python3 python3-pip iproute2 procps openssl ca-certificates \
 && pip3 install --break-system-packages --no-cache-dir "websockets>=15,<17" \
 && rm -rf /var/lib/apt/lists/*
"#;

/// The link's longest wait between dials (30 s + 20 %) and a dial's
/// handshake limit (10 s), `docs/guide/nodes.md`.
const LONGEST_WAIT: f64 = 36.0;
const HANDSHAKE: Duration = Duration::from_secs(10);

/// B's plugin, as in `nodes.rs`, and a line when it is unloaded.
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
            while not os.path.exists(self.gate):
                await asyncio.sleep(0.05)
        return f"inferred {job}"


def apply(ctx, config):
    ctx.provide("infer", Infer(config["gate"]))
    ctx.effect(lambda: print("infer: unloaded", flush=True))
"#;

/// Run `docker <args>`, failing the scenario with its output when it
/// fails; its stdout.
fn docker(args: &[&str]) -> String {
    docker_with_input(args, None)
}

fn docker_with_input(args: &[&str], input: Option<&str>) -> String {
    use std::io::Write;
    let mut child = Command::new("docker")
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("docker {}: {error}", args.join(" ")));
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .expect("docker's stdin");
    }
    let output = child.wait_with_output().expect("docker's output");
    assert!(
        output.status.success(),
        "docker {} failed ({}):\n{}{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// This user, `uid:gid`: the containers' programs write in the scenario's
/// directory as the test does.
#[cfg(unix)]
fn user() -> String {
    // SAFETY: getuid/getgid have no preconditions.
    unsafe { format!("{}:{}", libc::getuid(), libc::getgid()) }
}

#[cfg(not(unix))]
fn user() -> String {
    unreachable!("the scenario runs on Linux only")
}

/// The two containers and their network; removed when dropped, also when
/// the scenario fails.
struct Net {
    network: String,
    a: String,
    b: String,
    /// What listens in each container before the scenario starts anything
    /// (Docker's DNS resolver on 127.0.0.11): not residue.
    idle: Vec<String>,
}

impl Net {
    /// Build the image (cached), make the network and start both
    /// containers idle (`sleep infinity`, under `--init`), as this user,
    /// with the checkout mounted read-only and the scenario's directory
    /// read-write, at the same paths.
    fn start(s: &Scenario) -> Net {
        docker_with_input(&["build", "-q", "-t", IMAGE, "-"], Some(DOCKERFILE));
        let id: String = s
            .dir()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let mut net = Net {
            network: format!("rutis-{id}"),
            a: format!("rutis-{id}-a"),
            b: format!("rutis-{id}-b"),
            idle: Vec::new(),
        };
        docker(&["network", "create", &net.network]);
        let user = user();
        let checkout = repo().display().to_string();
        let checkout = format!("{checkout}:{checkout}:ro");
        let dir = s.dir().display().to_string();
        let scenario = format!("{dir}:{dir}");
        let host = rutis_e2e::host_program();
        let host = host.display().to_string();
        let host = format!("{host}:{host}:ro");
        let home = format!("HOME={dir}/run");
        for (name, alias) in [(&net.a, "main.netem.test"), (&net.b, B_NAME)] {
            docker(&[
                "run",
                "-d",
                "--init",
                "--name",
                name,
                "--network",
                &net.network,
                "--network-alias",
                alias,
                "--cap-add",
                "NET_ADMIN",
                "--user",
                &user,
                "-e",
                &home,
                "-e",
                "PYTHONDONTWRITEBYTECODE=1",
                "-v",
                &checkout,
                "-v",
                &host,
                "-v",
                &scenario,
                IMAGE,
                "sleep",
                "infinity",
            ]);
        }
        net.idle = [&net.a, &net.b]
            .into_iter()
            .flat_map(|container| listening(container))
            .collect();
        net
    }

    /// Start `program <args>` in `container`, in `cwd`, through
    /// `docker exec`: a [`Host`] of the scenario whose output is the
    /// program's. `env` goes to the program (passed by name: no value on a
    /// command line). Its pid goes to `<scenario>/run/<pidfile>`, for
    /// [`Net::ctrl_c`].
    #[allow(clippy::too_many_arguments)]
    fn exec(
        &self,
        s: &Scenario,
        container: &str,
        pidfile: &str,
        cwd: &Path,
        program: &str,
        args: &[String],
        env: &[(&str, &str)],
    ) -> Host {
        let pidfile = s.dir().join("run").join(pidfile);
        let mut command: Vec<String> = vec!["exec".into(), "-w".into(), cwd.display().to_string()];
        for name in [
            "TMPDIR",
            "TMP",
            "TEMP",
            "RUTIS_PYTHON_PATH",
            "RUTIS_NODE_RUNTIME",
        ]
        .into_iter()
        .chain(env.iter().map(|(name, _)| *name))
        {
            command.extend(["-e".into(), name.into()]);
        }
        command.extend([
            container.into(),
            "sh".into(),
            "-c".into(),
            r#"echo $$ > "$0"; exec "$@""#.into(),
            pidfile.display().to_string(),
            program.into(),
        ]);
        command.extend(args.iter().cloned());
        s.program("", "docker", command, env)
    }

    /// Ctrl-C to the program [`Net::exec`] started with `pidfile`: SIGINT
    /// to it alone (a terminal's process group; rutis-host puts each
    /// runtime in a group of its own).
    fn ctrl_c(&self, s: &Scenario, container: &str, pidfile: &str) {
        let pidfile = s.dir().join("run").join(pidfile).display().to_string();
        docker(&[
            "exec",
            container,
            "sh",
            "-c",
            r#"kill -INT "$(cat "$0")""#,
            &pidfile,
        ]);
    }

    /// Replace the qdisc of `container`'s `eth0` by `netem <spec>`, or
    /// remove it (`None`).
    fn netem(&self, container: &str, spec: Option<&str>) {
        let mut args = vec!["exec", "-u", "0", container, "tc", "qdisc"];
        match spec {
            Some(spec) => {
                args.extend(["replace", "dev", "eth0", "root", "netem"]);
                args.extend(spec.split(' '));
            }
            None => args.extend(["del", "dev", "eth0", "root"]),
        }
        docker(&args);
    }

    /// What is left in `container` once its programs have exited: its
    /// processes but `--init`'s, `sleep infinity` and the `ps` that lists
    /// them, and the TCP sockets listening there that did not before it
    /// started anything. Waits up to the hang guard for
    /// them to go.
    fn leftover(&self, container: &str) -> Vec<String> {
        let deadline = Instant::now() + rutis_e2e::hang_guard();
        loop {
            let processes = docker(&["exec", container, "ps", "-eo", "pid=,args="]);
            let mut left: Vec<String> = processes
                .lines()
                .map(str::trim)
                .filter(|line| {
                    let args = line.split_once(' ').map_or("", |(_, args)| args.trim());
                    !(line.starts_with("1 ")
                        || args == "sleep infinity"
                        || args.starts_with("ps -eo"))
                })
                .map(|line| format!("process in {container}: {line}"))
                .collect();
            left.extend(
                listening(container)
                    .into_iter()
                    .filter(|socket| !self.idle.contains(socket))
                    .map(|socket| format!("listening in {socket}")),
            );
            if left.is_empty() || Instant::now() >= deadline {
                return left;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// The listening TCP sockets in `container`: `<container> <address:port>`.
fn listening(container: &str) -> Vec<String> {
    docker(&["exec", container, "ss", "-Htln"])
        .lines()
        .filter_map(|line| line.split_whitespace().nth(3))
        .map(|address| format!("{container} {address}"))
        .collect()
}

impl Drop for Net {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.a, &self.b])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("docker")
            .args(["network", "rm", &self.network])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// A throwaway CA and a certificate for [`B_NAME`] it signs, made in a
/// container (no `openssl` needed on the machine): `<scenario>/certs/`.
fn certificates(s: &Scenario, net: &Net) {
    let certs = s.dir().join("certs");
    std::fs::create_dir_all(&certs).expect("the certificates' directory");
    let script = format!(
        r#"set -e
cd "$0"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout ca.key -out ca.pem -days 2 -subj /CN=rutis-e2e-netem-ca 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout b.key -out b.csr -subj /CN={B_NAME} 2>/dev/null
printf '%s\n' 'subjectAltName=DNS:{B_NAME}' 'basicConstraints=critical,CA:FALSE' \
  'keyUsage=critical,digitalSignature' 'extendedKeyUsage=serverAuth' > b.ext
openssl x509 -req -in b.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
  -out b.pem -days 2 -extfile b.ext 2>/dev/null
"#
    );
    docker(&[
        "exec",
        &net.b,
        "sh",
        "-c",
        &script,
        &certs.display().to_string(),
    ]);
}

/// Start node B in its container with `token`; returns once it listens.
fn node(s: &Scenario, net: &Net, token: &str) -> Host {
    let certs = s.dir().join("certs");
    let pythonpath = repo().join("python/rutis").display().to_string();
    let cert = certs.join("b.pem").display().to_string();
    let key = certs.join("b.key").display().to_string();
    let mut b = net.exec(
        s,
        &net.b,
        "b.pid",
        &s.path(""),
        "python3",
        &[
            "-m".into(),
            "rutis".into(),
            format!("listen:wss://0.0.0.0:{PORT}/rutis"),
            "--id".into(),
            "gpu".into(),
            "--peer".into(),
            "main".into(),
            s.path("gpu").display().to_string(),
        ],
        &[
            ("RUTIS_TOKEN", token),
            ("PYTHONPATH", pythonpath.as_str()),
            ("RUTIS_CERT", cert.as_str()),
            ("RUTIS_KEY", key.as_str()),
        ],
    );
    b.expect("rutis: listening on");
    b
}

/// Start host A in its container, dialing B with `token` and trusting the
/// scenario's CA; returns once `infer` runs on B and the probe started.
fn main_host(s: &Scenario, net: &Net, token: &str) -> (Host, Probe) {
    s.link_node_sdk();
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
                    "config": {
                        "peer": "gpu",
                        "dial": format!("wss://{B_NAME}:{PORT}/rutis"),
                        "runtime": "gpu",
                    },
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
    let ca = s.dir().join("certs/ca.pem").display().to_string();
    let mut a = net.exec(
        s,
        &net.a,
        "a.pid",
        &s.path(""),
        &rutis_e2e::host_program().display().to_string(),
        &["run".into(), "rutis.json".into()],
        &[("RUTIS_TOKEN_GPU", token), ("RUTIS_CA", ca.as_str())],
    );
    a.expect("gpu: link connected");
    a.expect("infer: running");
    probe.started(&mut a);
    (a, probe)
}

/// The waits `retrying in <n>s` of A's `link down` lines so far.
fn retry_waits(a: &Host) -> Vec<f64> {
    a.lines()
        .iter()
        .filter(|line| line.text.starts_with("gpu: link down ("))
        .filter_map(|line| line.text.rsplit_once("; retrying in "))
        .filter_map(|(_, wait)| wait.strip_suffix('s')?.parse().ok())
        .collect()
}

/// Phases 1–6 of the module's description, one after the other on the
/// same two containers.
///
/// risk: E12, E1, E11, E6 (over a real network), B7
/// guarantee: bridge/in-flight-call-fails-once, host/link-state-output,
/// bridge/tls-server-verified (the valid certificate side)
#[test]
#[ignore = "needs Linux with Docker and netem: the nightly netem job runs it (netem.yml)"]
fn two_containers_over_tls_through_delay_loss_and_an_outage() {
    if !cfg!(target_os = "linux") {
        eprintln!(
            "S5 netem: skipped, it runs on Linux only (containers sharing this machine's kernel)"
        );
        return;
    }
    // The waits here include a heartbeat timeout (30 s) and a backoff
    // (up to 36 s + a 10 s handshake): a longer hang guard, unless set.
    if std::env::var_os("RUTIS_E2E_TIMEOUT").is_none() {
        std::env::set_var("RUTIS_E2E_TIMEOUT", "120");
    }
    let s = Scenario::new("netem");
    std::fs::create_dir_all(s.dir().join("run")).expect("the scenario's run directory");
    s.write("gpu/infer.py", INFER);
    let net = Net::start(&s);
    certificates(&s, &net);
    let token = s.secret();

    // 1. Baseline.
    let mut b = node(&s, &net, &token);
    let (mut a, probe) = main_host(&s, &net, &token);
    assert_eq!(
        probe.call(&mut a, "infer", "run", json!(["first"])),
        Ok(json!("inferred first"))
    );
    b.expect("infer: executing first");

    // 2. 200 ms delay and 5 % loss on B's side, past a heartbeat timeout.
    net.netem(&net.b, Some("delay 200ms loss 5%"));
    let phase = Instant::now();
    let mut lossy = 0;
    while lossy < 20 || phase.elapsed() < Duration::from_secs(35) {
        lossy += 1;
        let job = format!("lossy-{lossy:03}");
        let started = Instant::now();
        assert_eq!(
            probe.call(&mut a, "infer", "run", json!([job])),
            Ok(json!(format!("inferred {job}"))),
            "a call under delay and loss"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the call took {:?}: the delay is not in effect",
            started.elapsed()
        );
    }
    assert_eq!(
        a.count("gpu: link down"),
        0,
        "the link went down under 200 ms delay and 5 % loss"
    );
    eprintln!(
        "S5 netem: {lossy} calls under delay and loss in {:?}",
        phase.elapsed()
    );
    net.netem(&net.b, None);

    // 3. A full outage while B runs a call.
    a.skip_written();
    b.skip_written();
    let gated = probe.send("infer", "run", json!(["gated"]));
    b.expect("infer: executing gated");
    net.netem(&net.a, Some("loss 100%"));
    net.netem(&net.b, Some("loss 100%"));
    let down = a.wait_for("A reporting the link down", |line| {
        line.starts_with("gpu: link down (") && line.contains("; retrying in ")
    });
    eprintln!("S5 netem: A: {down}");
    assert!(
        down.contains(B_NAME) && down.contains("heartbeat"),
        "the reason names the far end and the heartbeat: {down}"
    );
    let error = probe
        .answer(&mut a, gated)
        .expect_err("a call cut off by the outage fails, it does not succeed");
    assert!(!error.is_empty(), "the failure says why");
    eprintln!("S5 netem: the call cut off failed with: {error}");
    a.expect("infer: waiting for its services");
    probe.event(&mut a, "stopped");
    // B's own heartbeat ends its session: one state line (#434), then it
    // unloads the plugin.
    let ended = b.wait_for("B's state line for the session's end", |line| {
        line.starts_with("rutis: session ended (") && line.ends_with("); waiting for a controller")
    });
    eprintln!("S5 netem: B: {ended}");
    assert_eq!(
        ended, "rutis: session ended (heartbeat timeout); waiting for a controller",
        "B says its heartbeat ended the session"
    );
    b.expect("infer: unloaded");
    // A dial during the outage fails too (it times out), and is retried.
    let failed = a.wait_for("A's failed redial during the outage", |line| {
        line.starts_with("gpu: link down (") && line.contains("; retrying in ")
    });
    eprintln!("S5 netem: A: {failed}");

    // 4. Recovery, within the backoff bound.
    net.netem(&net.a, None);
    net.netem(&net.b, None);
    let restored = Instant::now();
    a.expect("gpu: link connected");
    a.expect("infer: running");
    let reconnect = restored.elapsed();
    eprintln!("S5 netem: reconnected {reconnect:?} after the network came back");
    let bound = Duration::from_secs_f64(LONGEST_WAIT) + HANDSHAKE + Duration::from_secs(5);
    assert!(
        reconnect <= bound,
        "reconnected {reconnect:?} after the network came back, over the backoff bound {bound:?}"
    );
    for wait in retry_waits(&a) {
        assert!(
            (0.4..=LONGEST_WAIT).contains(&wait),
            "a redial waits {wait}s, outside 0.5 s – 30 s ± 20 %"
        );
    }
    probe.started(&mut a);
    s.write("gate", "");
    assert_eq!(
        probe.call(&mut a, "infer", "run", json!(["after"])),
        Ok(json!("inferred after"))
    );
    b.expect("infer: executing after");

    // 5. B restarted with another token: A's redial is refused.
    net.ctrl_c(&s, &net.b, "b.pid");
    b.wait_exit();
    b.wait_output_end();
    let other = s.secret();
    let b2 = node(&s, &net, &other);
    a.wait_for("A reporting the refusal of gpu's credentials", |line| {
        line.starts_with("gpu: link refused: auth-rejected (")
            && line.ends_with("; retrying in 30s")
    });
    assert_eq!(
        a.count("infer: running"),
        2,
        "the row ran on the first B, again after the outage, and not on B with another token"
    );

    // 6. Ctrl-C to both; nothing left in either container.
    net.ctrl_c(&s, &net.a, "a.pid");
    let status = a.wait_exit();
    assert_eq!(status.code(), Some(0), "A after Ctrl-C: {status}");
    a.wait_output_end();
    let mut b2 = b2;
    net.ctrl_c(&s, &net.b, "b.pid");
    b2.wait_exit();
    b2.wait_output_end();
    let mut left = net.leftover(&net.a);
    left.extend(net.leftover(&net.b));
    assert!(
        left.is_empty(),
        "left in the containers:\n{}",
        left.join("\n")
    );

    // Every call ran once on B; none on B with another token.
    assert_eq!(b.count("infer: executing first"), 1);
    for i in 1..=lossy {
        assert_eq!(
            b.count(&format!("infer: executing lossy-{i:03}")),
            1,
            "call lossy-{i:03} under delay and loss ran once on B"
        );
    }
    assert_eq!(
        b.count("infer: executing gated"),
        1,
        "the call cut off ran once on B, not again after the reconnect"
    );
    assert_eq!(b.count("infer: executing after"), 1);
    // Network ends are state lines, not a library's traceback (#434).
    for (name, node) in [("B", &b), ("B with another token", &b2)] {
        assert_eq!(
            node.count("Traceback") + node.count("keepalive ping failed"),
            0,
            "{name} printed a traceback"
        );
    }
    assert_eq!(b2.count("infer: executing"), 0);
    drop(net);
    s.finish();
}
