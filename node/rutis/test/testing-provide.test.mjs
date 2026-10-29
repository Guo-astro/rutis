// A plugin providing a name the test's services already provide is
// refused by the host, in the host's words (#684 H4).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host, load } from '../src/testing.mjs'
import wordcount from './fixtures/wordcount.mjs'

const notes = () => ({ async put() {}, async get() { return null } })

test('a name the test does not provide is the plugin\'s', async () => {
  const t = await load(wordcount, { services: {} })
  assert.equal(await t.service('notes').get('k'), null)
  await t.unload()
})

test('providing a name the test provides fails, a host\'s or a row\'s own', async () => {
  for (const [shared, own] of [[{ notes: notes() }, {}], [{}, { notes: notes() }]]) {
    const h = host({ services: shared })
    await assert.rejects(h.load(wordcount, { services: own }), { name: 'PluginTestError', message: /^wordcount: Error: notes is already provided by rutis-test$/ })
    await h.unload()
  }
})
