// A row for test/runner-factory.test.mjs (#630): a Cordis plugin whose
// service reads a host service synchronously, so the runtime's frame pump is
// what answers it when the test runs the runtime in its own process.
export const name = 'factory-cordis'
export const inject = ['clock']

export function apply(ctx, config) {
  const clock = ctx.clock
  ctx.provide('factory', {
    label: () => config.label,
    now: () => clock.now(),
  })
}
