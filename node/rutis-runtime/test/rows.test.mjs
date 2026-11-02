// What a row provides, as the test tool's `provided()` reads it (#1030):
// the runtime's row, not the `rutis/service` events that cross the session
// afterwards. The runner is what registers a row here; the SDK's tests
// (node/rutis/test/testing-plugins.test.mjs) read it through a real host.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { followRow, forgetRow, providedBy } from '../src/rows.mjs'

test('a row is what it provides now, read each time', () => {
  let provides = ['echo']
  followRow('part', () => provides)
  assert.deepEqual(providedBy('part'), ['echo'])
  // A service withdrawn during the row's life shows at once; a row this
  // runtime does not run provides none.
  provides = []
  assert.deepEqual(providedBy('part'), [])
  assert.deepEqual(providedBy('elsewhere'), [])
})

test('an unloaded row provides nothing', () => {
  followRow('part', () => ['echo'])
  forgetRow('part')
  assert.deepEqual(providedBy('part'), [])
})
