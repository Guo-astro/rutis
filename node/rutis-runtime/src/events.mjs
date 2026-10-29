// The unified events of rutis 0.13 (#464), as one JavaScript runtime runs
// them: docs/design-events-protocol-2026-10-18.en.md (section numbers
// below refer to it).
//
// All listeners on a host form one logical bus, whose table the host
// keeps. This process registers its plugins' listeners with the host
// (`events.on` / `events.off`) and sends every emission to the host
// (`events.dispatch`), which dispatches it on its whole table and calls
// back the listeners here (`events.call`): the five modes, name-based
// delivery (a scope is a descriptor, §3), filters (§6), emit's error
// reporting (§5.6) and emit's order (§5.5) are the host's (platform
// cleanup §7.5: events are dispatched in the kernel only).
//
// This file is the same in the Node runtime (node/rutis-runtime) and the
// Bun runtime (bun/rutis-bun); a test checks the copies are equal. It
// knows nothing of Cordis or of the session: the runtime passes in what it
// needs.

export const MODES = ['emit', 'parallel', 'serial', 'waterfall', 'bail']

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

const errorText = error => `${error?.name ?? 'Error'}: ${error?.message ?? String(error)}`

// How many dispatch ids this process remembers having fired
// `internal/dispatch` for (§5.3); the oldest are forgotten.
const SEEN = 4096

// The session with the host ended: nothing more is dispatched.
const hostGone = () => Object.assign(named('BindingError', 'the host disconnected'), { code: 'peer-gone', data: {} })

export class Bus {
  // Entries of this process in registration order (§4.3).
  #entries = []
  #byId = new Map()
  #next = 0
  // Dispatch ids `internal/dispatch` was fired for here.
  #seen = new Set()
  // Invocations still running, by row (§7.1).
  #running = new Map()
  // Listeners still inside their synchronous start, by row: a synchronous
  // call there can let this process run the row's unload (a reentrant
  // runtime), which waits for them to come out and be counted.
  #starting = new Map()
  // The id each row's configuration gives it, by row key (`nameRow`).
  #rowIds = new Map()
  // The rows unloaded here, by key (`withdraw` to the next `nameRow`): an
  // emission from one is refused (§3).
  #unloaded = new Set()

  // `runtime`: this runtime's name, for descriptors (null when unknown).
  // `peer`: { callAsync, call, callAnswered?, callFunctionAsync } to the
  // host; null once the session ended. `report(line, details?)`: a failure
  // reported here (§5.6). `dispatched(mode, name, args)`: Cordis's
  // `internal/dispatch` for a dispatch from elsewhere (§5.3). `loading(row)`:
  // whether `row` (null: the mount) is loading, so that the host refusing
  // a listener it registers fails the load instead of being reported (§11).
  // `within(row, run)`: runs a listener of `row` as that row's code (the
  // runtime traces which row an unhandled error comes from, #688).
  constructor({ runtime = null, peer = null, report = line => process.stderr.write(`${line}\n`), dispatched, loading = () => false, within = (_row, run) => run() } = {}) {
    this.runtime = runtime
    this.peer = peer
    this.report = report
    this.dispatched = dispatched
    this.loading = loading
    this.within = within
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
    return true
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
    return { name: entry.name, prefix: false, plugin: entry.plugin, row: this.rowId(entry.row), runtime: this.runtime, node: null, scope, global: entry.global }
  }

  #where(entry) { return this.rowId(entry.row) ?? entry.plugin }

  // Where the lines say an emission comes from, as the host says it.
  #from(row) { return row === null || row === undefined ? 'the mount' : `row ${this.rowId(row)}` }

  // The session with the host ended: nothing more is dispatched.
  ended() {
    this.peer = null
  }

  // ── Dispatch (§5) ─────────────────────────────────────────────

  // Emit `name` with `args` in `mode`, from `row` (null: a plugin loaded by
  // `mount`), through the host (§5.3). `keep(descriptor, entry?)`: the
  // filter (§6), `entry` given for a listener of this process.
  // `terminal`: waterfall's innermost function, or none. Throws
  // ReservedEvent for rutis/* (§8).
  // `sync`: a Cordis `waterfall`, which returns its value at once: the
  // host dispatches it synchronously (§5.4).
  dispatch(mode, name, args, { row = null, keep, terminal, gone = false, sync = false } = {}) {
    if (!MODES.includes(mode)) throw new TypeError(`unknown dispatch mode ${mode}`)
    if (typeof name !== 'string') throw new TypeError(`an event name is a string, not ${name === null ? 'null' : typeof name}`)
    refuseReserved(name)
    this.#refuseUnloaded(row, gone)
    if (terminal !== undefined && terminal !== null && typeof terminal !== 'function') {
      throw new TypeError(`a waterfall's terminal is a function, not ${typeof terminal}`)
    }
    return this.#viaHost(mode, name, args, { row, keep, terminal, sync })
  }

