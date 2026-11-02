// What each row of this runtime provides now, by the id the host gave it
// (`rows.load`'s `id`): the test driver (`rutis-test`) answers `provided()`
// from here, as Python's test tool reads its runner's row, instead of
// waiting for the `rutis/service` events that cross the session afterwards
// (#1030). rutis publishes a row's services before it hears that the row
// loaded (`docs/guarantees.md`, `loader/announced-services`); the event that
// says so arrives a little later, so a test reading the events right after
// `load` may see none.
//
// It sits at the top of `src/`, not under `src/testing/`: a copy of this
// package's source (crates/rutis-bridge/tests/bun_runtime.rs) leaves the
// testing tool's folder out, and `runner` registers rows
// (docs/design-test-tool-definition-load-2026-11-01.md).
const rows = new Map<string, () => string[]>()

/**
 * What the row `id` provides, from now until it unloads: `services` is read
 * each time, so a withdrawal during the row's life shows at once.
 */
export function followRow(id: string, services: () => string[]): void {
  rows.set(id, services)
}

/** The row `id` unloaded: nothing of it is provided here. */
export function forgetRow(id: string): void {
  rows.delete(id)
}

/** The names the row `id` provides now, sorted; none for a row not loaded here. */
export function providedBy(id: string): string[] {
  return rows.get(id)?.() ?? []
}
