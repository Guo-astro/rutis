// guarantee: sdk/config-schema
// guarantee: sdk/testing-matches-runtime
// The plugin SDK (`@arcships/rutis`, from this checkout) works under
// `bun test`: its test tool runs the plugin in this Bun runtime, on a real
// host (`rutis-host test`).
import { expect, test } from 'bun:test'
import { host, load } from '../../../node/rutis/src/testing.mjs'
import greeter from './fixtures/sdk/greeter.ts'
import events from './fixtures/sdk/events.ts'
import warmer from './fixtures/sdk/warmer.ts'
import crossing from '../../../node/rutis/test/fixtures/crossing.mjs'
import summary from './fixtures/sdk/summary.ts'
import both from './fixtures/sdk/both.ts'

test('a plugin runs with the SDK test tool', async () => {
  const t = await load(greeter, { config: { greeting: 'Hi' }, services: { clock: { now: () => 7 } } })
  expect(t.service('greeter').hello('Ada')).toBe('Hi, Ada at 7')
  await t.unload()
})

test('the host fills the config defaults and refuses a wrong config', async () => {
  const t = await load(greeter, { services: { clock: { now: () => 1 } } })
  expect(t.service('greeter').hello('Ada')).toBe('Hello, Ada at 1')
  await t.unload()
  const refused = await load(greeter, { config: { greeting: 5 }, services: { clock: { now: () => 1 } } }).then(() => 'loaded', (error: Error) => error.message)
  expect(refused).toBe('greeter: config: greeting should be a string, got 5')
})

// The test tool's events (#464) under Bun too: the host's bus, the modes.
test('the SDK test tool runs events under Bun', async () => {
  const h = host()
  const t = await h.load(events)
  expect(await t.serial('ask', 'Oslo')).toBe('sunny in Oslo')
  expect(await t.waterfall.with(() => 'x')('wrap')).toBe('<x>')
  expect(() => t.emit('rutis/row')).toThrow('rutis/* events are emitted by rutis only: rutis/row')
  await h.unload()
})

// objects-case(bun): data-stays-data
// objects-case(bun): plain-object-by-reference
// The plugin is in another process (#687 §四): an object with methods
// arrives as asynchronous methods of the original, no fields, the
// original again when passed back; data is copied.
test('the SDK test tool passes objects with methods by reference under Bun', async () => {
  class Cache { #m = new Map(); get(k: string) { return this.#m.get(k) } set(k: string, v: unknown) { this.#m.set(k, v) } }
  const t = await load(warmer)
  const cache = new Cache()
  const service = t.service('warmer')
  expect(await service.warm(cache)).toEqual(['[remote object]', 1])
  expect(cache.get('a')).toBe(1)
  expect(await service.back(cache)).toBe(cache)
  expect(await service.plain({ ping: () => 'pong' })).toBe('pong')
  const value = { n: 1 }
  expect(await service.data(value)).toBe(2)
  expect(value.n).toBe(1)
  await t.unload()
})

// The test calls a callback of the plugin while the plugin calls a service
// of the test (#987): Bun's sessions run every incoming call while they
// wait, so under Bun neither side waits for the other for ever.
test('a callback of the plugin while the plugin calls a service of the test, under Bun', async () => {
  const scope = { withLock: (callback: () => unknown) => callback() }
  const t = await load(crossing, { services: { files: { open: () => scope }, other: { ping: () => 'pong' } } })
  expect(await t.service('probe').go()).toEqual(['got', 'pong'])
  await t.unload()
})

// The testing tool's optional services under Bun: not required by the test,
// read with `get`; `use` of one says to add it to `inject` (as Rust's
// `require`); a name in both lists fails the load.
// guarantee: sdk/optional-deps
test('the SDK testing tool runs optional services under Bun', async () => {
  const without = await load(summary, { services: { notes: {} } })
  expect(await without.service('summary').seen()).toEqual({
    llm: false,
    use: ["service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject", 'not-declared', { service: 'llm' }],
    get: ["service other is not available to this plugin, which does not declare it: add 'other' to its optional (or to its inject, if it cannot run without it)", 'not-declared', { service: 'other' }],
  })
  await without.unload()
  const llm = { ask: async (question: string) => `re: ${question}` }
  const withLlm = await load(summary, { services: { notes: {}, llm } })
  // `use` does not read `optional`, even when the service is there.
  expect(await withLlm.service('summary').seen()).toEqual({
    llm: true,
    use: ["service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject", 'not-declared', { service: 'llm' }],
    get: ["service other is not available to this plugin, which does not declare it: add 'other' to its optional (or to its inject, if it cannot run without it)", 'not-declared', { service: 'other' }],
  })
  expect(await withLlm.service('summary').ask('x')).toBe('re: x')
  await withLlm.unload()
  let refused: any
  try { await load(both, { services: { llm } }) } catch (error) { refused = error }
  expect(refused?.name).toBe('PluginTestError')
  expect(refused?.message).toEndWith(': llm is in both inject and optional; keep it in one')
})
