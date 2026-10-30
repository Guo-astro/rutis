// A test that awaits what a function the plugin handed it returns (run by
// ../testing-later.test.mjs).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '../../src/testing.mjs'
import later from './later.mjs'

test('a handed-over function answers later', async () => {
  const commands = new Map()
  const t = await load(later, {
    services: { commands: { register: async (name, run) => { commands.set(name, run) } } },
  })
  assert.equal(await commands.get('later')(), 'done')
  await t.unload()
})
