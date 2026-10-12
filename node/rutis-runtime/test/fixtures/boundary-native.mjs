// The native Cordis reference for `crates/rutis-bridge/tests/boundary_rules.rs`:
// `node boundary-native.mjs <scenario>` runs `boundary-rules.mjs` in one
// Cordis Context, with the listeners the test registers on the rutis side
// registered here as ordinary Cordis listeners, at the same points in time.
// It prints { log, result } as JSON.
import { Context } from '@deepseek-ai/cordis'
import { apply, inject, Log } from './boundary-rules.mjs'

const scenario = process.argv[2]
const ctx = new Context()
const log = new Log()
ctx.provide('log', log)
const mark = tag => { log.push(tag) }
const on = (name, listener) => ctx.on(name, listener)

// What the rutis side registers before the plugin is mounted.
const before = {
  order: () => on('boundary/order', () => { mark('R1') }),
  back: () => on('boundary/back', () => { mark('R1') }),
  serial: () => on('boundary/chain', async () => { mark('R:start'); await Promise.resolve(); mark('R:end') }),
  link: () => {
    on('boundary/link', () => { mark('R1') })
  },
  bail: () => on('boundary/chain', () => { mark('R') }),
  waterfall: () => on('boundary/wf', (value, next) => { mark('R'); return next() }),
  'waterfall-async': () => on('boundary/wf-async', (value, next) => { mark('R'); return next() }),
  internal: () => on('internal/service', name => { mark(`R:${String(name)}`) }),
  host: () => {},
}
// What the rutis side registers after the mount.
const after = {
  order: () => on('boundary/order', () => { mark('R2') }),
  back: () => on('boundary/back', () => { mark('R2') }),
  link: () => {
    on('boundary/link', () => { mark('R2'); return 'R2' })
    on('boundary/link', () => { mark('R3') })
  },
}

before[scenario]()
const fiber = ctx.plugin({ name: 'boundary-rules', inject, apply })
await fiber.await()
after[scenario]?.()
const probe = ctx.get('probe')
probe.late()

let result = null
switch (scenario) {
  case 'order': await probe.order(); break
  case 'back': await ctx.serial('boundary/back'); break
  case 'serial': result = await probe.serial(); break
  case 'link': result = await ctx.serial('boundary/link') ?? null; break
  case 'bail': result = probe.bail(); break
  case 'waterfall': result = await probe.waterfall(); break
  case 'waterfall-async': result = await probe.waterfallAsync(); break
  case 'internal': probe.announce(); result = probe.internal(); break
  case 'host': result = probe.host(); break
  default: throw new Error(`unknown scenario ${scenario}`)
}
process.stdout.write(JSON.stringify({ log: log.entries, result }))
await fiber.dispose()
