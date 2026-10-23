// The unified events of rutis 0.13 (#464), as one JavaScript process runs
// them: docs/design-events-protocol-2026-10-18.en.md (section numbers
// below refer to it).
//
// All listeners on a host form one logical bus, whose table the host
// keeps. This process registers its plugins' listeners with the host
// (`events.on` / `events.off`), dispatches an emission itself when every
// listener of the name is here (the fast path, §5.2), and otherwise sends
// it to the host (`events.dispatch`), which calls back the listeners here
// (`events.call`). Both paths give the same results (§5.4): the five modes,
// name-based delivery (a scope is a descriptor, §3), filters (§6), emit's
// error reporting (§5.6) and emit's order (§5.5).
//
// This file is the same in the Node runtime (node/rutis-runtime), the Bun
// runtime (bun/rutis-bun) and the SDK's testing tool (node/rutis), which
// runs it without a host (always the fast path); a test checks the copies
// are equal. It knows nothing of Cordis or of the session: the runtime
// passes in what it needs.
//
// A Cordis application linked to other nodes (node/rutis-runtime/src/
// bridge) is a host of its own: its bus has no host above it, and holds,
// besides its own listeners, remote entries (`onRemote`) for the listeners
// of linked nodes, called through `invoke` with the dispatch's mode (§9).

export const MODES = ['emit', 'parallel', 'serial', 'waterfall', 'bail']

// On the AggregateError of a `parallel` this process dispatched: where each
// of its children failed (`row <row>` / `node <peer>`), in the same order.
export const FAILED = Symbol.for('rutis.events.failed')

// "No result" (§1): undefined, null or false; anything else is a result.
export const isResult = value => value !== undefined && value !== null && value !== false

const isThenable = value => value !== null && (typeof value === 'object' || typeof value === 'function') && typeof value.then === 'function'

// A listener's kind (§4.2): `async` for an async function, `any` for any
// other function (known only from what it returns).
export function kindOf(listener) {
  const constructor = listener?.constructor?.name
  return constructor === 'AsyncFunction' || constructor === 'AsyncGeneratorFunction' ? 'async' : 'any'
}

export function named(name, message, errors) {
  const error = errors ? new AggregateError(errors, message) : new Error(message)
  Object.defineProperty(error, 'name', { value: name, configurable: true, writable: true, enumerable: false })
  return error
}

// §11: rutis/* is emitted by rutis only.
export const RESERVED = name => `rutis/* events are emitted by rutis only: ${name}`
export function refuseReserved(name) {
  if (typeof name === 'string' && name.startsWith('rutis/')) throw named('ReservedEvent', RESERVED(name))
}

// The config fields a 0.13 runtime refuses (§2.4).
export const REMOVED_FIELD = field => `the config field ${field} was removed in rutis 0.13: listeners register themselves; emit with ctx.emit / parallel / serial / waterfall / bail (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)`
export function refuseRemoved(config) {
  for (const field of ['events', 'emits']) {
    if (config !== null && typeof config === 'object' && Object.hasOwn(config, field)) throw new Error(REMOVED_FIELD(field))
  }
}

// §2.2: what a 0.13 runtime says of a host that does not declare events-v2.
export const EVENTS_V2 = 'events-v2'
export const NO_EVENTS_V2 = 'this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later'

// Whether the chain `listener` is a prefix of `emitter`, by id: a
// descriptor helper for filters, not a delivery rule (§3). Kept for
// compatibility with 0.13–0.15; delivery is by name.
export function inScope(listener, emitter) {
  if (listener.length > emitter.length) return false
  return listener.every((scope, index) => scope.id === emitter[index].id)
}

const errorText = error => `${error?.name ?? 'Error'}: ${error?.message ?? String(error)}`

// How many dispatch ids this process remembers having fired
// `internal/dispatch` for (§5.3); the oldest are forgotten.
const SEEN = 4096

// Whether `entry` hears an emission of `name` (a remote entry may be a
// prefix entry, §4.3).
const matches = (entry, name) => entry.prefix ? name.startsWith(entry.name) : entry.name === name

