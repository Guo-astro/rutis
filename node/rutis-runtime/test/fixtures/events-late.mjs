// A Cordis plugin for test/runner-events.test.mjs: its listener is
// registered after its row loaded, through the service, when a host
// refuses it (events protocol §11).
export const name = 'events-late'

export function apply(ctx) {
  ctx.provide('late-probe', {
    add(name) { ctx.on(name, () => {}); return true },
  })
}
