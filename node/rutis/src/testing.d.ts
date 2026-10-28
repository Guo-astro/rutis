import type { Plugin } from './index.js'
import type { Emitter, Listener, ListenerDescriptor, ListenerOptions } from './advanced.js'

export declare class PluginTestError extends Error {
  /** For a refusal a runtime makes too: its code (`not-declared`, `unavailable`, `no-method`). */
  code?: string
  /** For such a refusal: its details, as the runtimes give them (`{ service }`, …). */
  data?: unknown
}

/** The five ways to emit, with `filter`. */
export interface TestEmitter extends Emitter {
  filter(keep: (listener: ListenerDescriptor) => unknown): Emitter
}

/** A failure an `emit` listener (or filter) reported (protocol §5.6). */
export interface Reported {
  /** The line a runtime writes on stderr. */
  line: string
  event: string
  row: string | null
  /** The plugin of the failing listener (absent for a filter). */
  plugin?: string
  error: unknown
  /** The filter failed, not a listener. */
  filter?: boolean
}

/** One loaded plugin. Its emitting methods emit in its scope as the test: values cross as from another process (strict). */
export interface Loaded extends TestEmitter {
  /** The row's key. */
  readonly row: string
  /** The host it is loaded on. */
  readonly host: Host
  /** The service `name` the plugin provides, as rutis sees it: its declared methods, or, undeclared, the object's methods, all async. */
  service<T = any>(name: string): T
  /** The methods rutis sees on the service `name`, with their kinds. */
  methods(name: string): Record<string, 'sync' | 'async'>
  /** The names of the services the plugin provides now. */
  provided(): string[]
  /** What `emit` listeners reported on the host. */
  readonly reported: Reported[]
  /**
   * Listen to the event `name` as the host does, at the root scope: the plugin's emissions of `name` reach `listener`, and so do the test's own.
   * Strict, its arguments and what it returns cross as from another process. Returns the function that stops it; without it, it stops when the row unloads.
   */
  on(name: string, listener: Listener, options?: ListenerOptions): () => void
  /** Change the row's config, as a host's `rows.update`: the row unloads (as `unload`) and applies again with `config` under the same key, so its listeners register again, after every existing one. */
  update(config: unknown): Promise<void>
  /** Withdraw its listeners and services, abort its `ctx.signal` and wait for its running listeners, then run the cleanups, latest first. */
  unload(): Promise<void>
}

export interface LoadOptions {
  config?: unknown
  /** The services the plugin injects, by name. */
  services?: Record<string, unknown>
  /** Values cross as between processes (default true). */
  strict?: boolean
  /** The scope path to load it in (`t1/s1`; the root by default). */
  scope?: string
  /** How long unloading waits for running listeners, in ms (default: `RUTIS_SHUTDOWN_TIMEOUT` seconds, else 10 000, as the runtimes). */
  deadline?: number
  /** What listener descriptors call the runtime (default `'node'`, `'bun'` under Bun). */
  runtime?: string
}

export interface HostOptions {
  /** Services for every row that injects them. */
  services?: Record<string, unknown>
  /** Values cross between the test and the plugins as between processes (default true). */
  strict?: boolean
  /** The group of a scope path: `{ t1: 'tenant', 't1/s1': 'session' }` (default: the path's last part). */
  scopes?: Record<string, string>
  /** How long unloading waits for running listeners, in ms (default: `RUTIS_SHUTDOWN_TIMEOUT` seconds, else 10 000, as the runtimes). */
  deadline?: number
  /** What listener descriptors call the runtime (default `'node'`, `'bun'` under Bun). */
  runtime?: string
}

export interface RowOptions {
  config?: unknown
  /** Services for this row only. */
  services?: Record<string, unknown>
  /** The scope path (`t1/s1`; the root by default). */
  scope?: string
  /** The row's key (default `row1`, `row2`, ...). */
  row?: string
}

/** Plugins in one process sharing one event bus, as a real runtime's (always the fast path); its emitting methods emit at the root as the test. It emits no `rutis/*` events. */
export interface Host extends TestEmitter {
  load(plugin: Plugin | { default: Plugin }, options?: RowOptions): Promise<Loaded>
  /** The emitting methods from the scope path `scope`, as the test. */
  at(scope: string): TestEmitter
  /** What `emit` listeners reported. */
  readonly reported: Reported[]
  /** Unload every row, latest first. */
  unload(): Promise<void>
}

/** A host for several plugins, without rutis. */
export declare function host(options?: HostOptions): Host

/** Load `plugin` (or a module whose default export it is) without a host, as the one row of a `host()` of its own. */
export declare function load(plugin: Plugin | { default: Plugin }, options?: LoadOptions): Promise<Loaded>
