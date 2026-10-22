// The events conformance plugin `conformance-events` (#464), for the Node
// runtime: docs/design-events-protocol-2026-10-18.en.md §13.1. What it
// does is in crates/rutis-bridge/src/runtime/testing/events.expected.json;
// the real runtimes and the testing tool (@arcships/rutis/testing,
// node/rutis/test/events-conformance.test.mjs) are both checked against it.
//
// Each participant is one row; its config is the participant. A listener
// row registers one listener as `does` says; an emitter row
// (`{ emitter: true, label }`) provides `emitter-<label>` with
// `dispatch(spec)` (and `dispatch_sync(spec)` for `bail`), which emits with
// the plugin API and returns the result or throws. Every listener start,
// filter call and cancellation is logged with the host's synchronous
// service `events-log.add(entry)`. It imports the SDK by path, so it is
// loaded where it is, not copied.
import * as rutis from '../../../rutis/src/index.mjs'
import { definePlugin } from '../../../rutis/src/index.mjs'

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

// A value as the expectations write it (`{"$<tag>": …}`, `{"$callback":
// "upper"}`), made into what JavaScript sends.
const makers = {
  $instant: text => new Date(text),
  $bytes: text => Uint8Array.from(atob(text), c => c.charCodeAt(0)),
  $map: entries => new Map(entries.map(([key, value]) => [made(key), made(value)])),
  $set: elements => new Set(elements.map(made)),
  $bigint: BigInt,
  $datetime: rutis.datetime, $date: rutis.date, $time: rutis.time, $decimal: rutis.decimal, $uuid: rutis.uuid,
  $callback: name => {
    if (name !== 'upper') throw new Error(`unknown callback ${name}`)
    return text => String(text).toUpperCase()
  },
}
export function made(value) {
  if (Array.isArray(value)) return value.map(made)
  if (value === null || typeof value !== 'object') return value
  const keys = Object.keys(value)
  if (keys.length === 1 && Object.hasOwn(makers, keys[0])) return makers[keys[0]](value[keys[0]])
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, made(item)]))
}

export default definePlugin({
  name: 'conformance-events',
  inject: ['events-log'],
  apply(ctx, participant) {
    const log = entry => { ctx.use('events-log').add(entry) }
    if (participant?.emitter) {
      const emitting = spec => emit(ctx, log, spec)
      ctx.provide(`emitter-${participant.label ?? 'e'}`, { dispatch: async spec => emitting(spec), dispatch_sync: emitting })
      return
    }
    const { label, name, kind = 'async', options, does } = participant
    const how = participant.log ?? 'label'
    const record = args => {
      if (how === false) return
      if (how === 'label') log(label)
      else if (how === 'arg') log(`${label}:${args[0]}`)
      else if (Array.isArray(how)) log(`${label}:${how.map(field => args[0]?.[field]).join(',')}`)
    }
    const waterfall = does && ('next' in does || 'veto' in does || 'nextTwice' in does)
    const body = args => {
      record(args)
      const next = waterfall || typeof args.at(-1) === 'function' ? args.at(-1) : undefined
      return perform(ctx, log, label, does, args, next, kind)
    }
    const listener = kind === 'async' ? async (...args) => body(args) : (...args) => body(args)
    ctx.on(name, listener, options)
  },
})

// What a listener does (§13.2 `does`). Synchronous unless it must wait.
function perform(ctx, log, label, does, args, next, kind) {
  if ('returns' in does) return made(does.returns)
  if (does.returnsNothing) return undefined
  if (does.echo) return args[0]
  if (does.throws) throw Object.assign(new Error(does.throws.message), { name: does.throws.name })
  if ('after' in does) return sleep(does.after).then(() => perform(ctx, log, label, does.then, args, next, kind))
  if ('next' in does) {
    const rest = next()
    const join = r => `${does.next}${r === undefined || r === null || r === false ? '' : r}`
    return rest !== null && typeof rest?.then === 'function' ? rest.then(join) : join(rest)
  }
  if ('veto' in does) return made(does.veto)
  if (does.nextTwice) {
    const first = next()
    return first !== null && typeof first?.then === 'function' ? first.then(() => next()) : next()
  }
  if (does.waits) {
    // A `sync` listener blocks its thread until its process ends: nothing
    // can wake it.
    if (kind === 'sync') Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0)
    const signal = ctx.signal
    return new Promise(resolve => {
      if (signal.aborted) resolve()
      else signal.addEventListener('abort', () => resolve(), { once: true })
    }).then(() => { log(`${label}:cancelled`); return null })
  }
  if ('calls' in does) return args[does.calls]('x')
  if (does.emits) return emit(ctx, log, does.emits)
  throw new Error(`unknown listener behaviour ${JSON.stringify(does)}`)
}

// Emit `spec` from this row with the plugin API; the result, or a Promise of it.
function emit(ctx, log, spec) {
  const args = made(spec.args ?? [])
  let emitter = ctx
  if (spec.filter?.throws) {
    const { name, message } = spec.filter.throws
    emitter = ctx.filter(() => { throw Object.assign(new Error(message), { name }) })
  } else if (spec.filter) {
    const keep = new Set(spec.filter.keep)
    emitter = ctx.filter(descriptor => {
      log(`filter:${descriptor.row}`)
      return keep.has(descriptor.row)
    })
  }
  switch (spec.mode) {
    case 'emit': emitter.emit(spec.name, ...args); return null
    case 'parallel': return emitter.parallel(spec.name, ...args).then(() => null)
    case 'serial': return emitter.serial(spec.name, ...args)
    case 'waterfall': {
      // A text it returns, or `{ echo: true }`: its first argument.
      const terminal = typeof spec.terminal === 'string' ? () => { log('terminal'); return spec.terminal }
        : spec.terminal?.echo ? first => { log('terminal'); return first } : undefined
      return (terminal ? emitter.waterfall.with(terminal) : emitter.waterfall)(spec.name, ...args)
    }
    case 'bail': return emitter.bail(spec.name, ...args)
    default: throw new Error(`a JavaScript plugin cannot emit in mode ${spec.mode}`)
  }
}
