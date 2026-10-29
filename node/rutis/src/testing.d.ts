import type { Plugin } from './index.js'
import type { Emitter, Listener, ListenerDescriptor, ListenerOptions } from './advanced.js'

/** What the test tool refuses: a load that fails (the row's line, as the host prints it), a plugin it cannot load. */
export declare class PluginTestError extends Error {}

/** The five ways to emit, with `filter`. */
export interface TestEmitter extends Emitter {
  filter(keep: (listener: ListenerDescriptor) => unknown): Emitter
}

/** A failure the host reported of a row: a listener failing in an `emit`, or a filter (protocol §5.6). */
export interface Reported {
  /** The line the host prints. */
  line: string
}

/** One loaded plugin: a row of the host. Its emitting methods emit as the test's plugin next to it. */
export interface Loaded extends TestEmitter {
  /** The row's id on the host: its module's file name, `-2`, `-3`… when taken. */
  readonly row: string
  /** The host it is loaded on. */
  readonly host: Host
  /** The service `name` the plugin provides, as rutis sees it: its declared methods, or, undeclared, the object's methods, all async. */
  service<T = any>(name: string): T
  /** The methods rutis sees on the service `name`, with their kinds. */
  methods(name: string): Record<string, 'sync' | 'async'>
  /** The names of the services the plugin provides now. */
  provided(): string[]
  /** What the host reported of this row. */
  reported(): Promise<Reported[]>
  /**
   * Listen to the event `name` as a plugin next to it does: the plugin's emissions of `name` reach `listener`, and so do the test's own.
   * Its arguments and what it returns cross between processes. Returns the function that stops it; without it, it stops when the row unloads.
   */
  on(name: string, listener: Listener, options?: ListenerOptions): () => void
  /** Change the row's config, as a host does. */
  update(config: unknown): Promise<void>
  /** Unload the row as the host does (its listeners and services withdrawn, its `ctx.signal` aborted, its running listeners waited for, its cleanups run, latest first), then the test's services and listeners of it. */
  unload(): Promise<void>
}

export interface LoadOptions {
  config?: unknown
  /** The test's services the plugin injects, by name. */
  services?: Record<string, unknown>
}

export interface HostOptions {
  /** The test's services, for every row that injects them. */
  services?: Record<string, unknown>
}

export interface RowOptions {
  config?: unknown
  /** The test's services for this row only. */
  services?: Record<string, unknown>
  /** The row's id (default: its module's file name, with `-2`, `-3`, … when taken). */
  id?: string
  /** As a row's `isolate`: the services it keeps to a scope of its own (`{ forecast: true }`), or a label. */
  isolate?: Record<string, true | string>
}

/** Plugins on the host, sharing its bus; its emitting methods emit as the test's plugin. */
export interface Host extends TestEmitter {
  load(plugin: Plugin | { default: Plugin }, options?: RowOptions): Promise<Loaded>
  /** What the host reported of the rows here. */
  reported(): Promise<Reported[]>
  /** Unload every row, latest first, then withdraw the test's services. */
  unload(): Promise<void>
}

/** Plugins on the real host of this test process (`rutis-host test`, in the real runtime), which must be their modules' default exports. */
export declare function host(options?: HostOptions): Host

/** Load `plugin` (or a module whose default export it is) on a `host()` of its own. */
export declare function load(plugin: Plugin | { default: Plugin }, options?: LoadOptions): Promise<Loaded>
