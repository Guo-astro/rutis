// The plugin SDK (`@arcships/rutis`, from this checkout) works under
// `bun test`: plugins written with it are tested without a host.
import { expect, test } from 'bun:test'
import { definePlugin } from '../../../node/rutis/src/index.mjs'
import { host, load } from '../../../node/rutis/src/testing.mjs'

const greeter = definePlugin({
  inject: ['clock'],
  provides: { greeter: { hello: 'sync' } },
  apply(ctx: any, config: any) {
    const clock = ctx.use('clock')
    ctx.provide('greeter', { hello: (name: string) => `${config.greeting}, ${name} at ${clock.now()}` })
  },
})

test('a plugin runs with the SDK testing helpers', async () => {
  const t = await load(greeter, { config: { greeting: 'Hi' }, services: { clock: { now: () => 7 } } })
  expect(t.service('greeter').hello('Ada')).toBe('Hi, Ada at 7')
  await t.unload()
})

// The testing tool's events (#464) under Bun too: one bus, the five modes.
test('the SDK testing tool runs events under Bun', async () => {
  const h = host()
  const t = await h.load(definePlugin({
    apply(ctx: any) {
      ctx.on('ask', (city: string) => `sunny in ${city}`)
      ctx.on('wrap', (next: () => string) => `<${next()}>`)
    },
  }))
  expect(await t.serial('ask', 'Oslo')).toBe('sunny in Oslo')
  expect(await t.waterfall.with(() => 'x')('wrap')).toBe('<x>')
  expect(() => t.emit('rutis/row')).toThrow('rutis/* events are emitted by rutis only: rutis/row')
  await h.unload()
})

// objects-case(bun): strict-testing-tool
// objects-case(bun): data-stays-data
// objects-case(bun): plain-object-by-reference
// Strict mode under Bun gives an object with methods as another process
// would (#687 §四): asynchronous methods of the original, no fields, one
// proxy per object, the original again when passed back; data is copied.
test('the SDK testing tool passes objects with methods by reference under Bun', async () => {
  class Cache { #m = new Map(); get(k: string) { return this.#m.get(k) } set(k: string, v: unknown) { this.#m.set(k, v) } }
  const t = await load(definePlugin({
    provides: { warmer: { warm: 'async', back: 'async', plain: 'async', data: 'async' } },
    apply(ctx: any) {
      ctx.provide('warmer', {
        async warm(cache: any) { await cache.set('a', 1); return [String(cache), await cache.get('a')] },
        back(cache: unknown) { return cache },
        plain(object: any) { return object.ping() },
        data(value: any) { value.n = 2; return value.n },
      })
    },
  }))
  const cache = new Cache()
  const warmer = t.service('warmer')
  expect(await warmer.warm(cache)).toEqual(['[remote object]', 1])
  expect(cache.get('a')).toBe(1)
  expect(await warmer.back(cache)).toBe(cache)
  expect(await warmer.plain({ ping: () => 'pong' })).toBe('pong')
  const value = { n: 1 }
  expect(await warmer.data(value)).toBe(2)
  expect(value.n).toBe(1)
  await t.unload()
})