  // The filter on one listener: `descriptor`, and `entry` for one of this
  // process. A Cordis filter's `rutis` of the entry's Context is the
  // descriptor meanwhile (`filtering`).
  #keeps(keep, descriptor, entry) {
    const outer = this.filtering
    this.filtering = entry ? { owner: entry.owner, descriptor } : undefined
    try { return Boolean(keep(descriptor, entry)) } finally { this.filtering = outer }
  }

  // Run one listener here, counted as running for its row until it settles.
  #invoke(entry, args, thisArg) {
    let start = this.#starting.get(entry.row)
    if (!start) this.#starting.set(entry.row, start = { count: 0, out: null, resolve: null })
    start.count++
    try {
      const result = this.within(entry.row, () => Reflect.apply(entry.callback, thisArg, args))
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

  #asynchronous(entry, mode) {
    return named('AsyncListener', `listener of ${entry.name} in row ${this.#where(entry)} is asynchronous and cannot run in a synchronous ${mode}`)
  }

  // Through the host (§5.3): it dispatches on its whole table; listeners of
  // this process are called back with `events.call`.
  #viaHost(mode, name, args, { row, keep, terminal, sync }) {
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
    const synchronous = mode === 'bail' || (mode === 'waterfall' && sync)
    if (mode === 'waterfall' && sync) fields.sync = true
    const peer = this.peer
    if (!peer) {
      if (synchronous) throw hostGone()
      if (mode !== 'emit') return Promise.reject(hostGone())
      this.report(`rutis: emit of ${name} from ${this.#from(row)} failed: ${errorText(hostGone())}`)
      return undefined
    }
    if (synchronous) return peer.call('', 'events.dispatch', [fields])
    if (mode !== 'emit') return peer.callAsync('', 'events.dispatch', [fields])
    // emit: the API has returned already; a failure is reported here.
    const failed = error => { this.report(`rutis: emit of ${name} from ${this.#from(row)} failed: ${errorText(error)}`) }
    try {
      if (peer.callAnswered) {
        peer.callAnswered('', 'events.dispatch', [fields], ({ ok, value }) => {
          if (!ok) failed(value)
          else if (typeof value?.then === 'function') value.then(() => {}, failed)
        })
      } else {
        peer.callAsync('', 'events.dispatch', [fields]).catch(failed)
      }
    } catch (error) { failed(error) }
    return undefined
  }

  // ── events.call (§5.3) ────────────────────────────────────────

  // Run one listener of this process for the host. Returns its result (a
  // Promise for an asynchronous one), or throws its error.
  // `sync`: a synchronous waterfall's call (a Cordis `waterfall`): the
  // listener's `next` returns the rest of the chain's value at once.
  call({ listener, dispatch, own, name, mode, args, next, sync = false } = {}) {
    const entry = this.#byId.get(String(listener))
    if (!entry?.live) throw named('BindingError', `listener ${listener} of ${name ?? entry?.name} is withdrawn`)
    const values = Array.isArray(args) ? args : []
    // Cordis's internal/dispatch, once per dispatch from elsewhere in this
    // process, before its first listener here runs.
    if (!own && this.dispatched && !this.#seen.has(dispatch)) {
      this.#seen.add(dispatch)
      if (this.#seen.size > SEEN) this.#seen.delete(this.#seen.values().next().value)
      this.dispatched(mode, entry.name, values)
    }
    switch (mode) {
      case 'emit': {
        // A failure goes back to the host, which reports it (§5.6); one its
        // row's unload stopped (its signal's AbortError) does not.
        const stopped = error => {
          if (entry.stopped && error?.name === 'AbortError') return undefined
          throw error
        }
        let result
        try { result = this.#invoke(entry, values, null) } catch (error) { return stopped(error) }
        return isThenable(result) ? Promise.resolve(result).then(() => undefined, stopped) : undefined
      }
      case 'parallel': case 'serial': return this.#invoke(entry, values, null)
      case 'waterfall': {
        let called = false
        const onward = () => {
          if (called) return Promise.reject(named('WaterfallNext', `next of ${entry.name} was already called`))
          called = true
          if (!this.peer) return Promise.reject(hostGone())
          // Calling the function itself waits for it synchronously.
          return sync ? next() : this.peer.callFunctionAsync(next, [])
        }
        return this.#invoke(entry, [...values, onward], null)
      }
      case 'bail': {
        if (entry.kind === 'async') throw this.#asynchronous(entry, mode)
        const result = this.#invoke(entry, values, null)
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
