// Plugin definitions a test hands to the host as rows (`@arcships/rutis/testing`):
// the driver registers the definition it got from a test under a name, the
// host asks this runtime to load that name as a row, and the runtime resolves
// the name to the definition instead of a module (the way Python's testing
// tool puts one in `sys.modules`). Only this process's test driver registers
// and releases; a name no longer registered resolves like any other unknown
// entry.
const definitions = new Map()

/**
 * Register `definition` under `name`, so a row whose entry is `name` loads it.
 */
export function registerDefinition(name, definition) {
  definitions.set(name, definition)
}

/**
 * The definition registered under `name`, if any.
 */
export function registeredDefinition(name) {
  return definitions.get(name)
}

/**
 * Forget `name`: the rows loaded from it are unloaded by then.
 */
export function releaseDefinition(name) {
  definitions.delete(name)
}
