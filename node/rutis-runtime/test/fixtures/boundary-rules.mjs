// A Cordis plugin for the boundary rule tests (requirements-protocol-plugins
// §5, rules 3, 4, 5 and 7): `crates/rutis-bridge/tests/boundary_rules.rs`
// mounts it in the Node runtime, and `boundary-native.mjs` runs it in native
// Cordis as the reference. Every listener writes to the `log` service, which
// rutis provides across the boundary and the reference provides natively, so
// both sides' listeners land in one sequence.

// The class the plugin declares for the `log` service (rule 7).
export class Log {
  constructor() { this.entries = [] }
  push(tag) { this.entries.push(tag); return this.entries.length }
}

export const inject = ['log']

export function apply(ctx) {
  const mark = tag => { ctx.log.push(tag) }

  // Rule 3, Cordis -> rutis: A and B while starting, C later.
  ctx.on('boundary/order', () => { mark('A') })
  ctx.on('boundary/order', () => { mark('B') })
  // Rule 3, rutis -> Cordis: a while starting, b later.
  ctx.on('boundary/back', () => { mark('a') })

  // Rule 4, Cordis -> rutis: A while starting; C (bails) and D later.
  ctx.on('boundary/chain', () => { mark('A') })
  // Rule 4, rutis -> Cordis: the Cordis group of a rutis serial.
  ctx.on('boundary/link', async () => { mark('a:start'); await Promise.resolve(); mark('a:end') })
  ctx.on('boundary/link', async () => { mark('b:start'); await Promise.resolve(); mark('b:end') })
  // Rule 4: a waterfall whose middlewares call next once.
  ctx.on('boundary/wf', (value, next) => { mark('A'); return next() })

  // Rule 5: Cordis's own internal/* events, heard natively on this side.
  const internal = []
  ctx.on('internal/service', name => { internal.push(String(name)) })

  ctx.provide('probe', {
    late() {
      ctx.on('boundary/order', () => { mark('C') })
      ctx.on('boundary/back', () => { mark('b') })
      ctx.on('boundary/chain', () => { mark('C'); return 'C' })
      ctx.on('boundary/chain', () => { mark('D') })
      ctx.on('boundary/wf', (value, next) => { mark('C'); return next() })
    },
    async order() { await ctx.serial('boundary/order') },
    async serial() { return await ctx.serial('boundary/chain') ?? null },
    bail() {
      const result = ctx.bail('boundary/chain')
      return result instanceof Promise ? { promise: true } : { value: result ?? null }
    },
    waterfall() {
      const result = ctx.waterfall('boundary/wf', 1, () => 'end')
      return result instanceof Promise ? { promise: true } : { value: result ?? null }
    },
    // Rule 5: provide a service here, so Cordis emits internal/service.
    announce() { ctx.provide('boundary-extra', { ok: true }) },
    internal() { return internal },
    // Rule 7: the host service seen from Cordis.
    host() {
      return {
        instance: ctx.log instanceof Log,
        push: typeof ctx.log.push,
        pushed: ctx.log.push('through the interface'),
      }
    },
  })
}
