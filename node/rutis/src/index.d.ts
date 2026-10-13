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
  /** Dispatch synchronously to this runtime process's listeners, including other plugins; does not await listener promises. Forwarding to a Rust host requires a Node mount configured with `Mount.events` and an `EventSink`. */
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

export interface Plugin<C = any> extends Readonly<Required<Pick<PluginSpec<C>, 'inject' | 'provides'>>>, Readonly<PluginSpec<C>> {
  /** The plugin API it was written against. */
  readonly api: number
}

/** The plugin API this SDK writes plugins against. */
export declare const PLUGIN_API: number
export declare const PLUGIN: unique symbol
export declare function definePlugin<C = any>(spec: PluginSpec<C>): Plugin<C>
export declare function isPlugin(value: unknown): value is Plugin
