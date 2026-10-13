// The Bun runtime: leaf plugins loaded one by one for rutis-loader.
//
// rutis owns the plugin model: dependency gating, start and stop order,
// restarts and configuration. This process only runs what it is told:
//
// - `rows.load` imports a plugin module and runs its `apply(ctx, config)`;
//   `rows.unload` withdraws what the row provided and runs its cleanups;
// - `rows.schema` reports what a module declares (config schema, injected
//   services, provided services and their method kinds, package version);
// - `hosts.provide` / `hosts.withdraw` register rutis services by name;
// - the services a row exports are reported with `service(id, handle,
//   version)` notifications, and calls to a handle reach the object.
//
// Within the process, `ctx.use(name)` returns the object itself when another
// row here provides it, so those calls never leave the process.
// Rows also share an event bus, Cordis's own as on the Node runtime: each row
// owns a Cordis fiber, so `ctx.on` listens until the row unloads, `ctx.emit`
// is Cordis's `emit` and a host `emit` is its `parallel`, which waits for
// every listener. Host `emit` calls may use only the names declared by
// `mount.emits`.
//
// A row's `isolate` gives some names a scope label: rows isolating a name
// with the same label share it, and the same name under another label
// (another instance, say) is another service. Services, host proxies and
// export slots are registered by id (`scopes`): the name outside any scope,
// the name, a NUL and the label inside one.
import { Context, type Fiber } from '@deepseek-ai/cordis'
import type { Client } from './client.ts'
import { declared, locate, Modules, versionOf, type Kind } from './plugin.ts'
import { MANIFEST } from './session.ts'

// internal/* names Cordis's own events on Node; no leaf plugin listens to or
// emits them on either runtime (boundary rule 5).
function reserved(name: string): void {
  if (typeof name === 'string' && name.startsWith('internal/')) throw new Error(`event ${name} is internal to the runtime`)
}

// events: leaf plugins receive what rutis emits (RuntimePlugin::emits).
export const FEATURES = ['rows.v2', 'hosts', 'leaf', 'scopes', 'events']

// How the service `name` is identified in the scope `label`. Neither may
// contain NUL and a label may not be empty, so no two pairs share an id.
export function scopedId(name: string, label?: string | null): string {
  if (name.includes('\0') || label?.includes('\0')) throw new Error(`service ${JSON.stringify(name)} or its scope label contains NUL`)
  if (label === '') throw new Error(`service ${JSON.stringify(name)} has an empty scope label`)
  return label == null ? name : `${name}\0${label}`
}

// The handle of a slot's `generation`th object. A scoped id's handles are
// marked with a NUL too: a label may contain `#`.
export const handleOf = (id: string, generation: number) =>
  generation === 1 ? id : `${id}${id.includes('\0') ? '\0' : '#'}${generation}`

interface Row {
  key: string
  entry: string
  config: unknown
  exports: Record<string, Record<string, Kind>>
  isolate: Map<string, string>
  cleanups: Array<() => unknown>
  // Owns the row's event listeners; disposed first on unload.
  events?: Fiber
  provided: string[]
}
const idIn = (row: Row, name: string) => scopedId(name, row.isolate.get(name))

interface Slot { row: string, methods: Set<string>, object: unknown, handle: string | null, generation: number }
interface Handle { name: string, object: any, current: boolean, released: boolean }
type Listener = (...args: any[]) => unknown

export class Runtime {
  client?: Client
  closing = false
  #project: string
  #modules = new Modules()
  #rows = new Map<string, Row>()
  #services = new Map<string, { row: string, value: unknown }>()
  #hosts = new Map<string, unknown>()
  #slots = new Map<string, Slot>()
  #handles = new Map<string, Handle>()
  #events = new Context()
  #emits = new Set<string>()
  #version = 0

