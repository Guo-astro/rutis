// A test whose call of the plugin never returns: node:test's timeout
// fails it (run by ../testing-stuck.test.mjs). STUCK_TIMEOUT, its time
// limit, is 1000 ms; a test that gives it a shorter one has its timeout
// land other places: while `load` is still starting the host (#997).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '../../src/testing.mjs'
import stuck from './stuck.mjs'

test('a call that never returns', { timeout: Number(process.env.STUCK_TIMEOUT ?? 1000) }, async () => {
  const t = await load(stuck)
  await t.service('stuck').wait()
})

// The test after it: a host the timeout gave up on is started again rather
// than left to fail this load too (#997).
test('a plugin loads after a test a timeout ended', async () => {
  const t = await load(stuck, { id: 'after' })
  assert.equal(typeof t.service('stuck').wait, 'function')
  await t.unload()
})
