// What a plugin author rarely writes (#692): the plugin API version, the
// marks, the value constructors and the types of the events. `definePlugin`
// and `Context` are in `@arcships/rutis`.
import type { Plugin } from './index.js'

export type MethodKind = 'sync' | 'async'

/** Where a listener is, as the logical bus describes it (#464): what a filter sees. */
export interface ListenerDescriptor {
  /** The event name (a prefix, for a Rust pattern listener). */
  readonly name: string
  /** A Rust pattern listener's prefix entry. */
  readonly prefix: boolean
  /** The plugin that registered it: its declared name (`PluginSpec.name`, a Cordis plugin's name), else its row's id. */
  readonly plugin: string
  /** Its row's id, the name the configuration gives the row (the copies a group's instances make share it); `null` for a plugin a mount loaded. */
  readonly row: string | null
  /** The runtime it runs in; `null` for the host's own Rust plugins. */
  readonly runtime: string | null
  /** The node it came from over a node link; `null` otherwise. */
  readonly node: string | null
  /** Its scope chain, outermost first; `[]` at the root. */
  readonly scope: ReadonlyArray<{ readonly id: string, readonly group: string }>
  /** A `global` listener is never filtered. */
  readonly global: boolean
}

export interface ListenerOptions {
  /** Run before every listener of the name registered so far. */
  prepend?: boolean
  /** Not subject to emitters' filters. */
  global?: boolean
}

/** A listener: the event's arguments, then, in a `waterfall`, `next` (the rest of the chain) last. */
export type Listener = (...args: any[]) => any

/** The five ways to emit. Each reaches every listener of the name on the root's flat bus, wherever the listener runs, by the same rules as if all were in this process. Emitting `rutis/*` throws `ReservedEvent`; `internal/*` is refused. */
export interface Emitter {
  /** Start every listener in registration order; returns at once. A listener's failure is reported where it ran and reaches neither the emitter nor the other listeners. */
  emit(name: string, ...args: any[]): void
  /** Start every listener and wait for all. Failures reject with an `AggregateError` in registration order, even a single one. */
  parallel(name: string, ...args: any[]): Promise<void>
  /** Run the listeners one by one; resolves to the first result (not `undefined`, `null` or `false`), or `undefined`. An error stops and rejects. */
  serial<T = any>(name: string, ...args: any[]): Promise<T | undefined>
  /** Each listener gets `next` (no arguments) last and returns the result; not calling `next` vetoes the rest. Every argument is the event's; the end of the chain gives `undefined` (see `with` for a terminal). Returns the outermost listener's value as is (a Promise whenever a listener runs elsewhere: await it). Calling `next` twice, or after the end, gives a Promise rejected with `WaterfallNext`. */
  waterfall: Waterfall
  /** Synchronously, the first result as `serial`. An `async` listener, or one returning a Promise, fails it with `AsyncListener`. */
  bail<T = any>(name: string, ...args: any[]): T | undefined
}

export interface Waterfall {
  <T = any>(name: string, ...args: any[]): T | Promise<T>
  /** The same waterfall with a terminal: called with the event's arguments at the end of the chain, its value what the last `next` gives. */
  with<T = any>(terminal: (...args: any[]) => any): <U = T>(name: string, ...args: any[]) => U | Promise<U>
}

/** The plugin API this SDK writes plugins against. */
export declare const PLUGIN_API: number
/** The mark on what `definePlugin` makes (`Symbol.for('rutis.leaf-plugin')`), across copies of this package. */
export declare const PLUGIN: unique symbol
/** Whether `value` is a plugin `definePlugin` made. */
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
