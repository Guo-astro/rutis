// The testing tool refuses a plugin's provide of a name the host provides,
// as the host and Cordis do (#684 H4).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '../src/index.mjs'
import { host, load } from '../src/testing.mjs'

const wordcount = definePlugin({
  name: 'wordcount',
  inject: [],
  apply(ctx) {
    ctx.provide('notes', { async get() { return null } })
  },
})

const notes = () => ({ async put() {}, async get() { return null } })

test('providing a name in services fails', async () => {
  await assert.rejects(
    load(wordcount, { services: { notes: notes() } }),
    { name: 'PluginTestError', message: 'wordcount: notes is already provided by the host' },
  )
})

test("a host's services and a row's own services are both the host's", async () => {
  for (const [hostServices, own] of [[{ notes: notes() }, {}], [{}, { notes: notes() }]]) {
    const h = host({ services: hostServices })
    await assert.rejects(
      h.load(wordcount, { services: own }),
      { name: 'PluginTestError', message: 'wordcount: notes is already provided by the host' },
    )
    await h.unload()
  }
})

test('a name not in services is provided as before', async () => {
  const t = await load(wordcount, { services: {} })
  assert.equal(await t.service('notes').get('k'), null)
  await t.unload()
})

test('a plugin without a name is named by its row', async () => {
  const unnamed = definePlugin({ inject: [], apply(ctx) { ctx.provide('notes', notes()) } })
  const h = host({ services: { notes: notes() } })
  await assert.rejects(h.load(unnamed, { row: 'r1' }), { message: 'r1: notes is already provided by the host' })
  await h.unload()
})
