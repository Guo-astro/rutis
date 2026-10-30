// A test whose call of the plugin never returns: node:test's timeout
// fails it (run by ../testing-stuck.test.mjs).
import { test } from 'node:test'
import { load } from '../../src/testing.mjs'
import stuck from './stuck.mjs'

test('a call that never returns', { timeout: 1000 }, async () => {
  const t = await load(stuck)
  await t.service('stuck').wait()
})
