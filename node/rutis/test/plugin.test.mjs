import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '../src/index.mjs'
import { isPlugin, PLUGIN_API } from '../src/advanced.mjs'
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
  await assert.rejects(load(sneaky, { services: { llm: {} } }), /is not available to this plugin, which does not declare it in inject/)
  // guarantee: sdk/testing-matches-runtime — the runtimes' texts, codes and data (#677).
  await assert.rejects(load(sneaky, { services: { llm: {} } }), { message: "service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject", code: 'not-declared', data: { service: 'llm' } })
  const caller = definePlugin({ inject: ['llm'], apply(ctx) { ctx.use('llm').other() } })
  await assert.rejects(load(caller, { services: { llm: { ask: async () => '', tell() {} } } }), { name: 'PluginTestError', message: 'llm has no method other (methods: ask, tell)', code: 'no-method', data: { service: 'llm', method: 'other', methods: ['ask', 'tell'] } })
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
  // guarantee: sdk/testing-matches-runtime — the runtimes' words, code and
  // data for a value that cannot be sent, nested ones too (#677).
  assert.throws(() => t.service('store').put(Symbol('s')), { message: 'store.put: argument 1 cannot be sent: a symbol cannot cross between processes', code: 'bad-value', data: { part: 'argument', argument: 1 } })
  assert.throws(() => t.service('store').put({ opts: { n: [Symbol('s')] } }), { message: 'store.put: argument 1 cannot be sent: a symbol cannot cross between processes', code: 'bad-value', data: { part: 'argument', argument: 1 } })
  // Nesting: 64 levels cross, 65 do not (#279).
  const nest = depth => Array.from({ length: depth }).reduce(inner => [inner], 'bottom')
  t.service('store').put(nest(64))
  assert.throws(() => t.service('store').put(nest(65)), /nested deeper than 64 levels cannot cross/)
  assert.throws(() => t.service('store').wrong(), /declared sync but returned a promise/)
  // An empty slot of an array arrives as null, as the runtimes send it
  // (#687 §七); a whole argument keeps undefined.
  // eslint-disable-next-line no-sparse-arrays
  t.service('store').put([1, , 3])
  const crossed = kept.at(-1)
  assert.ok(1 in crossed, 'the empty slot is filled')
  assert.deepEqual(crossed, [1, null, 3])
  t.service('store').put(undefined)
  assert.equal(kept.at(-1), undefined)
})

test('a plugin that needs a newer plugin API is refused', async () => {
  const future = { ...weather, api: PLUGIN_API + 1 }
  await assert.rejects(load(future, { services: { llm: {} } }), /needs plugin API/)
})

// guarantee: sdk/leaf-events (parallel waits; aggregate errors in registration order)
test('parallel from the test waits for every listener and aggregates failures', async () => {
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
  }))
  let settled = false
  const done = t.parallel('tick', payload)
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
  }))
  await t.parallel('tick', 1)
  assert.deepEqual(received, [1, 1])
  off()
  off()
  assert.equal(context.emit('tick', 2), undefined)
  assert.deepEqual(received, [1, 1, 2], 'the other registration remains active')
  await assert.rejects(t.unload(), /cleanup failed/)
  // Unloaded, the row emits nothing, as on the real runtimes.
  assert.throws(() => context.emit('tick', 3), { name: 'BindingError', message: 'row row1 is not loaded in this runtime' })
  await t.host.parallel('early', 4)
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
  assert.throws(() => t.emit('internal/status'), /event internal\/status is internal to the runtime/)
  // A non-string name is not Cordis's `this` argument: `emit(null, 'internal/service')` does not dispatch.
  assert.throws(() => context.on(null, () => {}), TypeError)
  assert.throws(() => context.emit(null, 'internal/service'), TypeError)
  assert.throws(() => context.emit({}, 'internal/service'), TypeError)
  assert.throws(() => t.emit(null, 'internal/status'), TypeError)
  // rutis/* can be listened to, never emitted (ReservedEvent).
  assert.ok(context.on('rutis/row', () => {}))
  assert.throws(() => context.emit('rutis/row', {}), { name: 'ReservedEvent', message: 'rutis/* events are emitted by rutis only: rutis/row' })
  assert.throws(() => t.serial('rutis/row'), { name: 'ReservedEvent' })
  await t.unload()
})

