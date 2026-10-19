// The events conformance plugin `conformance-events` (#464), for the Bun
// runtime: docs/design-events-protocol-2026-10-18.en.md §13.1. What it
// does is in crates/rutis-bridge/src/runtime/testing/events.expected.json
// (test/events-conformance.test.ts runs the runtime against it in one
// process). The same plugin as node/rutis-runtime/test/fixtures/
// conformance-events.mjs, as module exports: this file is copied out of the
// repository to run, so it marks the SDK constructors' values itself.
//
// Each participant is one row; its config is the participant. A listener
// row registers one listener as `does` says; an emitter row
// (`{ emitter: true, label }`) provides `emitter-<label>` with
// `dispatch(spec)` (and `dispatch_sync(spec)` for `bail`). Every listener
// start, filter call and cancellation is logged with the host's
// synchronous service `events-log.add(entry)`.

const value = (tag: string, text: string) => Object.freeze({ [Symbol.for('rutis.value')]: tag, value: text })
const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms))

const makers: Record<string, (value: any) => unknown> = {
  $instant: (text: string) => new Date(text),
  $bytes: (text: string) => Uint8Array.from(atob(text), c => c.charCodeAt(0)),
  $map: (entries: [unknown, unknown][]) => new Map(entries.map(([key, item]) => [made(key), made(item)])),
  $set: (elements: unknown[]) => new Set(elements.map(made)),
  $bigint: (text: string) => BigInt(text),
  $datetime: (text: string) => value('datetime', text),
  $date: (text: string) => value('date', text),
  $time: (text: string) => value('time', text),
  $decimal: (text: string) => value('decimal', text),
  $uuid: (text: string) => value('uuid', text.toLowerCase()),
  $callback: (name: string) => {
    if (name !== 'upper') throw new Error(`unknown callback ${name}`)
    return (text: unknown) => String(text).toUpperCase()
  },
}
function made(item: any): any {
  if (Array.isArray(item)) return item.map(made)
  if (item === null || typeof item !== 'object') return item
  // A value that arrived as itself (a Date, a Map, binary data, ...) stays.
  const prototype = Object.getPrototypeOf(item)
  if (prototype !== Object.prototype && prototype !== null) return item
  const keys = Object.keys(item)
  if (keys.length === 1 && Object.hasOwn(makers, keys[0])) return makers[keys[0]](item[keys[0]])
  return Object.fromEntries(Object.entries(item).map(([key, inner]) => [key, made(inner)]))
}

type Log = (entry: string) => void

export const name = 'conformance-events'
export const inject = ['events-log']
// How a value is written in the log: strings as they are, null and
// undefined as `null`, the rest as JSON (`1`, `true`).
const fmt = (value: unknown) => typeof value === 'string' ? value : value == null ? 'null' : JSON.stringify(value)

export function apply(ctx: any, participant: any) {
  // The row's apply fails, having registered nothing (`fails`).
  if (participant?.fails) throw Object.assign(new Error(participant.fails.message), { name: participant.fails.name })
  const log: Log = entry => { ctx.use('events-log').add(entry) }
  // An empty service of this name besides (`provides`).
  if (participant?.provides) ctx.provide(participant.provides, { ping: () => null })
  if (participant?.emitter) {
    const emitting = (spec: any) => emit(ctx, log, spec)
    ctx.provide(`emitter-${participant.label ?? 'e'}`, { dispatch: async (spec: any) => emitting(spec), dispatch_sync: emitting })
    return
  }
  const { label, name, kind = 'async', options, does } = participant
  const how = participant.log ?? 'label'
  const record = (args: any[]) => {
    if (how === false) return
    if (how === 'label') log(label)
    else if (how === 'arg') log(`${label}:${fmt(args[0])}`)
    else if (Array.isArray(how)) log(`${label}:${how.map((field: string) => fmt(args[0]?.[field])).join(',')}`)
  }
  const waterfall = does && ('next' in does || 'veto' in does || 'nextTwice' in does)
  const body = (args: any[]) => {
    record(args)
    const next = waterfall || typeof args.at(-1) === 'function' ? args.at(-1) : undefined
    return perform(ctx, log, label, does, args, next, kind === 'sync')
  }
  const listener = kind === 'async' ? async (...args: any[]) => body(args) : (...args: any[]) => body(args)
  ctx.on(name, listener, options)
}

const isThenable = (item: any) => item !== null && typeof item?.then === 'function'

function perform(ctx: any, log: Log, label: string, does: any, args: any[], next: any, sync = false): any {
  if ('returns' in does) return made(does.returns)
  if (does.returnsNothing) return undefined
  if (does.echo) return args[0]
  if (does.throws) throw Object.assign(new Error(does.throws.message), { name: does.throws.name })
  // A synchronous listener (`kind: sync`) blocks the thread instead.
  if ('after' in does && sync) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, does.after)
    return perform(ctx, log, label, does.then, args, next, sync)
  }
  if ('after' in does) return sleep(does.after).then(() => perform(ctx, log, label, does.then, args, next))
  if ('next' in does) {
    const rest = next()
    const join = (r: unknown) => `${does.next}${r === undefined || r === null || r === false ? '' : r}`
    return isThenable(rest) ? rest.then(join) : join(rest)
  }
  if ('veto' in does) return made(does.veto)
  if (does.nextTwice) {
    const first = next()
    return isThenable(first) ? first.then(() => next()) : next()
  }
  if (does.waits) {
    const signal: AbortSignal = ctx.signal
    return new Promise<void>(resolve => {
      if (signal.aborted) resolve()
      else signal.addEventListener('abort', () => resolve(), { once: true })
    }).then(() => { log(`${label}:cancelled`); return null })
  }
  if ('calls' in does) return args[does.calls]('x')
  if (does.emits) return emit(ctx, log, does.emits)
  throw new Error(`unknown listener behaviour ${JSON.stringify(does)}`)
}

function emit(ctx: any, log: Log, spec: any): any {
  const args = made(spec.args ?? [])
  let emitter = ctx
  if (spec.filter?.throws) {
    const { name, message } = spec.filter.throws
    emitter = ctx.filter(() => { throw Object.assign(new Error(message), { name }) })
  } else if (spec.filter) {
    const keep = new Set(spec.filter.keep)
    emitter = ctx.filter((descriptor: any) => {
      // Logs `filter:<row>` and keeps the description (`filterSees`).
      ctx.use('events-log').filter(descriptor)
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
        : spec.terminal?.echo ? (first: unknown) => { log('terminal'); return first } : undefined
      return (terminal ? emitter.waterfall.with(terminal) : emitter.waterfall)(spec.name, ...args)
    }
    case 'bail': return emitter.bail(spec.name, ...args)
    default: throw new Error(`a JavaScript plugin cannot emit in mode ${spec.mode}`)
  }
}
