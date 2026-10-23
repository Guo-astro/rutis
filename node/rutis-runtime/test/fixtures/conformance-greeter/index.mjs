// The node conformance suite's hosted plugin: records through `probe` when
// it starts and when it stops; with `fail` in its config, its apply fails
// with that message instead.
export const name = 'conformance-greeter'
export const inject = ['probe']
export function apply(ctx, config) {
  if (config.fail) throw new Error(config.fail)
  ctx.probe.record(`greeter: ${config.text}`)
  ctx.effect(() => () => ctx.probe.record(`greeter gone: ${config.text}`))
}
