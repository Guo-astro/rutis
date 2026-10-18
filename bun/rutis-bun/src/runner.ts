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
//   version)` notifications, and calls to a handle reach the object; one it
//   provides without declaring it is announced to a host that asks, with
//   the methods its object has (`rows.announce`).
//
// Within the process, `ctx.use(name)` returns the object itself when another
// row here provides it, so those calls never leave the process.
// Events (#464, ./events.mjs, the same file as the Node runtime's): every
// listener a row registers is on the host's logical bus; an emission runs
// here when every listener of the name is here, else through the host,
// with the five modes, scopes (each row's chain, from `rows.load`), filters
// and unload waiting (the row's `ctx.signal`).
//
// A row's `isolate` gives some names a scope label: rows isolating a name
// with the same label share it, and the same name under another label
// (another instance, say) is another service. Services, host proxies and
// export slots are registered by id (`scopes`): the name outside any scope,
// the name, a NUL and the label inside one.
import type { Client } from './client.ts'
import { Bus, refuseRemoved, refuseReserved } from './events.mjs'
import { declared, locate, Modules, versionOf, type Kind } from './plugin.ts'
import { MANIFEST } from './session.ts'

// internal/* names Cordis's own events on Node; no leaf plugin listens to or
// emits them (internal/* stays in each runtime). The wording matches the
// Node side (`node/rutis-runtime/src/leaf.mjs`).
const internalMessage = (name: unknown) => `event ${name} is internal to the runtime`

// A name is a string: Cordis's `emit` takes an object, null or function first
// argument as `this`, so any other type would let `emit(null, 'internal/service')`
// past the check.
function reserved(name: unknown): asserts name is string {
  if (typeof name !== 'string') throw new TypeError(`an event name is a string, not ${name === null ? 'null' : typeof name}`)
  if (name.startsWith('internal/')) throw new Error(internalMessage(name))
}

// rows.announce: services rows provide without declaring them are announced
// with their methods, to a host whose mount says `announce`.
export const FEATURES = ['rows.v2', 'hosts', 'leaf', 'scopes', 'rows.announce']

// How long an unloading row's listeners are waited for: RUTIS_SHUTDOWN_TIMEOUT
// seconds, 10 by default; in ms (protocol §7.1).
function shutdownDeadline(): number {
  const seconds = Number(process.env.RUTIS_SHUTDOWN_TIMEOUT)
  return Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : 10_000
}

interface Scope { id: string, group: string }
// A scope chain as rows.load gives it: [{ id, group }], the root [].
function chainOf(scope: unknown): Scope[] {
  if (scope === undefined || scope === null) return []
  if (!Array.isArray(scope) || scope.some(item => typeof item?.id !== 'string' || typeof item?.group !== 'string')) throw new TypeError('a scope is a list of { id, group }')
  return scope.map(({ id, group }) => ({ id, group }))
}

// The methods of a service no declaration describes: its own and inherited
// functions, up to Object's, not the constructor; a property that is not a
// function hides an inherited function of the same name. All async.
export function methodsOf(object: unknown): Set<string> {
  const methods = new Set<string>()
  if (object === null || (typeof object !== 'object' && typeof object !== 'function')) return methods
  const seen = new Set<string>()
  for (let level: any = object; level != null && level !== Object.prototype && level !== Function.prototype; level = Object.getPrototypeOf(level)) {
    for (const name of Object.getOwnPropertyNames(level)) {
      if (seen.has(name)) continue
      seen.add(name)
      if (name !== 'constructor' && typeof Object.getOwnPropertyDescriptor(level, name)?.value === 'function') methods.add(name)
    }
  }
  return methods
}

// How the service `name` is identified in the scope `label`. Neither may
// contain NUL and a label may not be empty, so no two pairs share an id.
// What a plugin providing the undeclared service `name` is told when the
// host, older than 0.12, mounted without `announce`: such a host does not
// take the service, which stays in this process.
export const oldHost = (name: string) =>
  `rutis: service ${name} is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host`

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
  // Its scope chain, and the signal its unload aborts.
  chain: Scope[]
  abort: AbortController
  provided: string[]
  // Ids of the slots of services it announced.
  announced: string[]
  // Announcements the host has yet to answer, while the row loads.
  pending: Array<Promise<unknown>> | null
}
const idIn = (row: Row, name: string) => scopedId(name, row.isolate.get(name))

