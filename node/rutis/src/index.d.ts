export type MethodKind = 'sync' | 'async'

/** Where a listener is, as the logical bus describes it (#464): what a filter sees. */
export interface ListenerDescriptor {
  /** The event name (a prefix, for a Rust pattern listener). */
  readonly name: string
  /** A Rust pattern listener's prefix entry. */
  readonly prefix: boolean
  /** The plugin that registered it. */
  readonly plugin: string
  /** Its row's id (the row id in the configuration, not the session's row key); `null` for a plugin a mount loaded. */
  readonly row: string | null
  /** The runtime it runs in; `null` for the host's own Rust plugins. */
  readonly runtime: string | null
  /** The node it came from over a node link; `null` otherwise. */
  readonly node: string | null
  /** Its scope chain, outermost first; `[]` at the root. */
  readonly scope: ReadonlyArray<{ readonly id: string, readonly group: string }>
  /** A `global` listener is never filtered (the scope rule still applies). */
  readonly global: boolean
}

export interface ListenerOptions {
  /** Run before every listener of the name registered so far. */
  prepend?: boolean
  /** Not subject to emitters' filters (still to the scope rule). */
  global?: boolean
}

/** A listener: the event's arguments, then, in a `waterfall`, `next` (the rest of the chain) last. */
export type Listener = (...args: any[]) => any

/** The five ways to emit. Each reaches every listener of the name in the emitter's scope or above it, wherever the listener runs, by the same rules as if all were in this process. Emitting `rutis/*` throws `ReservedEvent`; `internal/*` is refused. */
export interface Emitter {
  /** Start every listener in registration order; returns at once. A listener's failure is reported where it ran and reaches neither the emitter nor the other listeners. */
  emit(name: string, ...args: any[]): void
  /** Start every listener and wait for all. Failures reject with an `AggregateError` in registration order, even a single one. */
  parallel(name: string, ...args: any[]): Promise<void>
  /** Run the listeners one by one; resolves to the first result (not `undefined`, `null` or `false`), or `undefined`. An error stops and rejects. */
  serial<T = any>(name: string, ...args: any[]): Promise<T | undefined>
  /** Each listener gets `next` (no arguments) last and returns the result; not calling `next` vetoes the rest. The last argument is the terminal, called with the event's arguments at the end of the chain. Returns the outermost listener's value as is (a Promise whenever a listener runs elsewhere: await it). Calling `next` twice, or after the end, throws `WaterfallNext`. */
  waterfall<T = any>(name: string, ...argsThenTerminal: any[]): T | Promise<T>
  /** Synchronously, the first result as `serial`. An `async` listener, or one returning a Promise, fails it with `AsyncListener`. */
  bail<T = any>(name: string, ...args: any[]): T | undefined
}

export interface Context extends Emitter {
  /** The service `name`, which the plugin declares in `inject`: the object itself when a plugin in the same process provides it, else a proxy whose methods are sync or async as declared. */
  use<T = any>(name: string): T
  /** Provide `value` as the service `name` until the plugin unloads, or until the returned function is called. It reaches rutis by itself: its methods are the object's functions (own and inherited, not `constructor`), `async` unless `provides` marks them `sync`. */
  provide(name: string, value: unknown): () => unknown
  /** Run `cleanup` when the plugin unloads. */
  effect(cleanup: () => unknown): void
  /** Aborted when the plugin unloads; its running listeners are then waited for (up to the host's shutdown deadline) before the cleanups run. */
  readonly signal: AbortSignal
  /** Listen to `name` on the host's logical bus: events emitted in the plugin's scope or below it, from any process or language. Registered with the host by itself; unregistered when the plugin unloads or the returned function is called. `rutis/*` (the host's framework events) can be listened to; `internal/*` cannot. */
  on(name: string, listener: Listener, options?: ListenerOptions): () => void
  /** The five ways to emit, delivered only to the listeners `keep` returns true for (`global` listeners are always delivered). `keep` runs in this process, once per candidate, before any listener runs. */
  filter(keep: (listener: ListenerDescriptor) => unknown): Emitter
}

export interface PluginSpec<C = any> {
  name?: string
  /** Services the plugin uses: rutis starts it once they are all available, and stops it when one goes. */
  inject?: string[]
  /** Optional: services it provides, with the kind of methods to call `sync` (the others are `async`). Without it, what `ctx.provide` provides still reaches rutis; with it, `rutis-host check` knows before the plugin runs. */
  provides?: Record<string, Record<string, MethodKind>>
  /** The JSON Schema of its config. */
  config?: object
  apply(ctx: Context, config: C): void | (() => unknown) | Promise<void | (() => unknown)>
}

export interface Plugin<C = any> extends Readonly<Required<Pick<PluginSpec<C>, 'inject' | 'provides'>>>, Readonly<Omit<PluginSpec<C>, 'inject' | 'provides'>> {
  /** The plugin API it was written against. */
  readonly api: number
  /** Always `true`: what it provides beyond `provides` reaches rutis by itself, known once it runs. */
  readonly announces: boolean
}

/** The plugin API this SDK writes plugins against. */
export declare const PLUGIN_API: number
export declare const PLUGIN: unique symbol
export declare function definePlugin<C = any>(spec: PluginSpec<C>): Plugin<C>
export declare function isPlugin(value: unknown): value is Plugin

/** The mark on a value made by `datetime`, `date`, `time`, `decimal` or `uuid` (`Symbol.for('rutis.value')`). */
export declare const VALUE: unique symbol

/**
 * A local date and time, date, time of day, decimal or UUID to send as that
 * value (#308): JavaScript has no type for them. Received, they are strings
 * (`value`'s text); a plain string is sent as a string.
 */
export interface TaggedValue<Tag extends 'datetime' | 'date' | 'time' | 'decimal' | 'uuid'> {
  readonly [VALUE]: Tag
  /** The value's text, as it crosses. */
  readonly value: string
  toString(): string
  toJSON(): string
}

/** A local date and time without a time zone: `'2026-10-15T14:30'`, `'2026-10-15T14:30:05.25'` (years 0001–9999, 0–9 fraction digits). Received as its text. */
export declare function datetime(text: string): TaggedValue<'datetime'>
/** A calendar date: `'2026-10-15'` (years 0001–9999). Received as its text. */
export declare function date(text: string): TaggedValue<'date'>
/** A time of day without a time zone: `'14:30'`, `'14:30:05.25'` (24-hour). Received as its text. */
export declare function time(text: string): TaggedValue<'time'>
/** A decimal number, trailing zeros kept: `'1.10'`, `'-12.3400'` (no exponent, at most 4300 digits). Received as its text. */
export declare function decimal(text: string | bigint): TaggedValue<'decimal'>
/** A UUID, `8-4-4-4-12` hex in either case; sent and received in lowercase. */
export declare function uuid(text: string): TaggedValue<'uuid'>
