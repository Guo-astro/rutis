// The Bun runtime: leaf plugins loaded one by one for rutis-loader.
//
// rutis owns the plugin model: dependency gating, start and stop order,
// restarts and configuration. This process only runs what it is told:
//
// - `rows.load` imports a plugin module and runs its `apply(ctx, config)`;
//   `rows.unload` withdraws what the row provided and runs its cleanups;
// - `rows.schema` reports what a module declares (config schema, injected
//   and optional services, provided services and their method kinds,
//   package version);
// - `hosts.provide` / `hosts.withdraw` register rutis services by name;
// - the services a row exports are reported with `service(id, handle,
//   version)` notifications, and calls to a handle reach the object; one it
//   provides without declaring it is announced to a host that asks, with
//   the methods its object has (`rows.announce`).
//
// Within the process, `ctx.use(name)` (and `ctx.get(name)`, for an optional
// service) returns the object itself when another row here provides it, so
// those calls never leave the process.
// Events (#464, ./events.mjs, the same file as the Node runtime's): every
// listener a row registers is on the host's logical bus, and every
// emission goes through the host, which dispatches it with the five modes,
// scopes (each row's chain, from `rows.load`) and filters; a row's unload
// waits for its running listeners (the row's `ctx.signal`).
//
// A row's `isolate` gives some names a scope label: rows isolating a name
// with the same label share it, and the same name under another label
// (another instance, say) is another service. Services, host proxies and
// export slots are registered by id (`scopes`): the name outside any scope,
// the name, a NUL and the label inside one.
import { realpathSync } from 'node:fs'
import { AsyncLocalStorage } from 'node:async_hooks'
import type { Client } from './client.ts'
import { Bus, refuseReserved } from './events.mjs'
import { definition, locate, Modules, versionOf, type Kind } from './plugin.ts'
import { registeredDefinition } from './definitions.ts'
import { followRow, forgetRow } from './rows.ts'
import { MANIFEST } from './session.ts'
import { UNDECLARED, noMethod, notDeclared, notDeclaredOptional, serviceName, unavailable } from './errors.ts'
import { prefixOutput, say, watchBlocking } from './output.mjs'

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
// with their methods.
// plugin-values: a plugin definition this runtime handed over comes back to
// be described (`plugins.read`) and loaded as a row's entry
// (docs/design-plugin-values-2026-10-25.md §5).
export const FEATURES = ['rows.v2', 'hosts', 'leaf', 'scopes', 'rows.announce', 'plugin-values']

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

// What a plugin using `name`, which neither its `inject` nor its row's
// `inject` declares and which it does not provide itself (one only in its
// `optional` is read with `get`), is told whether or not the service is
// there: what to declare, not that it is missing (#676). The same in every runtime and testing tool.
export const undeclaredUse = UNDECLARED

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
  /// A module, or a plugin definition this runtime handed over.
  entry: unknown
  config: unknown
  exports: Record<string, Record<string, Kind>>
  isolate: Map<string, string>
  // The unload stack: cleanups and the withdrawals of the services the row
  // provides, in the order they were registered, run in reverse when the
  // row unloads (as Rust's effects: a withdrawal is an effect too).
  steps: Array<() => unknown>
  // Its scope chain, and the signal its unload aborts.
  chain: Scope[]
  abort: AbortController
  provided: string[]
  // Ids of the slots of services it announced.
  announced: string[]
  // Announcements the host has yet to answer, while the row loads.
  pending: Array<Promise<unknown>> | null
  // The row's own `inject` (extra names it waits for).
  inject: string[] | null
  // Whether its unload stack ended: from then on it emits nothing
  // (protocol §7.1; while the stack runs, its cleanups' emissions still
  // reach the other rows).
  unloaded?: boolean
}
const idIn = (row: Row, name: string) => scopedId(name, row.isolate.get(name))

interface Slot { row: string, methods: Set<string>, object: unknown, handle: string | null, generation: number, announced?: boolean }
// A service a row of this process provides. `unloading`: its row is
// unloading, so no other row sees it any more, its own row still does until
// the withdrawal on its unload stack runs.
interface Provided { row: string, value: unknown, unloading: boolean }
interface Handle { name: string, object: any, methods: Set<string>, current: boolean, released: boolean, gone?: boolean }
type Listener = (...args: any[]) => unknown

