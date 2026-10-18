// The events conformance plugin `conformance-events` as a Cordis plugin
// (language `node`; #464 step 7). The cases are
// crates/rutis-bridge/src/runtime/testing/events.expected.json; what a row
// does with its config is the contract in that file's `about` and in
// crates/rutis-bridge/src/runtime/testing/events.rs. The real runtimes run
// it (crates/rutis-loader/tests/events_conformance.rs); the leaf form is
// conformance-events-leaf.mjs.
//
// Everything is Cordis's own API: `ctx.on(name, fn, { prepend, global })`,
// the five dispatch methods with a `thisArg` whose `[Context.filter]` sees
// each listener's Context (`c.rutis` is its descriptor), `ctx.signal`, and
// `internal/dispatch`.
//
// What it provides: a Cordis plugin declares its services' method kinds
// with `rutis.provides` in the package.json nearest its entry file, and a
// declared service is exported whether or not the row provides it. So this
// package declares nothing and serves listener rows; emitter rows load
// ../conformance-events-e and ../conformance-events-f, which re-export
// this file under a package.json declaring only `emitter-e` / `emitter-f`.
import { Context } from '@deepseek-ai/cordis'

export const name = 'conformance-events'
export const inject = ['events-log']

// How a value is written in the log: strings as they are, null and
// undefined as `null`, the rest as JSON (`1`, `true`).
const fmt = value => typeof value === 'string' ? value : value == null ? 'null' : JSON.stringify(value)

// The line a listener logs when it is called, or nothing (`log: false`).
function line({ label, log = 'label' }, args) {
  if (log === false) return undefined
  if (log === 'label') return label
  if (log === 'arg') return `${label}:${fmt(args[0])}`
  return `${label}:${log.map(field => fmt(args[0]?.[field])).join(',')}`
}

const named = ({ name, message }) => Object.assign(new Error(message), { name })

// Block the thread (a `sync` listener waits without returning).
const sleep = ms => { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms) }
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))

// `f(value)`, after `value` settles when it is a promise.
const then = (value, f) => typeof value?.then === 'function' ? value.then(f) : f(value)

// Until `signal` aborts.
const aborted = signal => new Promise(resolve => {
  if (signal.aborted) resolve()
  else signal.addEventListener('abort', () => resolve(), { once: true })
})

// What the listener does (`does` in the contract). `sync`: everything
// blocks; otherwise waiting returns a promise. `next` is the last argument.
function perform(does, args, row) {
  const next = args.at(-1)
  if ('returns' in does) return does.returns
  if (does.returnsNothing) return undefined
  if (does.echo) return args[0]
  if (does.throws) throw named(does.throws)
  if ('after' in does) {
    if (row.sync) {
      sleep(does.after)
      return perform(does.then, args, row)
    }
    return delay(does.after).then(() => perform(does.then, args, row))
  }
  if ('next' in does) return then(next(), r => does.next + (r == null || r === false ? '' : r))
  if ('veto' in does) return does.veto
  // The second call fails with WaterfallNext, which propagates.
  if (does.nextTwice) return then(next(), () => next())
  if (does.waits) {
    return aborted(row.ctx.signal).then(() => {
      row.log.add(`${row.label}:cancelled`)
      return null
    })
  }
  if ('calls' in does) return args[does.calls]('x')
  if (does.emits) return dispatch(row.ctx, row.log, does.emits)
  throw new Error(`unknown does: ${JSON.stringify(does)}`)
}

// The listener of `config`, of its kind: `sync` a plain function doing
// everything at once, `async` an async function, `any` a plain function
// returning a promise only when it has to wait.
function listener(ctx, log, config) {
  const call = (args, sync) => {
    const entry = line(config, args)
    if (entry !== undefined) log.add(entry)
    return perform(config.does, args, { ctx, log, label: config.label, sync })
  }
  switch (config.kind ?? 'async') {
    case 'sync': return (...args) => call(args, true)
    case 'async': return async (...args) => call(args, false)
    case 'any': return (...args) => call(args, false)
    default: throw new Error(`unknown kind ${config.kind}`)
  }
}

// The `thisArg` of a dispatch: none without a filter; with one, Cordis's
// `Context.filter`, called with each listener's Context. `keep` logs each
// descriptor (through `events-log.filter`) and keeps the listed rows;
// `throws` throws.
function thisArgOf(filter, log) {
  if (!filter) return undefined
  if (filter.throws) return { [Context.filter]: () => { throw named(filter.throws) } }
  return {
    [Context.filter]: c => {
      log.filter(c.rutis)
      return filter.keep.includes(c.rutis.row)
    },
  }
}

// Emit `spec` from `ctx` with Cordis's methods and return the result.
// JavaScript cannot give up a dispatch, so `cancel` fails at once. A filter
// is always passed as `thisArg` (`via: "thisArg"` says so; there is no
// other way in Cordis).
function dispatch(ctx, log, spec) {
  if (spec.cancel) throw new Error('JavaScript cannot cancel a dispatch')
  const thisArg = thisArgOf(spec.filter, log)
  const head = thisArg ? [thisArg, spec.name] : [spec.name]
  const args = spec.args ?? []
  switch (spec.mode) {
    case 'emit': ctx.emit(...head, ...args); return null
    case 'parallel': return ctx.parallel(...head, ...args).then(() => null)
    case 'serial': return ctx.serial(...head, ...args)
    case 'waterfall': {
      // A text it returns, or `{ echo: true }`: its first argument.
      const terminal = spec.terminal?.echo ? first => { log.add('terminal'); return first }
        : typeof spec.terminal === 'string'
        ? () => { log.add('terminal'); return spec.terminal }
        : () => undefined
      return ctx.waterfall(...head, ...args, terminal)
    }
    case 'bail': return ctx.bail(...head, ...args)
    default: throw new Error(`unknown mode ${spec.mode}`)
  }
}

export function apply(ctx, config) {
  if (config.fails) throw named(config.fails)
  const log = ctx.get('events-log')
  if (config.provides) ctx.provide(config.provides, { ping() { return null } })
  if (config.emitter) {
    ctx.provide(`emitter-${config.label}`, {
      async dispatch(spec) { return await dispatch(ctx, log, spec) },
      dispatch_sync(spec) { return dispatch(ctx, log, spec) },
    })
  }
  if (!config.name) return
  // Cordis's diagnostics hook, fired once per dispatch before any listener.
  if (config.observesDispatch) {
    ctx.on('internal/dispatch', (mode, name) => {
      if (name === config.name) log.add(`${config.label}:dispatch:${mode}`)
    })
  }
  const { prepend = false, global = false } = config.options ?? {}
  ctx.on(config.name, listener(ctx, log, config), { prepend, global })
}