interface Slot { row: string, methods: Set<string>, object: unknown, handle: string | null, generation: number, announced?: boolean }
interface Handle { name: string, object: any, current: boolean, released: boolean }
type Listener = (...args: any[]) => unknown

export class Runtime {
  client?: Client
  closing = false
  #bus = new Bus()
  #deadline = shutdownDeadline()
  #project: string
  #modules = new Modules()
  #rows = new Map<string, Row>()
  #services = new Map<string, { row: string, value: unknown }>()
  #hosts = new Map<string, unknown>()
  #slots = new Map<string, Slot>()
  #handles = new Map<string, Handle>()
  #version = 0
  // The host takes announced services (mount `announce`).
  #announce = false
  // Mounted by a host; one without `announce` is older than 0.12.
  #mounted = false
  // The undeclared names a plugin was told such a host does not take.
  #told = new Set<string>()

  constructor(project: string) { this.#project = project }

  // Connected: the bus registers and dispatches through the host.
  attach(client: Client) {
    this.client = client
    this.#bus.peer = {
      call: (target: string, method: string, args: unknown) => client.call(target, method, args),
      callAsync: (target: string, method: string, args: unknown) => client.callAsync(target, method, args),
      callFunctionAsync: (fn: unknown, args: unknown[]) => client.callFunctionAsync(fn, args),
    }
  }

  // The session ended: nothing is elsewhere any more.
  ended() { this.#bus.ended() }

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
    // Not declared: announced, with the methods it has; a host older than
    // 0.12 does not take it, and the plugin is told, once per name.
    if (!this.#slots.has(id) && !name.includes('#')) {
      if (this.#announce) {
        this.#slots.set(id, { row: row.key, methods: new Set(), object: undefined, handle: null, generation: 0, announced: true })
        row.announced.push(id)
      } else if (this.#mounted && !this.#told.has(name)) {
        this.#told.add(name)
        process.stderr.write(`${oldHost(name)}\n`)
      }
    }
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
      if (slot.announced) slot.methods = methodsOf(current)
    }
    this.#version++
    // A notification: sent before this returns, so it goes out ahead of the
    // reply of the call that caused it. An announced service says whose it
    // is and what it offers, and a row loading answers once rutis heard it.
    if (this.client && !this.closing) {
      const args = slot.announced
        ? [id, slot.handle, this.#version, { row: slot.row, methods: Object.fromEntries([...slot.methods].map(method => [method, 'async'])) }]
        : [id, slot.handle, this.#version]
      const sent = this.client.callAsync('', 'service', args).catch(() => {})
      if (slot.announced) this.#rows.get(slot.row)?.pending?.push(sent)
    }
  }

  #retire(handle: string) {
    const entry = this.#handles.get(handle)
    if (!entry) return
    entry.current = false
    if (entry.released) this.#handles.delete(handle)
  }

  // ── Events ────────────────────────────────────────────────────

