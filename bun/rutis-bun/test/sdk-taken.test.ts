// A test gives a service the host provides already (#988), under
// `bun test` too: only that load fails; the host stays, and the tests
// after it run on it (node/rutis/test/testing-taken.test.mjs).
import { expect, test } from 'bun:test'
import { load } from '../../../node/rutis/src/testing.mjs'
import taken from '../../../node/rutis/test/fixtures/taken.mjs'
import greeter from './fixtures/sdk/greeter.ts'

test('a service the host provides already fails the load that gives it', async () => {
  const refused = await load(taken, { services: { 'rutis/sessions': { x: () => 7 } } }).then(() => 'loaded', (error: Error) => error.message)
  expect(refused).toBe('rutis-test/rutis/sessions: rutis/sessions is already provided by the host: rename it')
})

test('the tests after it run on the same host', async () => {
  const t = await load(greeter, { services: { clock: { now: () => 7 } } })
  expect(t.service('greeter').hello('Ada')).toBe('Hello, Ada at 7')
  await t.unload()
})
