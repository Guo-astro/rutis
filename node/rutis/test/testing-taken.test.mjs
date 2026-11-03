// A test gives a service the host provides already (#988): only that
// load fails; the host stays, and the tests after it run on it. The
// service failed the test tool's own plugin, which provided it, and every
// later test failed with `the host disconnected`. `rutis/sessions` is one the
// host provides and the runtime does not know of (rutis refuses it once
// it is announced, not when it is provided).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '../src/testing.mjs'
import taken from './fixtures/taken.mjs'
import echo from './fixtures/echo.mjs'

test('a service the host provides already fails the load that gives it', async () => {
  await assert.rejects(load(taken, { services: { 'rutis/sessions': { x: () => 7 } } }), {
    message: 'rutis-test/rutis/sessions: rutis/sessions is already provided by the host: rename it',
  })
})

test('the tests after it run on the same host', async () => {
  const t = await load(echo)
  assert.equal(t.service('echo').echo('a'), 'a')
  await t.unload()
})
