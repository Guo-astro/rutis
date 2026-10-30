// Tests that fail before `unload` (run by ../testing-leak.test.mjs): the
// next test loads with the same services as if nothing had been left.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host, load } from '../../src/testing.mjs'
import weather from './weather.mjs'

const llm = { ask: async () => 'sunny' }

test('fails before unload', async () => {
  const t = await load(weather, { config: { city: 'Oslo' }, services: { llm } })
  assert.equal(await t.service('weather').today(), 'rainy in Oslo')
  await t.unload()
})

test('fails before the host unloads', async () => {
  const h = host({ services: { llm } })
  await h.load(weather, { config: { city: 'Oslo' } })
  throw new Error('failed')
})

test('loads with the same services', async () => {
  const t = await load(weather, { config: { city: 'Oslo' }, services: { llm } })
  assert.equal(await t.service('weather').today(), 'sunny in Oslo')
  await t.unload()
})
