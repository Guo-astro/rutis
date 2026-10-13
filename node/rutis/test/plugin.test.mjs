import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin, isPlugin, PLUGIN_API } from '../src/index.mjs'
import { load, PluginTestError } from '../src/testing.mjs'

class Weather {
  constructor(llm, city) { this.llm = llm; this.city = city }
  async today() { return `${await this.llm.ask(this.city)} in ${this.city}` }
  forecast(days) { return days.map(day => `${day}: fine`) }
}

const weather = definePlugin({
  inject: ['llm'],
  provides: { weather: { today: 'async', forecast: 'sync' } },
  apply(ctx, config) {
    ctx.provide('weather', new Weather(ctx.use('llm'), config.city))
    // Latest first, as the runtime runs them: the returned cleanup, then this.
    let cleaned = false
    ctx.effect(() => { if (!cleaned) throw new Error('the returned cleanup runs first') })
    return () => { cleaned = true }
  },
})

test('definePlugin checks the declaration and stamps the plugin API', () => {
  assert.ok(isPlugin(weather))
  assert.equal(weather.api, PLUGIN_API)
  assert.throws(() => definePlugin({}), /apply/)
  assert.throws(() => definePlugin({ apply() {}, provides: { a: { b: 'maybe' } } }), /sync' or 'async/)
})

test('a plugin runs with the services it injects and provides what it declares', async () => {
  const t = await load(weather, { config: { city: 'Oslo' }, services: { llm: { ask: async () => 'sunny' } } })
  assert.equal(await t.service('weather').today(), 'sunny in Oslo')
  assert.deepEqual(t.service('weather').forecast(['mon']), ['mon: fine'])
  assert.deepEqual(t.provided(), ['weather'])
  await t.unload()
  assert.deepEqual(t.provided(), [])
})

test('undeclared services, missing services and missing methods are errors', async () => {
  await assert.rejects(load(weather, { config: {} }), /injects llm/)
  const sneaky = definePlugin({ apply(ctx) { ctx.use('llm') } })
  await assert.rejects(load(sneaky, { services: { llm: {} } }), /without declaring it in inject/)
  const incomplete = definePlugin({ provides: { a: { run: 'sync' } }, apply(ctx) { ctx.provide('a', {}) } })
  await assert.rejects(load(incomplete), /no such method/)
  const t = await load(weather, { config: { city: 'x' }, services: { llm: { ask: async () => '' } } })
  assert.throws(() => t.service('other'), PluginTestError)
})

test('strict mode crosses values as between processes', async () => {
  const kept = []
  const plugin = definePlugin({
    provides: { store: { put: 'sync', take: 'sync', wrong: 'sync' } },
    apply(ctx) {
      ctx.provide('store', {
        put(item) { kept.push(item) },
        take() { return kept[0] },
        wrong() { return Promise.resolve(1) },
      })
    },
  })
  const t = await load(plugin)
  const item = { n: 1 }
  t.service('store').put(item)
  item.n = 2
  assert.equal(t.service('store').take().n, 1, 'data is copied, not shared')
  assert.throws(() => t.service('store').put(1n), /bigint cannot cross/)
  // Nesting: 64 levels cross, 65 do not (#279).
  const nest = depth => Array.from({ length: depth }).reduce(inner => [inner], 'bottom')
  t.service('store').put(nest(64))
  assert.throws(() => t.service('store').put(nest(65)), /nested deeper than 64 levels cannot cross/)
  assert.throws(() => t.service('store').wrong(), /declared sync but returned a promise/)
})

test('a plugin that needs a newer plugin API is refused', async () => {
  const future = { ...weather, api: PLUGIN_API + 1 }
  await assert.rejects(load(future, { services: { llm: {} } }), /needs plugin API/)
})

// guarantee: sdk/leaf-events (host emission waits; aggregate errors)
test('host emission waits for every listener and aggregates failures', async () => {
  const gate = Promise.withResolvers()
  const payload = { n: 1 }
  const t = await load(definePlugin({
    apply(ctx) {
      ctx.on('tick', value => {
        value.n = 2
        throw new TypeError('sync failure')
      })
      ctx.on('tick', async () => {
        await gate.promise
        throw new RangeError('async failure')
      })
    },
  }), { emits: ['tick'] })
  let settled = false
  const done = t.emit('tick', payload)
    .finally(() => { settled = true })
    .catch(error => error)
  assert.equal(payload.n, 1, 'event data is copied across the boundary')
  await Promise.resolve()
  assert.equal(settled, false, 'the async listener is still pending')
  gate.resolve()
  const error = await done
  assert.ok(error instanceof AggregateError)
  assert.deepEqual(error.errors.map(item => [item.name, item.message]), [
    ['TypeError', 'sync failure'], ['RangeError', 'async failure'],
  ])
  await t.unload()
})

// guarantee: sdk/leaf-events (disposers; a failed unload clears listeners)
test('duplicate listeners dispose independently and failed unload clears listeners', async () => {
  const received = []
  const cleaned = []
  const handler = value => { received.push(value) }
  let context, off
  const t = await load(definePlugin({
    apply(ctx) {
      context = ctx
      ctx.on('early', handler)
      ctx.effect(() => {
        cleaned.push('first')
        throw new Error('cleanup failed')
      })
      off = ctx.on('tick', handler)
      ctx.effect(() => { cleaned.push('middle') })
      ctx.on('tick', handler)
      ctx.effect(() => { cleaned.push('last') })
    },
  }), { emits: ['tick'] })
  await t.emit('tick', 1)
  assert.deepEqual(received, [1, 1])
  off()
  off()
  assert.equal(context.emit('tick', 2), undefined)
  assert.deepEqual(received, [1, 1, 2], 'the other registration remains active')
  await assert.rejects(t.unload(), /cleanup failed/)
  context.emit('tick', 3)
  context.emit('early', 4)
  assert.deepEqual(received, [1, 1, 2], 'even the listener before the failed cleanup is removed')
  assert.deepEqual(cleaned, ['last', 'middle', 'first'], 'manual off skips no cleanup')
})

// guarantee: sdk/leaf-events (internal/* refused)
test('internal/* events are refused, as on the real runtimes', async () => {
  let context
  const plugin = definePlugin({ apply(ctx) { context = ctx } })
  const t = await load(plugin)
  assert.throws(() => context.on('internal/dispatch', () => {}), /event internal\/dispatch is internal to the runtime/)
  assert.throws(() => context.emit('internal/service'), /event internal\/service is internal to the runtime/)
  await assert.rejects(t.emit('internal/status'), /event internal\/status is internal to the runtime/)
  await t.unload()
  await assert.rejects(load(plugin, { emits: ['internal/service'] }), /is internal to the runtime and cannot be emitted from rutis/)
})
