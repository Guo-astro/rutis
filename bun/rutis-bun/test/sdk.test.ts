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