  // The five ways to emit from `row`, filtered by `keep` when given
  // (`waterfall`'s last argument is its terminal).
  #emitters(row: Row, keep?: (descriptor: unknown) => unknown) {
    const dispatch = (mode: string, name: string, values: unknown[], terminal?: unknown) => {
      reserved(name)
      refuseReserved(name)
      return this.#bus.dispatch(mode, name, values, { row: row.key, scope: row.chain, keep, terminal })
    }
    return {
      emit: (name: string, ...values: unknown[]) => { dispatch('emit', name, values) },
      parallel: (name: string, ...values: unknown[]) => dispatch('parallel', name, values),
      serial: (name: string, ...values: unknown[]) => dispatch('serial', name, values),
      waterfall: (name: string, ...values: unknown[]) => { const terminal = values.pop(); return dispatch('waterfall', name, values, terminal) },
      bail: (name: string, ...values: unknown[]) => dispatch('bail', name, values),
    }
  }

  // ── Rows ──────────────────────────────────────────────────────

  async load(key: string, entry: string, config: unknown, isolate: Array<[string, string]> | null, exports: Row['exports'] | null, scope?: unknown) {
    if (this.#rows.has(key)) throw new Error(`row ${key} is already loaded`)
    const row: Row = { key, entry, config, exports: exports ?? {}, isolate: new Map(), cleanups: [], chain: chainOf(scope), abort: new AbortController(), provided: [], announced: [], pending: [] }
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
    const ctx = {
      use: (name: string) => this.lookup(idIn(row, name)),
      provide: (name: string, value: unknown) => this.provide(row, name, value),
      effect: (cleanup: () => unknown) => {
        if (typeof cleanup !== 'function') throw new TypeError('effect needs a cleanup function')
        row.cleanups.push(cleanup)
      },
      // Aborted when the row unloads, while its listeners are waited for.
      get signal() { return row.abort.signal },
      // On the host's bus until the row unloads, or the returned function
      // is called. `prepend` and `global` as Cordis's.
      on: (name: string, callback: Listener, options?: { prepend?: boolean, global?: boolean }) => {
        reserved(name)
        if (this.#rows.get(row.key) !== row) throw new Error(`row ${row.key} is not active`)
        if (typeof callback !== 'function') throw new TypeError('a listener is a function')
        const listener = this.#bus.on({ name, callback, prepend: options?.prepend, global: options?.global, row: row.key, plugin: plugin.name ?? key, scope: row.chain })
        return () => { this.#bus.off(listener) }
      },
      ...this.#emitters(row),
      filter: (predicate: (descriptor: unknown) => unknown) => {
        if (typeof predicate !== 'function') throw new TypeError('filter needs a predicate function')
        return this.#emitters(row, predicate)
      },
    }
    try {
      const cleanup = await plugin.apply(ctx, config)
      if (cleanup !== undefined && cleanup !== null) {
        if (typeof cleanup !== 'function') throw new TypeError('apply must return a cleanup function or nothing')
        row.cleanups.push(cleanup as () => unknown)
      }
      for (const name of Object.keys(row.exports)) this.#refresh(idIn(row, name))
      // rutis has published the announced services (or refused one, which
      // fails the row there) before it hears that the row loaded; so are
      // the listeners it registered (protocol §4.4).
      await Promise.all(row.pending ?? [])
      await Promise.all(this.#bus.registrations(key))
      row.pending = null
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
    // Its listeners are off the bus at once (protocol §7.1).
    this.#bus.withdraw(key)
    // Withdrawals first: rutis hears them before the plugin goes away.
    for (const id of row.provided) {
      if (this.#services.get(id)?.row === key) { this.#services.delete(id); this.#refresh(id) }
    }
    row.provided = []
    for (const id of [...Object.keys(row.exports).map(name => idIn(row, name)), ...row.announced]) {
      const slot = this.#slots.get(id)
      if (slot?.row !== key) continue
      this.#slots.delete(id)
      if (slot.handle) this.#retire(slot.handle)
    }
    row.announced = []
    // Its running listeners are signalled and waited for, at most the
    // shutdown deadline; then its cleanups run.
    row.abort.abort(new DOMException('the row is unloading', 'AbortError'))
    await this.#bus.settle(key, this.#deadline)
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
    return this.load(key, row.entry, config, [...row.isolate], row.exports, row.chain)
  }

  async #plugin(entry: string) {
    return declared(await this.#modules.import(locate(entry, this.#project)), entry)
  }

  async describe(entry: string) {
    const file = locate(entry, this.#project)
    const plugin = declared(await this.#modules.import(file), entry)
    return { config: plugin.config, inject: plugin.inject, provides: plugin.provides, announces: plugin.announces, version: versionOf(entry, file) }
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
        // Listeners register themselves (#464): the static forwarding is gone.
        refuseRemoved(args)
        // This runtime's name on the host, for descriptors, when it says.
        if (typeof args.runtime === 'string') this.#bus.runtime = args.runtime
        this.#announce = args.announce === true
        this.#mounted = true
        return {
          services: {},
          features: FEATURES,
          implementation: { name: MANIFEST.name, version: MANIFEST.version },
          engine: { name: 'bun', version: Bun.version },
        }
      case 'dispose':
        this.closing = true
        return (async () => { await this.dispose(); await this.client?.drain(); return null })()
      // The logical bus: `events.elsewhere` runs as its frame arrives (the
      // session's `inline`); `events.call` runs a listener here.
      case 'events.elsewhere': this.#bus.elsewhere(args[0] ?? {}); return undefined
      case 'events.call': return this.#bus.call(args[0] ?? {})
      case 'rows.load': {
        const [key, entry, config, isolate, _inject, exports, place] = args
        return this.load(key, String(entry), config, isolate, exports, place?.scope)
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
