// Plugin definitions a test hands to the host as rows (`@arcships/rutis/testing`):
// the driver registers the definition it got from a test under a name, the
// host asks this runtime to load that name as a row, and the runtime resolves
// the name to the definition instead of a module (the way Python's testing
// tool puts one in `sys.modules`). Nothing else registers: a name no longer
// registered resolves like any other unknown entry.
//
// It sits at the top of `src/`, not under `src/testing/`: a copy of this
// package's `src` files (crates/rutis-bridge/tests/bun_runtime.rs) leaves the
// testing tool's folder out, and `runner` resolves rows (docs/design-test-tool-definition-load-2026-11-01.md).
const definitions = new Map()

/**
 * The definition registered under `name`, if any.
 */
export function registeredDefinition(name) {
  return definitions.get(name)
}

/**
 * What the test driver registered, and when it lets go: the three paths are a
 * load that failed, a row that unloaded, and the test ending (which unloads
 * the rows it did not, the same call). Kept in one place so each is a test of
 * its own, without a host.
 */
export class Definitions {
  // row id -> the name it was loaded by.
  #names = new Map()
  #next = 0

  /**
   * Register `definition`, and answer the name a row loads it by. Its last
   * segment is the definition's name, which is how the host gives that name's
   * row its id (`-2`, `-3`… while taken).
   */
  add(definition) {
    const name = `rutis-test/definition-${++this.#next}/${definition.name ?? 'plugin'}`
    definitions.set(name, definition)
    return name
  }

  /** The row `id` was loaded from `name`. */
  loaded(id, name) {
    this.#names.set(id, name)
  }

  /** The load of `name` failed: nothing will refer to it. */
  failed(name) {
    definitions.delete(name)
  }

  /** The row `id` unloaded: forget the definition it was loaded from. */
  released(id) {
    const name = this.#names.get(id)
    this.#names.delete(id)
    if (name !== undefined) definitions.delete(name)
    return name
  }

  /** The names registered now, for the tests of this bookkeeping. */
  get names() {
    return [...definitions.keys()]
  }
}
