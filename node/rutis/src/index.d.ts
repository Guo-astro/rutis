import type * as advanced from './advanced.js'

/**
 * An error a listener threw arrives, through the host, as an `Error` of the
 * same built-in class (`TypeError`, `RangeError`, …, `AggregateError`) or a
 * plain `Error` with the thrower's `name`, `message` and `code`, never as
 * the thrower's own class: match errors by `name`.
 */
export interface Context extends advanced.Emitter {
  /** The service `name`, which the plugin declares in `inject`: the object itself when a plugin in the same process provides it, else a proxy whose methods are sync or async as declared. */
  use<T = any>(name: string): T
  /** Provide `value` as the service `name` until the plugin unloads, or until the returned function is called. It reaches rutis by itself: its methods are the object's functions (own and inherited, not `constructor`), `async` unless `provides` marks them `sync`. */
  provide(name: string, value: unknown): () => unknown
  /** Run `cleanup` when the plugin unloads. */
  effect(cleanup: () => unknown): void
  /** Aborted when the plugin unloads; its running listeners are then waited for (up to the host's shutdown deadline) before the cleanups run. */
  readonly signal: AbortSignal
  /** Listen to `name` on the host's logical bus: every event of that name, from any process or language (a scope is a descriptor). Registered with the host by itself; unregistered when the plugin unloads or the returned function is called. `rutis/*` (the host's framework events) can be listened to; `internal/*` cannot. */
  on(name: string, listener: advanced.Listener, options?: advanced.ListenerOptions): () => void
  /** The five ways to emit, delivered only to the listeners `keep` returns true for (`global` listeners are always delivered). `keep` runs in this process, once per candidate, before any listener runs. */
  filter(keep: (listener: advanced.ListenerDescriptor) => unknown): advanced.Emitter
}

export interface PluginSpec<C = any> {
  /** Its name: what listener descriptors (`ListenerDescriptor.plugin`) call it. Without one, they call it by its row's id. */
  name?: string
  /** Services the plugin uses: rutis starts it once they are all available, and stops it when one goes. */
  inject?: string[]
  /** Optional: services it provides, with the kind of methods to call `sync` (the others are `async`). Without it, what `ctx.provide` provides still reaches rutis; with it, `rutis-host check` knows before the plugin runs. */
  provides?: Record<string, Record<string, advanced.MethodKind>>
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

export declare function definePlugin<C = any>(spec: PluginSpec<C>): Plugin<C>
