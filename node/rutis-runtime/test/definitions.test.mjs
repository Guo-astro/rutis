// The test driver's registrations (#1004): what it registers for a row, and
// the three times it lets go — a load that failed, a row that unloaded, and
// the test ending, which unloads the rows the test did not and goes through
// the same call. A name still registered would resolve a row of this runtime
// to a definition nothing owns any more.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Definitions, registeredDefinition } from '../src/definitions.mjs'

const quiet = { name: 'quiet', apply() {} }

test('a load that failed registers nothing afterwards', () => {
  const definitions = new Definitions()
  const name = definitions.add(quiet)
  assert.equal(registeredDefinition(name), quiet)
  definitions.failed(name)
  assert.equal(registeredDefinition(name), undefined)
  assert.deepEqual(definitions.names, [])
})

test('an unloaded row leaves no registration', () => {
  const definitions = new Definitions()
  const name = definitions.add(quiet)
  definitions.loaded('quiet', name)
  assert.equal(definitions.released('quiet'), name)
  assert.equal(registeredDefinition(name), undefined)
  assert.deepEqual(definitions.names, [])
})

test('the test ending releases every row it loaded, not only the last', () => {
  const definitions = new Definitions()
  const first = definitions.add(quiet)
  const second = definitions.add(quiet)
  definitions.loaded('quiet', first)
  definitions.loaded('quiet-2', second)
  // The test tool unloads them latest first, then the one before it.
  definitions.released('quiet-2')
  definitions.released('quiet')
  assert.equal(registeredDefinition(first), undefined)
  assert.equal(registeredDefinition(second), undefined)
  assert.deepEqual(definitions.names, [])
})

test('each load registers a name of its own, with the definition\'s name at the end', () => {
  const definitions = new Definitions()
  assert.equal(definitions.add(quiet), 'rutis-test/definition-1/quiet')
  assert.equal(definitions.add(quiet), 'rutis-test/definition-2/quiet')
  // It plays the part of a file's name: the host's `base_id` (crates/rutis-host/src/test.rs)
  // gives the row the definition's name, `-2` while taken.
  assert.equal(definitions.add({ name: undefined }).split('/').pop(), 'plugin')
})
