// definePlugin, and the test tool on a real host (platform cleanup §7.5):
// the plugin runs in the real Node runtime, its config is the host's to
// check, and what crosses between it and the test crosses between
// processes.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '../src/index.mjs'
import { isPlugin, PLUGIN_API } from '../src/advanced.mjs'
import { host, load, PluginTestError } from '../src/testing.mjs'
import weather from './fixtures/weather.mjs'
import store from './fixtures/store.mjs'
import unnamed from './fixtures/unnamed.mjs'

const llm = { ask: async city => city === 'Oslo' ? 'sunny' : 'rain' }

test('definePlugin checks the declaration and stamps the plugin API', () => {
  assert.ok(isPlugin(weather))
  assert.equal(weather.api, PLUGIN_API)
  assert.throws(() => definePlugin({}), /apply/)
  assert.throws(() => definePlugin({ apply() {}, provides: { a: { b: 'maybe' } } }), /sync' or 'async/)
})

test('a plugin runs with the services it injects and provides what it declares', async () => {
  const t = await load(weather, { config: { city: 'Oslo' }, services: { llm } })
  assert.equal(await t.service('weather').today(), 'sunny in Oslo')
  assert.deepEqual(t.service('weather').forecast(['mon']), ['mon: fine'])
  assert.deepEqual(t.provided(), ['weather'])
  assert.deepEqual(t.methods('weather'), { today: 'async', forecast: 'sync', unit: 'sync' })
  assert.throws(() => t.service('other'), { name: 'PluginTestError', message: 'the plugin does not provide other (now)' })
  await t.unload()
})

test('the host fills the config defaults and refuses a config the schema refuses', async () => {
  const t = await load(weather, { config: { city: 'Oslo' }, services: { llm } })
  assert.equal(t.service('weather').unit(), 'C')
  await t.update({ city: 'Bergen', unit: 'F' })
  assert.equal(t.service('weather').unit(), 'F')
  assert.equal(await t.service('weather').today(), 'rain in Bergen')
  await t.unload()
  await assert.rejects(load(weather, { config: {}, services: { llm } }), { name: 'PluginTestError', message: /^weather: .*city/ })
})

test('a plugin using a service it does not declare, which no one provides, is told what to declare', async () => {
  await assert.rejects(load(unnamed), { name: 'PluginTestError', message: /service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject/ })
})

test('a plugin that is not its module\'s default export is refused', async () => {
  const inline = definePlugin({ apply() {} })
  await assert.rejects(load(inline), { name: 'PluginTestError', message: /^load needs a plugin that its module exports as default/ })
  await assert.rejects(load({}), { name: 'PluginTestError', message: 'load needs a plugin made with definePlugin' })
})

test('a plugin that needs a newer plugin API is refused', async () => {
  const future = { ...weather, api: PLUGIN_API + 1 }
  await assert.rejects(load(future, { services: { llm } }), /needs plugin API/)
})

// objects-case(node): strict-testing-tool
// objects-case(node): data-stays-data
// objects-case(node): plain-object-by-reference
// The plugin is in another process: data is copied, an object with methods
// arrives as a proxy (#687 §四) whose methods are asynchronous calls of the
// original, no fields, one proxy per object, the original again when
// passed back.
test('values cross between the test and the plugin as between processes', async () => {
  class Cache {
    #m = new Map(); size = 0
    get(k) { return this.#m.get(k) }
    set(k, v) { this.#m.set(k, v); this.size++ }
  }
  const t = await load(store)
  const item = { n: 1, at: undefined }
  t.service('store').put(item)
  item.n = 2
  assert.deepEqual(t.service('store').last(), { n: 1, at: null }, 'data is copied, undefined inside it is null')
  assert.throws(() => t.service('store').put(Symbol('s')), { code: 'bad-value' })
  const cache = new Cache()
  const warmer = t.service('warmer')
  const { got, seen } = await warmer.warm(cache)
  assert.equal(got, 1)
  assert.equal(cache.size, 1, 'the original object changed')
  assert.deepEqual(seen, {
    shown: '[remote object]',
    field: 'function',
    then: null,
    missing: ['Cache has no method gett (methods: get, set)', 'no-method', { object: 'Cache', method: 'gett', methods: ['get', 'set'] }],
  })
  assert.equal(await warmer.same(cache), true)
  assert.equal(await warmer.same(cache), true, 'passed twice, one proxy')
  assert.equal(await warmer.back(cache), cache, 'passed back, the original')
  assert.equal(await warmer.plain({ ping: () => 'pong' }), 'pong', 'a plain object with a function goes by reference')
  const value = { n: 1 }
  assert.equal(await warmer.data(value), 2)
  assert.equal(value.n, 1, 'data is copied')
  await t.unload()
})

test('a row takes its id and isolate as on a host', async () => {
  const h = host({ services: { llm } })
  const oslo = await h.load(weather, { config: { city: 'Oslo' }, id: 'oslo' })
  assert.equal(oslo.row, 'oslo')
  await assert.rejects(h.load(weather, { config: { city: 'Oslo' }, id: 'oslo' }), { name: 'PluginTestError', message: 'a row oslo is loaded already' })
  // Its own `weather`, in a scope of its own: no clash with oslo's.
  const bergen = await h.load(weather, { config: { city: 'Bergen' }, isolate: { weather: true } })
  assert.equal(bergen.row, 'weather')
  await h.unload()
})

test('PluginTestError is an Error', () => {
  assert.ok(new PluginTestError('x') instanceof Error)
})