export class Bus {
  // Entries of this process in registration order (§4.3).
  #entries = []
  #byId = new Map()
  #next = 0
  // elsewhere(this process) (§4.5): names and prefixes.
  #names = new Set()
  #prefixes = new Set()
  // Emits sent through the host and not yet answered, by name (§5.5).
  #unanswered = new Map()
  // Dispatch ids `internal/dispatch` was fired for here.
  #seen = new Set()
  // Invocations still running, by row (§7.1).
  #running = new Map()
  // Listeners still inside their synchronous start, by row: a synchronous
  // call there can let this process run the row's unload (a reentrant
  // runtime), which waits for them to come out and be counted.
  #starting = new Map()
  // Who follows the table (`watch`).
  #watchers = new Set()
  // The id each row's configuration gives it, by row key (`nameRow`).
  #rowIds = new Map()
  // The rows unloaded here, by key (`withdraw` to the next `nameRow`): an
  // emission from one is refused (§3).
  #unloaded = new Set()

  // `runtime`: this runtime's name, for descriptors (null when unknown).
  // `peer`: { callAsync, call, callFunctionAsync } to the host, or none
  // (the testing tool: always the fast path). `report(line, details?)`: a failure
  // reported here (§5.6). `dispatched(mode, name, args)`: Cordis's
  // `internal/dispatch` for a dispatch from elsewhere (§5.3). `loading(row)`:
  // whether `row` (null: the mount) is loading, so that the host refusing
  // a listener it registers fails the load instead of being reported (§11).
  constructor({ runtime = null, peer = null, report = line => process.stderr.write(`${line}\n`), dispatched, loading = () => false } = {}) {
    this.runtime = runtime
    this.peer = peer
    this.report = report
    this.dispatched = dispatched
    this.loading = loading
  }

  // ── Rows ──────────────────────────────────────────────────────

  // Row `key` is the row `id` of the configuration (`rows.load`'s `id`):
  // descriptors and reported lines call it so (§4.2). Keys are unique in
  // the process; ids need not be.
  // It also marks the row loaded: it may emit until `withdraw`.
  nameRow(key, id) {
    if (key === null || key === undefined) return
    if (typeof id === 'string') this.#rowIds.set(key, id)
    this.#unloaded.delete(key)
  }

