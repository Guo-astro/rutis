export type MethodKind = 'sync' | 'async'

export interface Context {
  /** The service `name`, which the plugin declares in `inject`: the object itself when a plugin in the same process provides it, else a proxy whose methods are sync or async as declared. */
  use<T = any>(name: string): T
  /** Provide `value` as the service `name` until the plugin unloads, or until the returned function is called. */
  provide(name: string, value: unknown): () => unknown
  /** Run `cleanup` when the plugin unloads. */
  effect(cleanup: () => unknown): void
  /** Listen to an event of this runtime's shared event bus: the host emits into it (names declared with `emits` on the runtime) and plugins may emit too. The listener is unregistered when the plugin unloads. */
  on(name: string, listener: (...args: any[]) => any): () => void
  /** Dispatch synchronously to this runtime process's listeners, including other plugins; does not await listener promises. `RuntimePlugin` exposes no `Mount.events` / `EventSink` configuration, so runtime → Rust host forwarding is not available. */
  emit(name: string, ...args: any[]): void
}

export interface PluginSpec<C = any> {
  name?: string
  /** Services the plugin uses: rutis starts it once they are all available, and stops it when one goes. */
  inject?: string[]
  /** Services it provides to rutis, with each method's kind. */
  provides?: Record<string, Record<string, MethodKind>>
  /** The JSON Schema of its config. */
  config?: object
  apply(ctx: Context, config: C): void | (() => unknown) | Promise<void | (() => unknown)>
}

export interface Plugin<C = any> extends Readonly<Required<Pick<PluginSpec<C>, 'inject' | 'provides'>>>, Readonly<Omit<PluginSpec<C>, 'inject' | 'provides'>> {
  /** The plugin API it was written against. */
  readonly api: number
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