export class Runtime {
  client?: Client
  closing = false
  // With RUTIS_PREFIX_OUTPUT=1 (#678, ./output.mjs), which row's code runs:
  // its apply and cleanups, its service methods, its listeners and the
  // functions it hands over run in its async context, and each line they
  // write starts with its name.
  #store = process.env.RUTIS_PREFIX_OUTPUT === '1' ? new AsyncLocalStorage<string | undefined>() : undefined
  /** Run `run` as row `key`'s code (undefined: no row's). */
  within = <T>(key: string | undefined, run: () => T): T => this.#store ? this.#store.run(key, run) : run()
  /** The row whose code runs now. */
  running = (): string | undefined => this.#store?.getStore()
  // A row's listener the host refuses while the row loads fails the load
  // (protocol §11).
  // Its lines (a listener that failed in `emit`) are the runtime's own:
  // never prefixed with a row's name.
  #bus = new Bus({ loading: (key: string) => Array.isArray(this.#rows.get(key)?.pending), within: this.within, report: (line: string) => say(`${line}\n`) })
  // A row's method holding this thread for 5 s is said (#678).
  #blocking = watchBlocking({ runtime: () => this.name })
  #deadline = shutdownDeadline()
  #project: string
  #modules = new Modules()
  #rows = new Map<string, Row>()
  #services = new Map<string, Provided>()
  #hosts = new Map<string, unknown>()
  #slots = new Map<string, Slot>()
  #handles = new Map<string, Handle>()
  // The last generation each id's handles reached, kept as its slots go and
  // come again (a row restarting): a handle is never given to two objects,
  // so one held for a retired object stays retired.
  #generations = new Map<string, number>()
  #version = 0

  constructor(project: string) {
    this.#project = project
    if (this.#store) {
      prefixOutput(() => {
        const key = this.running()
        return key === undefined ? this.name : this.#bus.rowId(key) ?? key
      })
    }
  }

