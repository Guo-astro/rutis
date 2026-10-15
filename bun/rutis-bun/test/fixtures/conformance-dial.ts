// The dial conformance plugin (crates/rutis-bridge/tests/dial_conformance.rs),
// for the Bun runtime: it says on stderr when its row loads and unloads,
// with its config, so the suite sees each session's rows come and go. Plain
// JavaScript: the suite installs it as a package's index.mjs.
export function apply(_ctx, config) {
  process.stderr.write(`conformance: loaded ${config}\n`)
  return () => { process.stderr.write(`conformance: unloaded ${config}\n`) }
}
