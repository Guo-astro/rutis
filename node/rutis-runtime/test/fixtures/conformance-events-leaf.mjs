// The events conformance plugin `conformance-events` as a leaf plugin
// (language `node-leaf`; #464 step 7). The cases are
// crates/rutis-bridge/src/runtime/testing/events.expected.json; what a row
// does with its config is the contract in that file's `about` and in
// crates/rutis-bridge/src/runtime/testing/events.rs. The real runtimes
// (crates/rutis-loader/tests/events_conformance.rs) and the testing tool
// (node/rutis/test/events-conformance.test.mjs) both run it. It imports the
// SDK by path, so it is loaded where it is, not copied.
//
// Each row is one participant of a case, keyed by its label. A listener row
// (`name` in its config) registers one listener; an emitter row (`emitter`)
// provides `emitter-<label>`, whose `dispatch(spec)` emits with the native
// API. Everything is recorded through the host service `events-log`.
import { definePlugin } from '../../../rutis/src/index.mjs'

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

// The filter of a spec: `keep` logs each description (through
// `events-log.filter`) and keeps the listed rows; `throws` throws.
function predicate(filter, log) {
  if (filter.throws) return () => { throw named(filter.throws) }
  return description => {
    log.filter(description)
    return filter.keep.includes(description.row)
  }
}

// Emit `spec` from `ctx` with the native API and return the result.
// JavaScript cannot give up a dispatch, so `cancel` fails at once.
function dispatch(ctx, log, spec) {
  if (spec.cancel) throw new Error('JavaScript cannot cancel a dispatch')
  const from = spec.filter ? ctx.filter(predicate(spec.filter, log)) : ctx
  const { name, args = [] } = spec
  switch (spec.mode) {
    case 'emit': from.emit(name, ...args); return null
    case 'parallel': return from.parallel(name, ...args).then(() => null)
    case 'serial': return from.serial(name, ...args)
    case 'waterfall': {
      // A text it returns, or `{ echo: true }`: its first argument.
      if (spec.terminal?.echo) return from.waterfall.with(first => { log.add('terminal'); return first })(name, ...args)
      if (typeof spec.terminal !== 'string') return from.waterfall(name, ...args)
      return from.waterfall.with(() => { log.add('terminal'); return spec.terminal })(name, ...args)
    }
    case 'bail': return from.bail(name, ...args)
    default: throw new Error(`unknown mode ${spec.mode}`)
  }
}

// The plugin declaring `provides`. A declared service is exported whether
// or not the row provides it, so listener rows load the plugin declaring
// nothing (this module's default export) and emitter rows the one declaring
// only their own emitter (conformance-events-leaf-e.mjs / -f.mjs).
export const conformanceEvents = (provides = {}) => definePlugin({
  name: 'conformance-events',
  inject: ['events-log'],
  provides,
  apply(ctx, config) {
    if (config.fails) throw named(config.fails)
    const log = ctx.use('events-log')
    if (config.provides) ctx.provide(config.provides, { ping() { return null } })
    if (config.emitter) {
      ctx.provide(`emitter-${config.label}`, {
        async dispatch(spec) { return await dispatch(ctx, log, spec) },
        dispatch_sync(spec) { return dispatch(ctx, log, spec) },
      })
    }
    if (config.name) ctx.on(config.name, listener(ctx, log, config), config.options ?? {})
  },
})

// What an emitter row `<label>` provides: `{ "emitter-<label>": EMITTER }`.
export const EMITTER = { dispatch: 'async', dispatch_sync: 'sync' }

export default conformanceEvents()
