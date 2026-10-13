import type { Plugin } from './index.js'

export declare class PluginTestError extends Error {}

export interface Loaded {
  /** The service `name` the plugin provides, as rutis sees it: its declared methods only. */
  service<T = any>(name: string): T
  /** The names of the services the plugin provides now. */
  provided(): string[]
  /** Emit as the host would with `Process::emit`: arguments cross in strict mode; waits for every listener and rejects with AggregateError if any fail. */
  emit(name: string, ...args: any[]): Promise<void>
  /** Run the cleanups, latest first; the plugin's services are withdrawn. */
  unload(): Promise<void>
}

export interface LoadOptions {
  config?: unknown
  /** The services the plugin injects, by name. */
  services?: Record<string, unknown>
  /** Values cross as between processes (default true). */
  strict?: boolean
  /** The events `emit` may send, as `RuntimePlugin::emits` declares them on a real runtime (default none). */
  emits?: string[]
}

/** Load `plugin` (or a module whose default export it is) without a host. */
export declare function load(plugin: Plugin | { default: Plugin }, options?: LoadOptions): Promise<Loaded>
