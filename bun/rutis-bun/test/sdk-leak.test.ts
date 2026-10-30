// A test that fails before `unload` leaves nothing on the SDK test tool's
// host under `bun test` either: what it loaded is unloaded and its services
// are withdrawn when it ends, so the next test does not fail with `clock is
// already provided by rutis-test` (#933).
import { expect, test } from 'bun:test'
import { host, load } from '../../../node/rutis/src/testing.mjs'
import greeter from './fixtures/sdk/greeter.ts'

const clock = { now: () => 7 }

test.failing('a test that fails before unload', async () => {
  const t = await load(greeter, { services: { clock } })
  expect(t.service('greeter').hello('Ada')).toBe('Hi, Ada at 7')
  await t.unload()
})

test.failing('a test that fails before its host unloads', async () => {
  const h = host({ services: { clock } })
  await h.load(greeter)
  throw new Error('failed')
})

test('the next test loads with the same services', async () => {
  const t = await load(greeter, { services: { clock } })
  expect(t.service('greeter').hello('Ada')).toBe('Hello, Ada at 7')
  await t.unload()
})
