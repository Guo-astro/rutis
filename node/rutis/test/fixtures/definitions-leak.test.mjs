// Tests that leave the rows they loaded (run by ../testing-definitions.test.mjs):
// what a test loaded is unloaded when it ends, so the next one loads the same
// definition as if nothing had been left (#1004).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '../../src/testing.mjs'
import handover from './handover.mjs'

const handed = new Map()
const parts = { async register(name, definition) { handed.set(name, definition); return definition } }

test('leaves its definition row loaded', async () => {
  await load(handover, { services: { parts } })
  const t = await load(handed.get('part'))
  assert.equal(t.service('echo').say(), 'part')
  throw new Error('failed before unload')
})

test('loads the same definition again', async () => {
  const t = await load(handed.get('part'))
  assert.equal(t.service('echo').say(), 'part')
  await t.unload()
})