  forgetRow(key) { this.#rowIds.delete(key) }

  // Refuse an emission from `row` once it is not loaded here (a plugin
  // keeping its context past its unload), as the host would (§3).
  // `gone`: the emitting context is of an unloaded row whose key a new row
  // may have taken since.
  #refuseUnloaded(row, gone) {
    if (row !== null && row !== undefined && (gone || this.#unloaded.has(row))) throw named('BindingError', `row ${row} is not loaded in this runtime`)
  }

  // What descriptors and lines call row `key`: its id, or the key itself
  // when it was not named; null for none.
  rowId(key) { return key === null || key === undefined ? null : this.#rowIds.get(key) ?? key }

  // ── Registration (§4) ─────────────────────────────────────────

  // Register a listener; returns its entry. `owner` is what a Cordis filter
  // sees for it (its Context); `row` null for a plugin loaded by `mount`.
  on({ name, callback, kind = kindOf(callback), prepend = false, global = false, row = null, plugin = 'plugin', scope = [], owner }) {
    if (typeof name !== 'string') throw new TypeError(`an event name is a string, not ${name === null ? 'null' : typeof name}`)
    if (typeof callback !== 'function') throw new TypeError('a listener is a function')
    const entry = { id: String(++this.#next), name, callback, kind, prepend: Boolean(prepend), global: Boolean(global), row, plugin, scope, owner, live: true, registered: undefined }
    if (entry.prepend) this.#entries.unshift(entry); else this.#entries.push(entry)
    this.#byId.set(entry.id, entry)
    this.#changed({ added: entry })
    if (this.peer) {
      // Registered while its row loads, a refusal fails the load, which
      // waits for it (`registrations`); later, it is reported (§11).
      const loading = Boolean(this.loading(row))
      entry.registered = this.peer.callAsync('', 'events.on', [{ id: entry.id, name, row, plugin, kind, prepend: entry.prepend, global: entry.global }])
      entry.registered.catch(error => {
        if (!loading) this.report(`rutis: registration of listener of ${name} in row ${this.#where(entry)} failed: ${errorText(error)}`)
      })
    }
    return entry
  }

  // A listener's disposer: it leaves the table, here and on the host.
  off(entry) {
    if (!this.#remove(entry)) return false
    if (this.peer) this.peer.callAsync('', 'events.off', [[entry.id]]).catch(() => {})
    return true
  }

  #remove(entry) {
    if (!entry.live) return false
    entry.live = false
    this.#byId.delete(entry.id)
    const index = this.#entries.indexOf(entry)
    if (index >= 0) this.#entries.splice(index, 1)
    this.#changed({ removed: entry })
    return true
  }

  // A remote entry (§9): a linked node's listener of `name` (a prefix when
  // `prefix`), called with `invoke(mode, name, args)` (in a waterfall the
  // last argument is `next`). Withdrawn with `off`.
  onRemote({ name, prefix = false, kind = 'any', global = false, plugin = 'plugin', row = null, runtime = null, node = null, scope = [], invoke }) {
    const entry = { id: String(++this.#next), name, prefix: Boolean(prefix), kind, prepend: false, global: Boolean(global), row, plugin, runtime, node, scope, remote: true, invoke, live: true }
    this.#entries.push(entry)
    this.#byId.set(entry.id, entry)
    this.#changed({ added: entry })
    return entry
  }

  // Follow the table: `watcher({ added })` / `watcher({ removed })` for each
  // entry from now on. Returns the unsubscription.
  watch(watcher) {
    this.#watchers.add(watcher)
    return () => this.#watchers.delete(watcher)
  }

  // The live entries, in registration order.
  entries() {
    return [...this.#entries]
  }

  #changed(change) {
    for (const watcher of this.#watchers) {
      try { watcher(change) } catch (error) { this.report(`rutis: event table watcher failed: ${errorText(error)}`) }
    }
  }

  // The registrations of `row`'s listeners still unanswered (§4.4 2).
  registrations(row) {
    return this.#entries.filter(entry => entry.row === row && entry.registered).map(entry => entry.registered)
  }

  // Withdraw `row`'s listeners at once (§7.1 1–2): the host withdrew them
  // itself; calls for them fail as withdrawn from now on, and the row may
  // not emit any more. `restart`: the row restarts with a new config
  // (`rows.update`), so it stays loaded, and the host, which keeps the
  // row, is told (`events.off`). A failure of an invocation the row's
  // unload stops (its signal's AbortError) is not reported (§5.6).
  withdraw(row, { restart = false } = {}) {
    const ids = []
    for (const entry of [...this.#entries]) {
      if (entry.row !== row || !this.#remove(entry)) continue
      entry.stopped = true
      ids.push(entry.id)
    }
    if (restart) {
      if (ids.length && this.peer) this.peer.callAsync('', 'events.off', [ids]).catch(() => {})
    } else if (row !== null && row !== undefined) this.#unloaded.add(row)
  }

  // Wait for `row`'s running listeners, at most `deadline` ms (§7.1 4).
  async settle(row, deadline) {
    if (!this.#running.get(row)?.size && !this.#starting.has(row)) return true
    let timer
    const late = new Promise(resolve => { timer = setTimeout(() => resolve(false), deadline) })
    const done = (async () => {
      const start = this.#starting.get(row)
      if (start) await (start.out ??= new Promise(resolve => { start.resolve = resolve }))
      const running = this.#running.get(row)
      while (running?.size) await Promise.allSettled([...running])
      return true
    })()
    const finished = await Promise.race([done, late])
    clearTimeout(timer)
    if (!finished) {
      this.report(`rutis: listeners of row ${this.rowId(row)} still running after ${deadline / 1000}s; unloading anyway`)
      this.#running.delete(row)
    }
    return finished
  }

  // An entry's descriptor (§4.2): a copy, so a filter changing it changes
  // nothing else.
  describe(entry) {
    const scope = entry.scope.map(item => ({ ...item }))
    if (entry.remote) return { name: entry.name, prefix: entry.prefix, plugin: entry.plugin, row: entry.row, runtime: entry.runtime, node: entry.node, scope, global: entry.global }
    return { name: entry.name, prefix: false, plugin: entry.plugin, row: this.rowId(entry.row), runtime: this.runtime, node: null, scope, global: entry.global }
  }

  #where(entry) { return (entry.remote ? entry.row : this.rowId(entry.row)) ?? entry.plugin }

  // Where the lines say an emission comes from, as the host says it.
  #from(row) { return row === null || row === undefined ? 'the mount' : `row ${this.rowId(row)}` }

  // ── elsewhere (§4.5) ──────────────────────────────────────────

  elsewhere({ add = [], remove = [], addPrefixes = [], removePrefixes = [] } = {}) {
    for (const name of add) this.#names.add(name)
    for (const name of remove) this.#names.delete(name)
    for (const prefix of addPrefixes) this.#prefixes.add(prefix)
    for (const prefix of removePrefixes) this.#prefixes.delete(prefix)
  }

  // The session ended: nothing is elsewhere any more.
  ended() {
    this.peer = null
    this.#names.clear()
    this.#prefixes.clear()
    this.#unanswered.clear()
  }

  #isElsewhere(name) {
    if (this.#names.has(name)) return true
    for (const prefix of this.#prefixes) if (name.startsWith(prefix)) return true
    return false
  }

  // Whether an emission of `name` in `mode` runs here (§5.2).
  local(mode, name) {
    if (!this.peer) return true
    if (this.#isElsewhere(name)) return false
    return !(mode === 'emit' && this.#unanswered.get(name))
  }

  // ── Dispatch (§5) ─────────────────────────────────────────────

  // Emit `name` with `args` in `mode`, from `row` (null: a plugin loaded by
  // `mount`). `keep(descriptor, entry?)`: the filter (§6), `entry` given for
  // a listener of this process. `thisArg`: what the listeners here are called
  // with (Cordis's). `terminal`: waterfall's innermost function, or none.
  // Throws ReservedEvent for rutis/* (§8).
  dispatch(mode, name, args, { row = null, keep, thisArg = null, terminal, gone = false } = {}) {
    if (!MODES.includes(mode)) throw new TypeError(`unknown dispatch mode ${mode}`)
    if (typeof name !== 'string') throw new TypeError(`an event name is a string, not ${name === null ? 'null' : typeof name}`)
    refuseReserved(name)
    this.#refuseUnloaded(row, gone)
    if (terminal !== undefined && terminal !== null && typeof terminal !== 'function') {
      throw new TypeError(`a waterfall's terminal is a function, not ${typeof terminal}`)
    }
    if (!this.local(mode, name)) return this.#viaHost(mode, name, args, { row, keep, terminal })
    let snapshot = this.#entries.filter(entry => matches(entry, name))
    if (keep) {
      try {
        snapshot = snapshot.filter(entry => entry.global || this.#keeps(keep, this.describe(entry), entry))
      } catch (error) {
        if (mode !== 'emit') {
          if (mode === 'bail') throw error
          return Promise.reject(error)
        }
        this.report(`rutis: filter of ${name} from ${this.#from(row)} failed: ${errorText(error)}`, { event: name, row, filter: true, error })
        return undefined
      }
    }
    switch (mode) {
      case 'emit': {
        const run = () => { for (const entry of snapshot) if (entry.live) this.#emitTo(entry, args, thisArg, name) }
        // Behind the host's calls of an earlier emit of the name that this
        // process has not run yet (a synchronous call of it waits): they
        // start first, in frame order (§5.5).
        if (!this.peer?.behind?.(name, run)) run()
        return undefined
      }
      case 'parallel': return this.#parallel(name, snapshot, args, thisArg)
      case 'serial': return this.#serial(name, snapshot, args, thisArg)
      case 'waterfall': return this.#waterfall(name, snapshot, args, thisArg, terminal)
      case 'bail': return this.#bail(name, snapshot, args, thisArg)
    }
  }

  // The filter on one listener: `descriptor`, and `entry` for one of this
  // process. A Cordis filter's `rutis` of the entry's Context is the
  // descriptor meanwhile (`filtering`).
  #keeps(keep, descriptor, entry) {
    const outer = this.filtering
    this.filtering = entry ? { owner: entry.owner, descriptor } : undefined
    try { return Boolean(keep(descriptor, entry)) } finally { this.filtering = outer }
  }

  // Run one listener here, counted as running for its row until it settles;
  // a remote entry's call goes to its node, in `mode`, for `name`.
  #invoke(entry, args, thisArg, mode, name) {
    let start = this.#starting.get(entry.row)
    if (!start) this.#starting.set(entry.row, start = { count: 0, out: null, resolve: null })
    start.count++
    try {
      const result = entry.remote ? entry.invoke(mode, name ?? entry.name, args) : Reflect.apply(entry.callback, thisArg, args)
      if (isThenable(result)) {
        let running = this.#running.get(entry.row)
        if (!running) this.#running.set(entry.row, running = new Set())
        const settled = Promise.resolve(result).then(() => {}, () => {}).then(() => { running.delete(settled) })
        running.add(settled)
      }
      return result
    } finally {
      if (--start.count === 0) {
        this.#starting.delete(entry.row)
        start.resolve?.()
      }
    }
  }

  // emit: the listener's failure is reported here and goes no further
  // (§5.6), unless its row's unload stopped it.
  #emitTo(entry, args, thisArg, name) {
    const where = entry.remote ? `node ${entry.node}` : `row ${this.#where(entry)}`
    const failed = error => {
      if (entry.stopped && error?.name === 'AbortError') return
      this.report(`rutis: listener of ${name ?? entry.name} in ${where} failed: ${errorText(error)}`, { event: entry.name, row: entry.row, plugin: entry.plugin, error })
    }
    let result
    try { result = this.#invoke(entry, args, thisArg, 'emit', name) } catch (error) { failed(error); return undefined }
    if (isThenable(result)) return Promise.resolve(result).then(() => undefined, error => { failed(error) })
    return undefined
  }

  async #parallel(name, snapshot, args, thisArg) {
    const started = []
    const entries = []
    for (const entry of snapshot) {
      if (!entry.live) continue
      entries.push(entry)
      started.push(new Promise(resolve => resolve(this.#invoke(entry, args, thisArg, 'parallel', name))))
    }
    const settled = await Promise.allSettled(started)
    const failed = settled.flatMap((result, index) => result.status === 'rejected' ? [[entries[index], result.reason]] : [])
    if (!failed.length) return
    const error = named('AggregateError', `${failed.length} listeners of ${name} failed`, failed.map(([, reason]) => reason))
    // Where each failed, for a caller that reports them (FAILED).
    Object.defineProperty(error, FAILED, { value: failed.map(([entry]) => entry.remote ? `node ${entry.node}` : `row ${this.#where(entry)}`) })
    throw error
  }

  async #serial(name, snapshot, args, thisArg) {
    for (const entry of snapshot) {
      if (!entry.live) continue
      const result = await this.#invoke(entry, args, thisArg, 'serial', name)
      if (isResult(result)) return result
    }
    return undefined
  }

  #waterfall(name, snapshot, args, thisArg, terminal) {
    let ended = false
    const from = index => {
      while (index < snapshot.length && !snapshot[index].live) index++
      if (index >= snapshot.length) return terminal ? terminal(...args) : undefined
      let called = false
      // A failing call gives a rejected Promise, as a `next` through the
      // host does (waterfall is asynchronous).
      const next = () => {
        if (ended) return Promise.reject(named('WaterfallNext', `next of ${name} was called after its waterfall ended`))
        if (called) return Promise.reject(named('WaterfallNext', `next of ${name} was already called`))
        called = true
        return from(index + 1)
      }
      return this.#invoke(snapshot[index], [...args, next], thisArg, 'waterfall', name)
    }
    let result
    try { result = from(0) } catch (error) { ended = true; throw error }
    if (!isThenable(result)) { ended = true; return result }
    return Promise.resolve(result).finally(() => { ended = true })
  }

  #bail(name, snapshot, args, thisArg) {
    for (const entry of snapshot) {
      if (!entry.live) continue
      if (entry.kind === 'async') throw this.#asynchronous(entry, 'bail')
      const result = this.#invoke(entry, args, thisArg, 'bail', name)
      if (isThenable(result)) {
        Promise.resolve(result).catch(() => {})
        throw this.#asynchronous(entry, 'bail')
      }
      if (isResult(result)) return result
    }
    return undefined
  }

  #asynchronous(entry, mode) {
    return named('AsyncListener', `listener of ${entry.name} in row ${this.#where(entry)} is asynchronous and cannot run in a synchronous ${mode}`)
  }

  // Through the host (§5.3): it dispatches on its whole table; listeners of
  // this process are called back with `events.call`.
  #viaHost(mode, name, args, { row, keep, terminal }) {
    // A record (the session's mark): its functions cross as references.
    const fields = { row, name, mode, args, [Symbol.for('rutis.session.record')]: true }
    if (keep) {
      // The selector (§6): one call per dispatch, with the descriptors of
      // the non-global candidates, in registration order. The host marks
      // those of this process's own listeners with their id (`listener`),
      // by which the filter gets their entry (a Cordis Context); the
      // filter sees the descriptor without it.
      fields.filter = descriptors => {
        if (!Array.isArray(descriptors)) throw new TypeError('a filter selector takes a list of descriptors')
        return descriptors.map(described => {
          const { listener, ...descriptor } = described ?? {}
          const entry = listener === undefined ? undefined : this.#byId.get(String(listener))
          return this.#keeps(keep, descriptor, entry)
        })
      }
    }
    if (mode === 'waterfall' && terminal) fields.terminal = (...values) => terminal(...values)
    const peer = this.peer
    if (mode === 'bail') return peer.call('', 'events.dispatch', [fields])
    if (mode !== 'emit') return peer.callAsync('', 'events.dispatch', [fields])
    // emit: later emits of this name follow this one through the host until
    // it is answered (§5.5); the API has returned already.
    this.#unanswered.set(name, (this.#unanswered.get(name) ?? 0) + 1)
    const answered = () => {
      const count = this.#unanswered.get(name) - 1
      if (count > 0) this.#unanswered.set(name, count); else this.#unanswered.delete(name)
    }
    const failed = error => {
      answered()
      this.report(`rutis: emit of ${name} from ${this.#from(row)} failed: ${errorText(error)}`)
    }
    try {
      if (peer.callAnswered) {
        // The reply is taken as its frame is read, before the next one: an
        // emit a later frame starts sees it answered (§5.5).
        peer.callAnswered('', 'events.dispatch', [fields], ({ ok, value }) => {
          if (!ok) failed(value)
          else if (typeof value?.then === 'function') value.then(answered, failed)
          else answered()
        })
      } else {
        peer.callAsync('', 'events.dispatch', [fields]).then(answered, failed)
      }
    } catch (error) { failed(error) }
    return undefined
  }

  // ── events.call (§5.3) ────────────────────────────────────────

  // Run one listener of this process for the host. Returns its result (a
  // Promise for an asynchronous one), or throws its error.
  call({ listener, dispatch, own, name, mode, args, next } = {}) {
    const entry = this.#byId.get(String(listener))
    if (!entry?.live) throw named('BindingError', `listener ${listener} of ${name ?? entry?.name} is withdrawn`)
    const values = Array.isArray(args) ? args : []
    // Cordis's internal/dispatch, once per dispatch from elsewhere in this
    // process, before its first listener here runs.
    if (!own && this.dispatched && !this.#seen.has(dispatch)) {
      this.#seen.add(dispatch)
      if (this.#seen.size > SEEN) this.#seen.delete(this.#seen.values().next().value)
      this.dispatched(mode === 'waterfall-sync' ? 'waterfall' : mode, entry.name, values)
    }
    switch (mode) {
      case 'emit': return this.#emitTo(entry, values, null)
      case 'parallel': case 'serial': return this.#invoke(entry, values, null)
      case 'waterfall': {
        let called = false
        const onward = () => {
          if (called) return Promise.reject(named('WaterfallNext', `next of ${entry.name} was already called`))
          called = true
          // A linked node's `next`, to a bus with no host above it (§9).
          if (!this.peer && typeof next === 'function') return next()
          return this.peer ? this.peer.callFunctionAsync(next, []) : Promise.reject(named('BindingError', 'the session ended'))
        }
        return this.#invoke(entry, [...values, onward], null)
      }
      case 'bail': case 'waterfall-sync': {
        if (entry.kind === 'async') throw this.#asynchronous(entry, mode)
        let called = false
        const onward = () => {
          if (called) throw named('WaterfallNext', `next of ${entry.name} was already called`)
          called = true
          return next()
        }
        const result = this.#invoke(entry, mode === 'bail' ? values : [...values, onward], null)
        if (isThenable(result)) {
          Promise.resolve(result).catch(() => {})
          throw this.#asynchronous(entry, mode)
        }
        return result
      }
      default: throw named('BindingError', `unknown dispatch mode ${mode}`)
    }
  }
}
