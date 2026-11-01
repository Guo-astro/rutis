// The test driver's registrations (#1004) under Bun: what it registers for a
// row, and the three times it lets go — a load that failed, a row that
// unloaded, and the test ending, which unloads the rows the test did not and
// goes through the same call (node/rutis-runtime/test/definitions.test.mjs).
import { expect, test } from 'bun:test'
import { Definitions, registeredDefinition } from '../src/definitions.ts'

const quiet = { name: 'quiet', apply() {} }

test('a load that failed registers nothing afterwards', () => {
  const definitions = new Definitions()
  const name = definitions.add(quiet)
  expect(registeredDefinition(name)).toBe(quiet)
  definitions.failed(name)
  expect(registeredDefinition(name)).toBeUndefined()
  expect(definitions.names).toEqual([])
})

test('an unloaded row leaves no registration', () => {
  const definitions = new Definitions()
  const name = definitions.add(quiet)
  definitions.loaded('quiet', name)
  expect(definitions.released('quiet')).toBe(name)
  expect(registeredDefinition(name)).toBeUndefined()
  expect(definitions.names).toEqual([])
})

test('the test ending releases every row it loaded, not only the last', () => {
  const definitions = new Definitions()
  const first = definitions.add(quiet)
  const second = definitions.add(quiet)
  definitions.loaded('quiet', first)
  definitions.loaded('quiet-2', second)
  definitions.released('quiet-2')
  definitions.released('quiet')
  expect(registeredDefinition(first)).toBeUndefined()
  expect(registeredDefinition(second)).toBeUndefined()
  expect(definitions.names).toEqual([])
})

test('each load registers a name of its own, with the definition\'s name at the end', () => {
  const definitions = new Definitions()
  expect(definitions.add(quiet)).toBe('rutis-test/definition-1/quiet')
  expect(definitions.add(quiet)).toBe('rutis-test/definition-2/quiet')
  expect(definitions.add({ name: undefined }).split('/').pop()).toBe('plugin')
})
