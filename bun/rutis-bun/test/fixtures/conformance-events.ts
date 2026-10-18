// The events conformance plugin `conformance-events` for the Bun runtime
// (language `bun`; #464 step 7). The cases are
// crates/rutis-bridge/src/runtime/testing/events.expected.json; what a row
// does with its config is the contract in that file's `about` and in
// crates/rutis-bridge/src/runtime/testing/events.rs. The real runtimes run
// it (crates/rutis-loader/tests/events_conformance.rs). This file is copied
// out of the repository to run, so it imports nothing; it is the Node leaf
// form (node/rutis-runtime/test/fixtures/conformance-events-leaf.mjs) with
// types.
//
// Each row is one participant of a case, keyed by its label. A listener row
// (`name` in its config) registers one listener; an emitter row (`emitter`)
// provides `emitter-<label>`, whose `dispatch(spec)` emits with the native
// API. Everything is recorded through the host service `events-log`.

type Args = any[]
type ErrorSpec = { name: string, message: string }
type Does = Record<string, any>
type Filter = { keep: string[] } | { throws: ErrorSpec }
type Spec = { mode: string, name: string, args?: Args, filter?: Filter, terminal?: string, cancel?: { when: string } }
type Config = {
  label: string, name?: string, kind?: 'sync' | 'async' | 'any', options?: { prepend?: boolean, global?: boolean },
  log?: 'label' | 'arg' | string[] | false, does?: Does, emitter?: boolean, provides?: string, fails?: ErrorSpec,
}
// What the plugin uses of the host's `events-log`.
type Log = { add(entry: string): void, filter(description: unknown): void }
// The five ways to emit, on `ctx` and on what `ctx.filter` returns.
interface Emitter {
  emit(name: string, ...args: Args): void
  parallel(name: string, ...args: Args): Promise<void>
  serial(name: string, ...args: Args): Promise<unknown>
  waterfall(name: string, ...args: Args): unknown
  bail(name: string, ...args: Args): unknown
}
interface Ctx extends Emitter {
  use(name: string): any
  provide(name: string, value: unknown): unknown
  on(name: string, listener: (...args: Args) => unknown, options?: { prepend?: boolean, global?: boolean }): () => void
  filter(predicate: (description: { row: string | null }) => boolean): Emitter
  readonly signal: AbortSignal
}
type Row = { ctx: Ctx, log: Log, label: string, sync: boolean }

// How a value is written in the log: strings as they are, null and
// undefined as `null`, the rest as JSON (`1`, `true`).
const fmt = (value: unknown) => typeof value === 'string' ? value : value == null ? 'null' : JSON.stringify(value)

// The line a listener logs when it is called, or nothing (`log: false`).
function line({ label, log = 'label' }: Config, args: Args): string | undefined {
  if (log === false) return undefined
  if (log === 'label') return label
  if (log === 'arg') return `${label}:${fmt(args[0])}`
  return `${label}:${log.map(field => fmt(args[0]?.[field])).join(',')}`
}

const named = ({ name, message }: ErrorSpec) => Object.assign(new Error(message), { name })

// Block the thread (a `sync` listener waits without returning).
const sleep = (ms: number) => { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms) }
const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms))

// `f(value)`, after `value` settles when it is a promise.
const then = (value: any, f: (value: any) => unknown) => typeof value?.then === 'function' ? value.then(f) : f(value)

// Until `signal` aborts.
const aborted = (signal: AbortSignal) => new Promise<void>(resolve => {
  if (signal.aborted) resolve()
  else signal.addEventListener('abort', () => resolve(), { once: true })
})

// What the listener does (`does` in the contract). `sync`: everything
// blocks; otherwise waiting returns a promise. `next` is the last argument.
function perform(does: Does, args: Args, row: Row): unknown {
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
function listener(ctx: Ctx, log: Log, config: Config) {
  const call = (args: Args, sync: boolean) => {
    const entry = line(config, args)
    if (entry !== undefined) log.add(entry)
    return perform(config.does ?? {}, args, { ctx, log, label: config.label, sync })
  }
  switch (config.kind ?? 'async') {
    case 'sync': return (...args: Args) => call(args, true)
    case 'async': return async (...args: Args) => call(args, false)
    case 'any': return (...args: Args) => call(args, false)
    default: throw new Error(`unknown kind ${config.kind}`)
  }
}

// The filter of a spec: `keep` logs each description (through
// `events-log.filter`) and keeps the listed rows; `throws` throws.
function predicate(filter: Filter, log: Log) {
  if ('throws' in filter) return () => { throw named(filter.throws) }
  return (description: { row: string | null }) => {
    log.filter(description)
    return filter.keep.includes(description.row as string)
  }
}

// Emit `spec` from `ctx` with the native API and return the result.
// JavaScript cannot give up a dispatch, so `cancel` fails at once.
function dispatch(ctx: Ctx, log: Log, spec: Spec): unknown {
  if (spec.cancel) throw new Error('JavaScript cannot cancel a dispatch')
  const from: Emitter = spec.filter ? ctx.filter(predicate(spec.filter, log)) : ctx
  const { name, args = [] } = spec
  switch (spec.mode) {
    case 'emit': from.emit(name, ...args); return null
    case 'parallel': return from.parallel(name, ...args).then(() => null)
    case 'serial': return from.serial(name, ...args)
    case 'waterfall': {
      const terminal = typeof spec.terminal === 'string'
        ? () => { log.add('terminal'); return spec.terminal }
        : () => undefined
      return from.waterfall(name, ...args, terminal)
    }
    case 'bail': return from.bail(name, ...args)
    default: throw new Error(`unknown mode ${spec.mode}`)
  }
}

export const name = 'conformance-events'
export const inject = ['events-log']
// A declared service is exported whether or not the row provides it, so
// this module (for listener rows) declares nothing; emitter rows load
// conformance-events-e.ts / -f.ts, which declare only their own emitter.
export const provides = {}
// What an emitter row `<label>` provides: `{ "emitter-<label>": EMITTER }`.
export const EMITTER = { dispatch: 'async', dispatch_sync: 'sync' } as const

export function apply(ctx: Ctx, config: Config) {
  if (config.fails) throw named(config.fails)
  const log: Log = ctx.use('events-log')
  if (config.provides) ctx.provide(config.provides, { ping() { return null } })
  if (config.emitter) {
    ctx.provide(`emitter-${config.label}`, {
      async dispatch(spec: Spec) { return await dispatch(ctx, log, spec) },
      dispatch_sync(spec: Spec) { return dispatch(ctx, log, spec) },
    })
  }
  if (config.name) ctx.on(config.name, listener(ctx, log, config), config.options ?? {})
}
