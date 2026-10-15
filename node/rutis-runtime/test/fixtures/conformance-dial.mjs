// The dial conformance plugin (crates/rutis-bridge/tests/dial_conformance.rs),
// for the Node runtime: it says on stderr when its row loads and unloads,
// with its config, so the suite sees each session's rows come and go.
export function apply(ctx, config) {
  process.stderr.write(`conformance: loaded ${config}\n`)
  ctx.effect(() => () => process.stderr.write(`conformance: unloaded ${config}\n`))
}