// guarantee: sdk/testing-matches-runtime
// Calls through a service the test gives, and through objects and
// functions it returns, are named as the runtimes name them; what such a
// service throws arrives with its code and data and this side's stack.
test('the testing tool names calls and refusals as the runtimes do (#677)', async () => {
  const seen = {}
  const remote = Object.assign(new Error('no entry'), { code: 'NotFound', data: { id: 7 } })
  remote.stack = 'Error: no entry\n    at far (remote.go:12)'
  const services = {
    llm: {
      open() { return { run() {}, bad() { return { n: Symbol('s') } } } },
      later() { return x => x },
      fail() { throw remote },
    },
  }
  const caller = definePlugin({
    inject: ['llm'],
    async apply(ctx) {
      const llm = ctx.use('llm')
      // An object passed by reference has asynchronous methods (#687).
      const attempt = async (key, run) => { try { await run() } catch (error) { seen[key] = { message: error.message, code: error.code, data: error.data, stack: error.stack } } }
      await attempt('method', () => llm.open().run(Symbol('s')))
      await attempt('result', () => llm.open().bad())
      await attempt('function', () => llm.later()(Symbol('s')))
      await attempt('nested', () => llm.open().run({ a: [1, { b: Symbol('s') }] }))
      await attempt('thrown', () => llm.fail())
    },
  })
  await load(caller, { services })
  assert.deepEqual(seen.method, { message: 'the method run: argument 1 cannot be sent: a symbol cannot cross between processes', code: 'bad-value', data: { part: 'argument', argument: 1 }, stack: seen.method.stack })
  assert.deepEqual([seen.result.message, seen.result.data], ['the method bad: the result cannot be sent: a symbol cannot cross between processes', { part: 'result' }])
  assert.equal(seen.function.message, 'a function: argument 1 cannot be sent: a symbol cannot cross between processes')
  assert.equal(seen.nested.message, 'the method run: argument 1 cannot be sent: a symbol cannot cross between processes')
  assert.deepEqual([seen.thrown.message, seen.thrown.code, seen.thrown.data], ['no entry', 'NotFound', { id: 7 }])
  assert.doesNotMatch(seen.thrown.stack, /remote\.go/)
})

// objects-case(node): strict-testing-tool
// objects-case(node): data-stays-data
// objects-case(node): plain-object-by-reference
// Strict mode gives an object with methods as another process would (#687
// §四): a proxy whose methods are asynchronous calls of the original, no
// fields, one proxy per object, the original again when passed back.
test('strict mode passes objects with methods by reference, as between processes', async () => {
  class Cache {
    #m = new Map(); size = 0
    get(k) { return this.#m.get(k) }
    set(k, v) { this.#m.set(k, v); this.size++ }
  }
  const seen = {}
  const plugin = definePlugin({
    provides: { warmer: { warm: 'async', same: 'async', back: 'async', plain: 'async', data: 'async' } },
    apply(ctx) {
      let first
      ctx.provide('warmer', {
        async warm(cache) {
          seen.shown = String(cache)
          seen.field = typeof cache.size
          seen.then = cache.then
          await cache.set('a', 1)
          seen.missing = await cache.gett().catch(error => [error.message, error.code, error.data])
          return cache.get('a')
        },
        same(cache) { first ??= cache; return first === cache },
        back(cache) { return cache },
        plain(object) { return object.ping() },
        data(value) { value.n = 2; return value.n },
      })
    },
  })
  const t = await load(plugin)
  const cache = new Cache()
  const warmer = t.service('warmer')
  assert.equal(await warmer.warm(cache), 1)
  assert.equal(cache.size, 1, 'the original object changed')
  assert.deepEqual(seen, {
    shown: '[remote object]',
    field: 'function',
    then: undefined,
    missing: ['Cache has no method gett (methods: get, set)', 'no-method', { object: 'Cache', method: 'gett', methods: ['get', 'set'] }],
  })
  assert.equal(await warmer.same(cache), true)
  assert.equal(await warmer.same(cache), true, 'passed twice, one proxy')
  assert.equal(await warmer.back(cache), cache, 'passed back, the original')
  assert.equal(await warmer.plain({ ping: () => 'pong' }), 'pong', 'a plain object with a function goes by reference')
  const value = { n: 1 }
  assert.equal(await warmer.data(value), 2)
  assert.equal(value.n, 1, 'data is copied')
})
