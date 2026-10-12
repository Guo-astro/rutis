//! A runtime whose host was killed does not stay behind (#232, #353).
//! rutis-host puts each runtime in a process group of its own, so once the
//! host is dead nothing else ends it on Unix: the runtime notices that its
//! channel closed, unloads its rows itself (their cleanups run, those that
//! do not finish are given up at the host's shutdown deadline) and exits,
//! even with a plugin timer that would keep its event loop going. On
//! Windows, rutis-host's job ends it with the host.
//!
//! That the residue checks find a process that does outlive its host, in a
//! group of its own, is the harness's self-test
//! (`residue::tests::an_orphan_outside_the_group_is_found`).

use std::time::{Duration, Instant};

use rutis_e2e::{hang_guard, Scenario};
use serde_json::json;

/// A plugin whose timer nothing clears: the Node runtime's event loop never
/// empties. Its cleanup leaves a mark.
const STRAY: &str = r#"
import { writeFileSync } from 'node:fs'
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  provides: { stray: { ping: 'sync' } },
  apply(ctx, config) {
    setInterval(() => {}, 1 << 30)
    ctx.provide('stray', { ping: () => 'pong' })
    return () => writeFileSync(config.marker, 'cleaned up')
  },
})
"#;

/// A plugin whose cleanup never finishes. Loaded first, it is unloaded
/// last, after the stray row's cleanup ran.
const STUCK: &str = r#"
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  apply() {
    return () => new Promise(() => {})
  },
})
"#;

/// risk: B3 (runtimes outlive a killed host)
#[test]
fn a_runtime_whose_host_was_killed_unloads_its_rows_and_exits() {
    let s = Scenario::new("stray-runtime");
    s.link_node_sdk();
    s.write("stray.ts", STRAY);
    s.write("stuck.ts", STUCK);
    let marker = s.path("cleaned-up");
    s.write_json(
        "rutis.json",
        &json!({
            "runtimes": { "node": {} },
            "rows": [
                { "id": "stuck", "name": "./stuck.ts" },
                { "id": "stray", "name": "./stray.ts", "config": { "marker": marker } },
            ],
        }),
    );
    // The runtime gives up the stuck cleanup after the host's deadline,
    // which the host passes on to it.
    let mut host = s.host(["run", "--shutdown-timeout", "1", "rutis.json"]);
    host.expect("stray: running");
    // The runtime is the host's: by its marker (Unix), in its job
    // (Windows, where the harness sees the program's path only).
    let runtime = if cfg!(windows) {
        "node"
    } else {
        "rutis-runtime"
    };
    let started = s.descendants(&host);
    assert!(
        started
            .iter()
            .any(|(_, command)| command.to_lowercase().contains(runtime)),
        "the Node runtime is not among the host's processes: {started:?}"
    );
    host.kill();
    host.wait_exit();

    // Unix: the runtime exits by itself, once its deadline (1 s) gave up
    // the stuck cleanup; the hang guard only guards against it never
    // exiting. Windows: it ended with the host's job.
    let guard = Instant::now() + Duration::from_secs(1) + hang_guard();
    loop {
        let left = s.descendants(&host);
        if left.is_empty() {
            break;
        }
        assert!(
            Instant::now() < guard,
            "the Node runtime still runs after its host was killed: {left:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Unix: it unloaded the rows itself (Windows: the job ended it first).
    #[cfg(unix)]
    assert_eq!(
        std::fs::read_to_string(&marker).ok().as_deref(),
        Some("cleaned up"),
        "the stray row's cleanup did not run"
    );
    // Nothing is left: no process, socket, port or temporary file.
    s.finish();
}