  // Connected: the bus registers and dispatches through the host.
  attach(client: Client) {
    this.client = client
    this.#bus.peer = {
      call: (target: string, method: string, args: unknown, described?: unknown) => client.call(target, method, args, described),
      callAsync: (target: string, method: string, args: unknown, described?: unknown) => client.callAsync(target, method, args, described),
      callAnswered: (target: string, method: string, args: unknown, answered: (result: { ok: boolean, value: unknown }) => void, described?: unknown) => client.callAnswered(target, method, args, answered, described),
      callFunctionAsync: (fn: unknown, args: unknown[]) => client.callFunctionAsync(fn, args),
      callFunctionSync: (fn: unknown, args: unknown[]) => client.callFunctionSync(fn, args),
      withinSync: (run: () => unknown) => (typeof client.withinSync === 'function' ? client.withinSync(run) : run()),
    }
  }

  // The session ended: nothing more is dispatched.
  ended() { this.#bus.ended() }
  /** This runtime's name on the host (`mount`'s `runtime`), or `bun`. */
  get name(): string { return this.#bus.runtime ?? 'bun' }
  /** The id of the one row whose plugin file is among `files` (real
   * paths), which names the row calling process.exit (#688). */
  rowOfFiles(files: Set<string>): string | undefined {
    const found = [...this.#rows.values()].filter(row => {
      if (typeof row.entry !== 'string' || registeredDefinition(row.entry) !== undefined) return false
      try { return files.has(realpathSync(locate(row.entry, this.#project))) } catch { return false }
    })
    return found.length === 1 ? this.#bus.rowId(found[0].key) ?? undefined : undefined
  }

  // ── Services ──────────────────────────────────────────────────

  lookup(id: string, reader?: string): unknown {
    const found = this.#find(id, reader)
    if (found !== undefined) return found
    throw unavailable(serviceName(id))
  }

  // The service `id` now, as the row `reader` sees it: a row's here, else
  // the host's; else undefined. One whose row is unloading is that row's
  // still, and no other's: as Rust's read_binding_as, where a provider out
  // of Active hides what it provides from every reader but its own subtree.
  #find(id: string, reader?: string): unknown {
    const provided = this.#services.get(id)
    if (provided && (!provided.unloading || provided.row === reader)) return provided.value
    return this.#hosts.get(id)
  }

  provide(row: Row, name: string, value: unknown): () => void {
    const id = idIn(row, name)
    const current = this.#services.get(id)
    // `forecast is already provided by a: rename one of them` (§8.1 row
    // 13, #937): the row's name, and what to do.
    if (current) throw new Error(`${name} is already provided by ${this.#bus.rowId(current.row)}: rename one of them`)
    const entry: Provided = { row: row.key, value, unloading: false }
    this.#services.set(id, entry)
    row.provided.push(id)
    // Not declared: announced, with the methods it has.
    if (!this.#slots.has(id) && !name.includes('#')) {
      this.#slots.set(id, { row: row.key, methods: new Set(), object: undefined, handle: null, generation: 0, announced: true })
      row.announced.push(id)
    }
    this.#refresh(id)
    // Whichever comes first, the returned function or the registration's own
    // entry on the unload stack, withdraws it; the other finds it gone (or the
    // name provided again, which it leaves alone: the entry is this one).
    const withdraw = () => {
      if (this.#services.get(id) !== entry) return
      this.#services.delete(id)
      row.provided = row.provided.filter(provided => provided !== id)
      this.#refresh(id)
    }
    // The withdrawal is an entry of the unload stack, as an effect is: a
    // cleanup registered after this one runs before it and still reads the
    // service; one registered before runs after it and does not.
    row.steps.push(withdraw)
    return withdraw
  }

  // Report the object now in an exported slot, under a new handle.
  #refresh(id: string) {
    const slot = this.#slots.get(id)
    if (!slot) return
    const provided = this.#services.get(id)
    const current = provided && provided.row === slot.row && !provided.unloading ? provided.value : undefined
    if (current === slot.object) return
    if (slot.handle !== null) this.#retire(slot.handle)
    slot.object = current
    slot.handle = null
    if (current !== undefined) {
      slot.generation = (this.#generations.get(id) ?? 0) + 1
      this.#generations.set(id, slot.generation)
      slot.handle = handleOf(id, slot.generation)
      if (slot.announced) slot.methods = methodsOf(current)
      // A handle answers with its object's methods: one its row replaced
      // keeps the old object's, as the old object.
      this.#handles.set(slot.handle, { name: id, object: current, methods: slot.methods, current: true, released: false })
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

  // The row of slot `id` unloaded: every handle on its objects, the current
  // one and those it replaced, says the service is gone (to a row holding
  // it, an optional service leased at start); the released ones are
  // forgotten. A handle its row replaced while running keeps its object
  // until then, as a snapshot of a service does in Rust.
  #gone(id: string) {
    for (const [handle, entry] of this.#handles) {
      if (entry.name !== id) continue
      entry.current = false
      entry.gone = true
      if (entry.released) this.#handles.delete(handle)
    }
  }

  // ── Events ────────────────────────────────────────────────────

  // The five ways to emit from `row`, filtered by `keep` when given. Every
  // argument of `waterfall` is the event's; a terminal is given with
  // `waterfall.with(terminal)(name, ...args)`. A row whose unload ended (its
  // key perhaps taken by a new row since) emits nothing (protocol §3, §7.1);
  // one on its unload stack still emits, and the other rows hear it.
  #emitters(row: Row, keep?: (descriptor: unknown) => unknown) {
    const dispatch = (mode: string, name: string, values: unknown[], terminal?: unknown) => {
      reserved(name)
      refuseReserved(name)
      return this.#bus.dispatch(mode, name, values, { row: row.key, keep, terminal, gone: this.#rows.get(row.key) !== row && Boolean(row.unloaded) })
    }
    const waterfall = (terminal?: unknown) => (name: string, ...values: unknown[]) => dispatch('waterfall', name, values, terminal)
    return {
      emit: (name: string, ...values: unknown[]) => { dispatch('emit', name, values) },
      parallel: (name: string, ...values: unknown[]) => dispatch('parallel', name, values),
      serial: (name: string, ...values: unknown[]) => dispatch('serial', name, values),
      waterfall: Object.assign(waterfall(), {
        with: (terminal: (...args: any[]) => unknown) => {
          if (typeof terminal !== 'function') throw new TypeError('waterfall.with needs a terminal function')
          return waterfall(terminal)
        },
      }),
      bail: (name: string, ...values: unknown[]) => dispatch('bail', name, values),
    }
  }

  // ── Rows ──────────────────────────────────────────────────────

  async load(key: string, entry: unknown, config: unknown, isolate: Array<[string, string]> | null, exports: Row['exports'] | null, scope?: unknown, id?: unknown, inject?: string[] | null) {
    if (this.#rows.has(key)) throw new Error(`row ${key} is already loaded`)
    // Descriptors and reported lines call the row by its id (protocol §4.2).
    this.#bus.nameRow(key, id)
    const row: Row = { key, entry, config, exports: exports ?? {}, isolate: new Map(), steps: [], chain: chainOf(scope), abort: new AbortController(), provided: [], announced: [], pending: [], inject: inject ?? null }
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
    // What it may `use`: what it or its row injects (as Rust's `require`
    // reads only the injects), and what it provides; the test driver,
    // anything. What it may `get`: that and its plugin's `optional`.
    const uses = new Set([...plugin.inject, ...(inject ?? [])])
    const mayUse = (name: string) => plugin.usesAny || uses.has(name) || row.provided.includes(idIn(row, name))
    const mayGet = (name: string) => mayUse(name) || plugin.optional.includes(name)
    const ctx = {
      use: (name: string) => {
        // Not injected by it or its row (an optional one is read with
        // `get`): say what to declare, not that it is missing (#676),
        // whether or not another row here provides it.
        if (!mayUse(name)) throw notDeclared(name)
        return this.lookup(idIn(row, name), row.key)
      },
      // An optional service: the service when there, else undefined.
      get: (name: string) => {
        if (!mayGet(name)) throw notDeclaredOptional(name)
        return this.#find(idIn(row, name), row.key)
      },
      provide: (name: string, value: unknown) => this.provide(row, name, value),
      effect: (cleanup: () => unknown) => {
        if (typeof cleanup !== 'function') throw new TypeError('effect needs a cleanup function')
        // As Rust's InactiveEffect: once the row is unloading, the stack it
        // would join is the one the unload took, so it would never run.
        if (this.#rows.get(row.key) !== row) {
          process.stderr.write(`rutis: row ${this.#bus.rowId(row.key) ?? row.key} is not active: cleanup is not registered\n`)
          return
        }
        row.steps.push(cleanup)
      },
      // Aborted when the row unloads, while its listeners are waited for.
      get signal() { return row.abort.signal },
      // On the host's bus until the row unloads, or the returned function
      // is called. `prepend` and `global` as Cordis's.
      on: (name: string, callback: Listener, options?: { prepend?: boolean, global?: boolean }) => {
        reserved(name)
        if (this.#rows.get(row.key) !== row) throw new Error(`row ${row.key} is not active`)
        if (typeof callback !== 'function') throw new TypeError('a listener is a function')
        const listener = this.#bus.on({ name, callback, prepend: options?.prepend, global: options?.global, row: row.key, plugin: plugin.name ?? this.#bus.rowId(key), scope: row.chain })
        return () => { this.#bus.off(listener) }
      },
      ...this.#emitters(row),
      filter: (predicate: (descriptor: unknown) => unknown) => {
        if (typeof predicate !== 'function') throw new TypeError('filter needs a predicate function')
        return this.#emitters(row, predicate)
      },
    }
    try {
      // The config as the host checked it against the plugin's schema, its
      // defaults filled (the kernel's `rutis::config::check`, platform
      // cleanup §7.5): the runtime only passes it on.
      const cleanup = await this.within(key, () => plugin.apply(ctx, config))
      if (cleanup !== undefined && cleanup !== null) {
        if (typeof cleanup !== 'function') throw new TypeError('apply must return a cleanup function or nothing')
        row.steps.push(cleanup as () => unknown)
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
    // What the row provides, as the test driver reads it (`provided()`): the
    // host published these before it heard that the row loaded, so they are
    // here at once; its `rutis/service` events cross the session afterwards
    // (#1030).
    if (this.#rows.get(key) === row) followRow(String(this.#bus.rowId(key)), () => [...new Set(row.provided.map(serviceName))].sort())
    return null
  }

  async unload(key: string) {
    const row = this.#rows.get(key)
    if (!row) return null
    this.#rows.delete(key)
    const rowId = this.#bus.rowId(key)
    if (typeof rowId === 'string') forgetRow(rowId)
    // Its listeners are off the bus at once (protocol §7.1). The row itself
    // is still loaded for its unload stack: an emission its cleanups make
    // reaches the other rows until it ends.
    this.#bus.withdraw(key, { emitting: true })
    // From here on no other row sees what this one provides; it still does,
    // until each withdrawal's place on the unload stack comes (as Rust's
    // read_binding_as). The host hears the withdrawals now, before the row
    // is gone from it.
    for (const id of row.provided) {
      const provided = this.#services.get(id)
      if (provided?.row === key) { provided.unloading = true; this.#refresh(id) }
    }
    for (const id of [...Object.keys(row.exports).map(name => idIn(row, name)), ...row.announced]) {
      const slot = this.#slots.get(id)
      if (slot?.row !== key) continue
      this.#slots.delete(id)
      this.#gone(id)
    }
    row.announced = []
    // Its running listeners are signalled and waited for, at most the
    // shutdown deadline; then its unload stack runs.
    row.abort.abort(new DOMException('the row is unloading', 'AbortError'))
    await this.#bus.settle(key, this.#deadline)
    this.#bus.forgetRow(key)
    const errors: unknown[] = []
    try {
      for (const step of row.steps.splice(0).reverse()) {
        try { await this.within(key, step) } catch (error) { errors.push(error) }
      }
    } finally {
      // Its cleanups may emit, or register listeners: both stop here.
      row.unloaded = true
      this.#bus.withdraw(key)
    }
    row.provided = []
    if (errors.length) throw errors[0]
    return null
  }

  // Leaf plugins have no volatile fields: a new config restarts the row.
  async update(key: string, config: unknown) {
    const row = this.#rows.get(key)
    if (!row) throw new Error(`row ${key} is not loaded`)
    const id = this.#bus.rowId(key)
    await this.unload(key)
    return this.load(key, row.entry, config, [...row.isolate], row.exports, row.chain, id, row.inject)
  }

  async #plugin(entry: unknown) {
    if (typeof entry !== 'string') return definition(entry)
    // A definition a test handed over, registered by the test driver: loaded
    // as a row of this process, not imported from a file
    // (docs/design-test-tool-definition-load-2026-11-01.md).
    const registered = registeredDefinition(entry)
    if (registered !== undefined) return definition(registered)
    return this.#modules.described(locate(entry, this.#project), entry)
  }

  // What a plugin definition declares (`plugins.read`), answered at once.
  describeDefinition(value: unknown) {
    const plugin = definition(value)
    return { name: plugin.name, config: plugin.config, inject: plugin.inject, optional: plugin.optional, provides: plugin.provides, announces: plugin.announces, version: null }
  }

  async describe(entry: string) {
    const registered = registeredDefinition(entry)
    if (registered !== undefined) {
      const plugin = definition(registered)
      return { config: plugin.config, inject: plugin.inject, optional: plugin.optional, provides: plugin.provides, announces: plugin.announces, version: null }
    }
    const file = locate(entry, this.#project)
    const plugin = await this.#modules.plugin(file, entry)
    return { config: plugin.config, inject: plugin.inject, optional: plugin.optional, provides: plugin.provides, announces: plugin.announces, version: versionOf(entry, file) }
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
    // Its row unloaded (`gone`): a caller still holding it (an optional
    // service, leased at start) is told it is gone.
    if (entry.gone) throw unavailable(serviceName(entry.name))
    const slot = this.#slots.get(entry.name)
    if (!entry.methods.has(method)) throw noMethod(serviceName(entry.name), method, entry.methods)
    if (!Array.isArray(args)) throw new TypeError('method arguments must be an array')
    const operation = entry.object[method]
    if (typeof operation !== 'function') throw noMethod(serviceName(entry.name), method, entry.methods)
    const row = slot?.row ?? this.#services.get(entry.name)?.row
    const shown = row === undefined ? this.name : this.#bus.rowId(row) ?? row
    return this.within(row, () => this.#blocking.held(shown, `${serviceName(entry.name)}.${method}`, () => Reflect.apply(operation, entry.object, args)))
  }

  #control(method: string, args: any): unknown {
    switch (method) {
      case 'mount':
        // This runtime's name on the host, for descriptors, when it says.
        if (typeof args.runtime === 'string') this.#bus.runtime = args.runtime
        return {
          services: {},
          features: FEATURES,
          implementation: { name: MANIFEST.name, version: MANIFEST.version },
          engine: { name: 'bun', version: Bun.version },
        }
      case 'dispose':
        this.closing = true
        return (async () => { await this.dispose(); await this.client?.drain(); return null })()
      // The logical bus: the host runs a listener here.
      case 'events.call': return this.#bus.call(args[0] ?? {})
      case 'rows.load': {
        const [key, entry, config, isolate, inject, exports, place] = args
        // A module's name, or a plugin definition this runtime handed over.
        return this.load(key, typeof entry === 'string' ? entry : entry ?? String(entry), config, isolate, exports, place?.scope, place?.id, Array.isArray(inject) ? inject : null)
      }
      case 'rows.update': return this.update(args[0], args[1])
      case 'rows.unload': return this.unload(args[0])
      case 'rows.schema': return this.describe(String(args[0]))
      case 'plugins.read': return this.describeDefinition(args[0])
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
        if (entry.gone) throw unavailable(serviceName(entry.name))
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
        return () => { throw noMethod(serviceName(id), property, Object.keys(methods)) }
      },
    })
  }
}