  constructor(project: string) { this.#project = project }

  // ── Services ──────────────────────────────────────────────────

  lookup(id: string): unknown {
    const provided = this.#services.get(id)
    if (provided) return provided.value
    if (this.#hosts.has(id)) return this.#hosts.get(id)
    throw new Error(`service ${id.replace('\0', ' in scope ')} is not available`)
  }

  provide(row: Row, name: string, value: unknown): () => void {
    const id = idIn(row, name)
    const current = this.#services.get(id)
    if (current) throw new Error(`service ${id} is already provided by row ${current.row}`)
    this.#services.set(id, { row: row.key, value })
    row.provided.push(id)
    this.#refresh(id)
    return () => {
      if (this.#services.get(id)?.value !== value) return
      this.#services.delete(id)
      row.provided = row.provided.filter(provided => provided !== id)
      this.#refresh(id)
    }
  }

  // Report the object now in an exported slot, under a new handle.
  #refresh(id: string) {
    const slot = this.#slots.get(id)
    if (!slot) return
    const provided = this.#services.get(id)
    const current = provided && provided.row === slot.row ? provided.value : undefined
    if (current === slot.object) return
    if (slot.handle !== null) this.#retire(slot.handle)
    slot.object = current
    slot.handle = null
    if (current !== undefined) {
      slot.generation++
      slot.handle = handleOf(id, slot.generation)
      this.#handles.set(slot.handle, { name: id, object: current, current: true, released: false })
    }
    this.#version++
    // A notification: sent before this returns, so it goes out ahead of the
    // reply of the call that caused it.
    if (this.client && !this.closing) this.client.callAsync('', 'service', [id, slot.handle, this.#version]).catch(() => {})
  }

  #retire(handle: string) {
    const entry = this.#handles.get(handle)
    if (!entry) return
    entry.current = false
    if (entry.released) this.#handles.delete(handle)
  }

  // ── Events ────────────────────────────────────────────────────

  // The row's Cordis context: listeners registered there leave with the row.
  async #fiber(row: Row): Promise<Context> {
    let context!: Context
    row.events = this.#events.plugin({ name: `row:${row.key}`, apply: (ctx: Context) => { context = ctx } })
    await row.events.await()
    return context
  }

  // ── Rows ──────────────────────────────────────────────────────

  async load(key: string, entry: string, config: unknown, isolate: Array<[string, string]> | null, exports: Row['exports'] | null) {
    if (this.#rows.has(key)) throw new Error(`row ${key} is already loaded`)
    const row: Row = { key, entry, config, exports: exports ?? {}, isolate: new Map(), cleanups: [], provided: [] }
    for (const [name, label] of isolate ?? []) { scopedId(name, label); row.isolate.set(name, label) }
    for (const name of Object.keys(row.exports)) {
      if (name.includes('#')) throw new Error(`service name ${name} cannot be projected`)
      const owner = this.#slots.get(idIn(row, name))
      if (owner) throw new Error(`service ${idIn(row, name)} is already exported by row ${owner.row}`)
    }
    const plugin = await this.#plugin(entry)
    this.#rows.set(key, row)
    for (const [name, methods] of Object.entries(row.exports)) {
      this.#slots.set(idIn(row, name), { row: key, methods: new Set(Object.keys(methods ?? {})), object: undefined, handle: null, generation: 0 })
    }
    const events = await this.#fiber(row)
    const ctx = {
      use: (name: string) => this.lookup(idIn(row, name)),
      provide: (name: string, value: unknown) => this.provide(row, name, value),
      effect: (cleanup: () => unknown) => {
        if (typeof cleanup !== 'function') throw new TypeError('effect needs a cleanup function')
        row.cleanups.push(cleanup)
      },
      on: (name: string, callback: Listener) => {
        reserved(name)
        if (this.#rows.get(row.key) !== row) throw new Error(`row ${row.key} is not active`)
        return events.on(name, callback)
      },
      emit: (name: string, ...args: any[]) => { reserved(name); events.emit(name, ...args) },
    }
    try {
      const cleanup = await plugin.apply(ctx, config)
      if (cleanup !== undefined && cleanup !== null) {
        if (typeof cleanup !== 'function') throw new TypeError('apply must return a cleanup function or nothing')
        row.cleanups.push(cleanup as () => unknown)
      }
      for (const name of Object.keys(row.exports)) this.#refresh(idIn(row, name))
    } catch (error) {
      await this.unload(key).catch(() => {})
      throw error
    }
    return null
  }

  async unload(key: string) {
    const row = this.#rows.get(key)
    if (!row) return null
    this.#rows.delete(key)
    await row.events?.dispose()
    // Withdrawals first: rutis hears them before the plugin goes away.
    for (const id of row.provided) {
      if (this.#services.get(id)?.row === key) { this.#services.delete(id); this.#refresh(id) }
    }
    row.provided = []
    for (const name of Object.keys(row.exports)) {
      const id = idIn(row, name)
      const slot = this.#slots.get(id)
      this.#slots.delete(id)
      if (slot?.handle) this.#retire(slot.handle)
    }
    const errors: unknown[] = []
    for (const cleanup of row.cleanups.splice(0).reverse()) {
      try { await cleanup() } catch (error) { errors.push(error) }
    }
    if (errors.length) throw errors[0]
    return null
  }

  // Leaf plugins have no volatile fields: a new config restarts the row.
  async update(key: string, config: unknown) {
    const row = this.#rows.get(key)
    if (!row) throw new Error(`row ${key} is not loaded`)
    await this.unload(key)
    return this.load(key, row.entry, config, [...row.isolate], row.exports)
  }

  async #plugin(entry: string) {
    return declared(await this.#modules.import(locate(entry, this.#project)), entry)
  }

  async describe(entry: string) {
    const file = locate(entry, this.#project)
    const plugin = declared(await this.#modules.import(file), entry)
    return { config: plugin.config, inject: plugin.inject, provides: plugin.provides, version: versionOf(entry, file) }
  }

  async dispose() {
    for (const key of [...this.#rows.keys()].reverse()) {
      try { await this.unload(key) } catch {}
    }
  }

  // ── Dispatch ──────────────────────────────────────────────────

  dispatch = (target: string, method: string, args: any): unknown => {
    if (this.closing) throw new Error('runtime is closing')
    if (target === '') return this.#control(method, args ?? [])
    const entry = this.#handles.get(target)
    if (!entry) throw new Error(`unknown or released service object ${target}`)
    const slot = this.#slots.get(entry.name)
    if (slot && !slot.methods.has(method)) throw new Error(`unknown service method ${entry.name}.${method}`)
    if (!Array.isArray(args)) throw new TypeError('method arguments must be an array')
    const operation = entry.object[method]
    if (typeof operation !== 'function') throw new TypeError(`${entry.name}.${method} is not a method`)
    return Reflect.apply(operation, entry.object, args)
  }

  #control(method: string, args: any): unknown {
    switch (method) {
      case 'mount':
        for (const name of args.emits ?? []) {
          if (String(name).startsWith('internal/')) throw new Error(`event ${name} is internal to the runtime and cannot be emitted from rutis`)
        }
        this.#emits = new Set(args.emits ?? [])
        return {
          services: {},
          features: FEATURES,
          implementation: { name: MANIFEST.name, version: MANIFEST.version },
          engine: { name: 'bun', version: Bun.version },
        }
      case 'dispose':
        this.closing = true
        return (async () => { await this.dispose(); await this.client?.drain(); return null })()
      case 'emit': {
        const [name, values] = args
        if (!this.#emits.has(name)) throw new Error(`event ${name} is not declared for emission from rutis`)
        return this.#events.parallel(name, ...(values ?? []))
      }
      case 'rows.load': {
        const [key, entry, config, isolate, _inject, exports] = args
        return this.load(key, String(entry), config, isolate, exports)
      }
      case 'rows.update': return this.update(args[0], args[1])
      case 'rows.unload': return this.unload(args[0])
      case 'rows.schema': return this.describe(String(args[0]))
      case 'hosts.provide': {
        // With a label, only rows isolating the name with it see it.
        const [name, methods, label] = args
        const id = scopedId(name, label)
        if (this.#hosts.has(id)) throw new Error(`host service ${id} is already provided`)
        this.#hosts.set(id, this.#hostProxy(id, methods ?? {}))
        return null
      }
      case 'hosts.withdraw':
        this.#hosts.delete(args[0])
        return null
      case 'release': {
        const entry = this.#handles.get(args[0])
        if (entry) { entry.released = true; if (!entry.current) this.#handles.delete(args[0]) }
        return null
      }
      case 'get': {
        const [handle, property] = args
        const entry = this.#handles.get(handle)
        if (!entry) throw new Error(`unknown or released service object ${handle}`)
        return entry.object[property]
      }
      default: throw new Error(`unknown control method ${method}`)
    }
  }

  // A rutis service: its declared methods call the host, synchronously or
  // returning a Promise, as declared; any other method is reported as not
  // provided rather than silently missing. `id` is the service's id, which
  // its calls target.
  #hostProxy(id: string, methods: Record<string, Kind>) {
    const target: Record<string, unknown> = {}
    for (const [method, kind] of Object.entries(methods)) {
      target[method] = kind === 'async'
        ? (...args: unknown[]) => this.client!.callAsync(`host:${id}`, method, args)
        : (...args: unknown[]) => this.client!.call(`host:${id}`, method, args)
    }
    const passthrough = new Set(['then', 'toJSON', 'constructor'])
    return new Proxy(target, {
      get(target, property, receiver) {
        if (typeof property !== 'string' || property in target || passthrough.has(property)) return Reflect.get(target, property, receiver)
        return () => { throw new Error(`${id}.${property} is not provided by the rutis host`) }
      },
    })
  }
}
