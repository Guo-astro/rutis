//! A runtime that outlives its killed host is found (#232). rutis-host puts
//! each runtime in a process group of its own; once the host is dead the
//! runtime's parent is init (launchd), so on macOS looking for the host's
//! group or children found nothing, and the residue checks passed while
//! the runtime still ran. They look for the scenario's marker now.

use rutis_e2e::Scenario;
use serde_json::json;

/// A plugin whose timer nothing clears: the Node runtime's event loop never
/// empties, so the runtime does not exit when its channel closes.
const STRAY: &str = r#"
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  provides: { stray: { ping: 'sync' } },
  apply(ctx) {
    setInterval(() => {}, 1 << 30)
    ctx.provide('stray', { ping: () => 'pong' })
  },
})
"#;

/// risk: B3 (runtimes outlive a killed host)
#[test]
fn a_runtime_that_outlives_its_killed_host_is_found() {
    let s = Scenario::new("stray-runtime");
    s.link_node_sdk();
    s.write("stray.ts", STRAY);
    s.write_json(
        "rutis.json",
        &json!({
            "runtimes": { "node": {} },
            "rows": [{ "id": "stray", "name": "./stray.ts" }],
        }),
    );
    let mut host = s.host(["run", "rutis.json"]);
    host.expect("stray: running");
    host.kill();
    host.wait_exit();

    // Unix: the runtime stays, and is found by its marker.
    #[cfg(unix)]
    {
        let left = s.descendants(&host);
        assert!(
            left.iter()
                .any(|(_, command)| command.contains("rutis-runtime")),
            "the Node runtime is not found: {left:?}"
        );
        for (pid, _) in left {
            // SAFETY: kill(2) on processes the scenario started.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
    // Windows: rutis-host's job, closed when the host ends, ends the
    // runtime with it; the residue checks wait for the host's job to empty.
    s.finish();
}
