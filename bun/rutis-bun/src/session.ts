import { AsyncLocalStorage } from 'node:async_hooks'
import { readFileSync } from 'node:fs'
import { encode } from './codec.ts'
import { encodeError, decodeError } from './errors.ts'
import { VALUE_CAPABILITIES, ValueError, base64Decode, base64Encode, bytesOf, checkKeys, checkText, constructedTag, describe, instantFromText, instantText, keyOf, tagOf, undeclared } from './values.ts'

// Values with behaviour cross by reference, so their identity and state stay
// with the owner: objects with methods and class instances become object
// references. Built-in value types are not objects of this kind, nor are
// the values the SDK constructors make (`rutis.date(…)`, ...).
const builtins = [Date, RegExp, Map, Set, WeakMap, WeakSet, ArrayBuffer, DataView, Error, Promise]
// A plain object marked as a record crosses field by field even when it
// holds functions: a protocol message such as `events.dispatch` (#464).
export const RECORD = Symbol.for('rutis.session.record')
// A leaf plugin (`definePlugin`, the symbol the SDK marks it with) crosses
// as a plugin definition: a `plugin` reference, sent only to a far end that
// declared `plugins` (docs/design-plugin-values-2026-10-25.md §5).
const LEAF_PLUGIN = Symbol.for('rutis.leaf-plugin')
const isPluginDefinition = (value: any) => value !== null && typeof value === 'object' && value[LEAF_PLUGIN] === true
function isLive(value) {
  if (value === null || typeof value !== 'object' || Array.isArray(value) || ArrayBuffer.isView(value)) return false
  if (builtins.some(type => value instanceof type) || constructedTag(value) || value[RECORD] === true) return false
  const prototype = Object.getPrototypeOf(value)
  if (prototype !== Object.prototype && prototype !== null) return true
  return Object.values(value).some(item => typeof item === 'function')
}
// Whether a value holds a reference or a tagged value (a Date, a Map, ...)
// anywhere: itself, or inside the arrays and plain objects it contains.
// Such plain objects are encoded field by field.
function needsRecord(value, seen = new Set()) {
  if (typeof value === 'function' || value instanceof Promise || isLive(value) || tagOf(value)) return true
  if (value === null || typeof value !== 'object' || seen.has(value)) return false
  if (!Array.isArray(value) && ![Object.prototype, null].includes(Object.getPrototypeOf(value))) return false
  seen.add(value)
  return Object.values(value).some(item => needsRecord(item, seen))
}

// The protocol version this runtime speaks (package.json `rutisProtocol`).
export const MANIFEST = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'))
const PROTOCOL = MANIFEST.rutisProtocol
// The endpoint format: endpoint ids in the handshake and the call ids,
// capabilities, either side calling the other.
export const ENDPOINT_PROTOCOL = 3

// Why a far end's protocol version is refused, saying which end to upgrade
// (standard Q6.2.4): this side when the far end is newer, naming the
// package to upgrade here; the far end when it is older, naming its
// implementation when its handshake said.
export function versionMismatch(far, local, implementation) {
  const mismatch = `the far end speaks protocol ${far}, this side ${local}`
  if (typeof far !== 'number' || far === local) return mismatch
  if (far > local) return `${mismatch}: upgrade this side (npm ${MANIFEST.name})`
  const { name, version } = implementation ?? {}
  return typeof name === 'string' && typeof version === 'string'
    ? `${mismatch}: upgrade the far end (${name} ${version})`
    : `${mismatch}: upgrade the far end`
}
// What this implementation supports in the endpoint format. It sends
// objects but cannot receive them, so it does not declare `objects`.
// `reentrant-sync`: while a synchronous call waits, every incoming call
// runs, not only those of its own call chain (see `#run`). `sync-wait`:
// its synchronous calls say so (`sync: true`), for the host's wait-cycle
// check; `sync-stack`: calls run while it waits are stacked on its one
// thread. `value-<tag>`: it takes all ten tagged values (#308,
// ./values.ts).
export const CAPABILITIES = ['signals', 'reentrant-sync', 'sync-wait', 'sync-stack', ...VALUE_CAPABILITIES, 'events-v2']
// What it declares in the compat format, where older hosts ignore it.
const COMPAT_CAPABILITIES = ['reentrant-sync', 'sync-wait', 'sync-stack', ...VALUE_CAPABILITIES, 'events-v2']
export const ENDPOINT_ID = /^[a-z0-9-]+$/
// `events-v2` (#464): the event protocol of rutis 0.13, declared in both
// formats. A runtime requires it of its host (`requires`) and ends the
// session right after the handshake without it (protocol §2.1).
export const EVENTS_V2_REFUSAL = 'this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later'
function required(capabilities, requires) {
  if (requires?.includes('events-v2') && !capabilities.includes('events-v2')) throw Object.assign(new Error(EVENTS_V2_REFUSAL), { category: 'incompatible' })
}

// What a far end's endpoint-format hello says of it, or why it is refused:
// an error whose `category` says what it means for a link or a dialer
// (incompatible: stop; auth-rejected: retry slowly). `endpoint` is this
// side's ({ local, expected?, verified? }).
export function greetingOf(frame, endpoint) {
  const fail = (category, message) => Object.assign(new Error(message), { category })
  if (frame.version !== ENDPOINT_PROTOCOL) throw fail('incompatible', `incompatible session: ${versionMismatch(frame.version, ENDPOINT_PROTOCOL, frame.implementation)}`)
  const far = frame.endpoint
  if (typeof far !== 'string' || !ENDPOINT_ID.test(far)) throw fail('incompatible', 'incompatible session: the far end named no valid endpoint')
  for (const [whose, expected] of [['verified', endpoint.verified], ['expected', endpoint.expected]]) {
    if (expected !== undefined && expected !== far) throw fail('auth-rejected', `endpoint mismatch: the far end greeted as ${far}, but ${expected} is the ${whose} endpoint`)
  }
  if (far === endpoint.local) throw fail('auth-rejected', `endpoint mismatch: the far end greeted as this endpoint (${far})`)
  const capabilities = Array.isArray(frame.capabilities) ? frame.capabilities : []
  required(capabilities, endpoint.requires)
  return { endpoint: far, implementation: frame.implementation, capabilities }
}

// How deep a value crossing a session may nest (#279): each array or
// object is a level, so 64 nested arrays cross and 65 do not; a call's
// argument list is not a level. A deeper value fails its own call here,
// before it is sent; the session goes on.
export const MAX_DEPTH = 64
const tooDeep = () => new RangeError(`value nested deeper than ${MAX_DEPTH} levels, the most a value crossing a session may nest`)

// The largest message either way, as the channels take it: 16 MiB. A frame
// over it fails its own call here, before it is sent; the channel would
// end on it (#675).
export const MAX_MESSAGE = 16 * 1024 * 1024
const MIB = 1024 * 1024
export const overLimit = (size, limit) => `the message would be ${size} bytes, over the limit of ${limit} bytes${limit % MIB === 0 ? ` (${limit / MIB} MiB)` : ''}`
// A frame's size in UTF-8, counted only when it may be over `limit`: a
// UTF-16 code unit takes at most 3 bytes.
const sizeOver = (text, limit) => text.length * 3 <= limit ? 0 : Buffer.byteLength(text)
// What a call is, for messages: `service.method` for an invocation (its
// target without a `host:` prefix or a scope label), the method alone for a
// control call, `a function` for a call of a function reference.
export function describeCall(op, { target, method, property } = {}) {
  if (op === 'invoke') {
    const service = String(target).replace(/^host:/, '').split('\0')[0]
    return service ? `${service}.${method}` : method
  }
  if (op === 'call') return method === undefined ? 'a function' : `the method ${method}`
  if (op === 'get') return `the property ${property}`
  return 'an awaited result'
}
// `error`, thrown encoding argument `index` (or the result) of `call`, as a
// new error saying which, the thrown one its cause. Never changed in place:
// a getter run while encoding may throw one error many times, or a frozen
// one.
const ofArgument = (error, index, call) => prefixed(error, `argument ${index + 1} of ${call}`)
const ofResult = (error, call) => prefixed(error, call ? `the result of ${call}` : 'the result')
// The classes a value error of this side is: it keeps its class, any other
// thrown value is wrapped in an Error.
const VALUE_ERRORS = [ValueError, RangeError, TypeError]
function prefixed(error, what) {
  let reason
  try { reason = error instanceof Error ? error.message : String(error) } catch { reason = 'a value that cannot be shown was thrown' }
  const Class = VALUE_ERRORS.find(type => error?.constructor === type) ?? Error
  return new Class(`${what}: ${reason}`, { cause: error })
}
// A class instance's class, for messages: none for a plain object.
const className = value => {
  const prototype = Object.getPrototypeOf(value)
  const name = prototype && prototype !== Object.prototype ? prototype.constructor?.name : undefined
  return typeof name === 'string' && name && name !== 'Object' ? ` (${name})` : ''
}

function checkData(value, seen = new Set(), depth = 0) {
  if (typeof value === 'function' || typeof value === 'symbol') throw new TypeError(`a ${typeof value} cannot cross between processes`)
  if (value === null || typeof value !== 'object') return
  if (depth >= MAX_DEPTH) throw tooDeep()
  if (seen.has(value)) throw new TypeError('a value that holds itself (a cycle) cannot cross between processes')
  if (!Array.isArray(value) && ![Object.prototype, null].includes(Object.getPrototypeOf(value))) throw new TypeError(`a ${value.constructor?.name ?? 'value'} cannot cross between processes: pass plain data instead`)
  seen.add(value)
  for (const item of Object.values(value)) checkData(item, seen, depth + 1)
  seen.delete(value)
}

class RemotePromise extends Promise {
  static get [Symbol.species]() { return Promise }
  #start
  constructor(start) {
    let resolve, reject
    super((a, b) => { resolve = a; reject = b })
    this.#start = () => { this.#start = undefined; start().then(resolve, reject) }
  }
  then(...args) { this.#start?.(); return super.then(...args) }
}

// One session of the rutis protocol, as the runtime side. The transport owns
// I/O (a worker); every decoder, reference table and callback stays on the
// main thread, including while a synchronous call blocks it and pumps the
// incoming frames.
//
// Re-entrant: while a synchronous call waits, every incoming call runs on
// the main thread, whether or not it belongs to the waiting call's chain.
// Two runtimes that synchronously call each other's services at the same
// time would otherwise each hold back the other's call until its own
// returns, and wait for ever. So a plugin's service may be called while that
// plugin is itself inside a synchronous call: do not hold a lock across a
// call into rutis.
export class Session {
  #send; #pump; #dispatch; #abort; #settled; #inline; #requires
  #next = 0; #received = 0; #ref = 0; #pending = new Map(); #exports = new Map(); #identities = new WeakMap()
  #imports = new Map(); #proxies = new WeakMap(); #finalizer
  #closed; #handshake = false; #resolveReady; #rejectReady
  // Call id prefixes: `node:` / `rust:` (compat), `<endpoint>:` (endpoint format).
  #endpoint; #local = 'node:'; #remote = 'rust:'
  #context = new AsyncLocalStorage(); #syncPath; #waiting = []; #queued = new Set()
  #active = 0; #draining = []
  // AbortControllers for incoming calls that received a signal argument.
  // A signal lives until the call's result settles: a returned Promise keeps
  // it (via its future export), and an `await` frame maps back to the call.
  #signals = new Map(); #decodingFor; #promiseCalls = new WeakMap(); #awaits = new Map()
  // The call a returned Promise is the result of (`service.method`): an
  // await of it whose result cannot be sent names it. And the message limit.
  #resultOf = new WeakMap(); #max
  // `settled` runs after a call or property read on an exported reference
  // returns (or its Promise settles): the owner may have changed state that
  // invoke dispatch would otherwise observe, such as replaced services.
  // `endpoint` ({ local, expected?, verified?, declare? }) selects the
  // endpoint format (`declare`: capabilities beyond the session's, such as
  // the contract); without it the session speaks the compat protocol.
  constructor({ send, pump, dispatch, abort, settled, endpoint, inline, requires, maxMessage = MAX_MESSAGE }) {
    this.#send = send; this.#pump = pump; this.#dispatch = dispatch; this.#abort = abort; this.#settled = settled
    this.#max = maxMessage
    // `inline`: control methods (empty target) run as their frame arrives,
    // in frame order, before the next frame is read, also while a
    // synchronous call waits (`events.on`, `events.off`, `events.elsewhere`).
    // `requires`: capabilities the far end must declare (`events-v2`).
    this.#inline = new Set(inline ?? []); this.#requires = requires ?? endpoint?.requires
    if (endpoint) {
      if (!ENDPOINT_ID.test(endpoint.local)) throw new Error(`invalid endpoint id ${endpoint.local}`)
      this.#endpoint = endpoint; this.#local = `${endpoint.local}:`; this.#remote = undefined
    }
    this.ready = new Promise((resolve, reject) => { this.#resolveReady = resolve; this.#rejectReady = reject })
    this.#finalizer = new FinalizationRegistry(record => this.#release(record))
  }
  start() {
    this.#send(encode(this.#endpoint
      ? { op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: this.#endpoint.local, implementation: { name: MANIFEST.name, version: MANIFEST.version }, capabilities: [...CAPABILITIES, ...(this.#endpoint.declare ?? [])] }
      : { op: 'hello', version: PROTOCOL, capabilities: COMPAT_CAPABILITIES }))
  }
  // What the far end declared (either format), once it greeted.
  #declared: string[] = []
  // What the far end said of itself (endpoint format), once it greeted.
  greeting
  #greet(frame) {
    // Handshake failures carry what they mean for a link: stop on an
    // incompatible far end, retry slowly on a mismatched identity.
    const fail = (category, message) => Object.assign(new Error(message), { category })
    if (!this.#endpoint) {
      if (frame.version !== PROTOCOL || frame.endpoint !== undefined) throw fail('incompatible', `incompatible session: ${versionMismatch(frame.version, PROTOCOL, frame.implementation)}`)
      if (Array.isArray(frame.capabilities)) this.#declared = frame.capabilities
      required(this.#declared, this.#requires)
      return
    }
    this.greeting = greetingOf(frame, { ...this.#endpoint, requires: this.#requires })
    this.#remote = `${this.greeting.endpoint}:`
    this.#declared = this.greeting.capabilities
  }
  supports(capability) { return this.greeting?.capabilities.includes(capability) ?? false }
  // Whether the far end declared `capability` (either format).
  declares(capability) { return this.#declared.includes(capability) }
  close(error = new Error('session closed')) {
    if (this.#closed) return
    this.#closed = error; this.#rejectReady(error)
    for (const pending of this.#pending.values()) pending({ ok: false, value: error })
    this.#pending.clear(); this.#exports.clear(); this.#imports.clear(); this.#queued.clear()
  }
  #fault(error) { this.close(error); this.#abort?.(error) }
  // A call id in an invocation chain: one of either side's, possibly
  // tagged with the session it came through (`s3/mac:4`).
  #callId(id) {
    if (typeof id !== 'string') return false
    // Compat: this session's ids are `node:`/`rust:`; another session's
    // come tagged with it, whatever its format (`s1/node:3`, #225).
    if (!this.#endpoint) return /^(node|rust|s[0-9]+\/[a-z0-9-]+):[1-9][0-9]*$/.test(id)
    return /^(s[0-9]+\/)?[a-z0-9-]+:[1-9][0-9]*$/.test(id)
  }
  #path() { return this.#syncPath ?? this.#context.getStore() ?? [] }
  #write(frame) { if (this.#closed) throw this.#closed; this.#send(encode(frame)) }
  #allocate() {
    if (this.#closed) throw this.#closed
    if (!this.#handshake) throw new Error('protocol handshake incomplete')
    if (!Number.isSafeInteger(++this.#next)) throw new Error('call identifiers exhausted')
    return `${this.#local}${this.#next}`
  }
  // `depth`: the arrays and objects around `value`; an argument list
  // starts at -1, as it is not a level.
  #encode(value, grants, business, depth = 0) {
    if (value === undefined) return { type: 'undefined' }
    const imported = value && (typeof value === 'function' || typeof value === 'object') && this.#proxies.get(value)
    if (imported) {
      if (imported.released) throw new Error('reference released')
      return { type: 'reference', value: { id: imported.id, kind: imported.kind, home: true, origin: imported.origin } }
    }
    if (typeof value === 'function' || value instanceof Promise || isLive(value)) {
      let id = this.#identities.get(value), entry = this.#exports.get(id)
      if (!entry) {
        id = ++this.#ref
        if (!Number.isSafeInteger(id)) throw new Error('reference identifiers exhausted')
        const kind = typeof value === 'function' ? 'function' : value instanceof Promise ? 'future' : isPluginDefinition(value) ? 'plugin' : 'object'
        // An object goes only to a far end that can call its methods.
        if (kind === 'object' && this.#endpoint && !this.supports('objects')) throw new ValueError(`an object with methods${className(value)} cannot cross to ${this.greeting.endpoint}; pass plain data or a function instead`)
        if (kind === 'plugin' && !this.#declared.includes('plugins')) throw new ValueError('cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)')
        entry = { origin: [...this.#path()], value, kind, grants: 0, business, call: kind === 'future' ? this.#promiseCalls.get(value) : undefined }
        this.#identities.set(value, id); this.#exports.set(id, entry)
        if (entry.kind === 'future') {
          if (business) this.#active++
          value.then(value => { entry.result = { ok: true, value } }, error => { entry.result = { ok: false, value: error } })
            .finally(() => { if (business) this.#finish() })
        }
      }
      if (!Number.isSafeInteger(entry.grants + 1)) throw new Error('reference grant overflow')
      entry.grants++; grants.push(id)
      return { type: 'reference', value: { id, kind: entry.kind, home: false, origin: entry.origin } }
    }
    const tag = tagOf(value)
    if (tag) return this.#encodeTagged(value, tag, grants, business, depth)
    // Binary data is a Uint8Array, an ArrayBuffer or a DataView: another
    // typed array's element type would be lost.
    if (ArrayBuffer.isView(value)) throw new ValueError(`cannot send ${describe(value)}: binary data crosses as a Uint8Array (or Buffer), an ArrayBuffer or a DataView`)
    if (value !== null && typeof value === 'object' && depth >= MAX_DEPTH) throw tooDeep()
    // An empty slot of an array is undefined, as when it is read (#675).
    if (Array.isArray(value)) return { type: 'list', value: Array.from(value, value => this.#encode(value, grants, business, depth + 1)) }
    // An Error passed as a value (e.g. to a callback) crosses as data.
    if (value instanceof Error) return { type: 'data', value: { name: value.name, message: value.message, ...(typeof value.stack === 'string' ? { stack: value.stack } : {}) } }
    if (value !== null && typeof value === 'object' && needsRecord(value)) {
      return { type: 'record', value: Object.fromEntries(Object.entries(value).map(([key, item]) => [key, this.#encode(item, grants, business, depth + 1)])) }
    }
    checkData(value, new Set(), depth)
    return { type: 'data', value }
  }
  // A Date, binary data, a Map, a Set, a bigint or an SDK constructor's
  // value, to a far end that declared it takes `tag`; else this call fails.
  #encodeTagged(value, tag, grants, business, depth) {
    if (!this.#declared.includes(`value-${tag}`)) throw undeclared(value, tag)
    switch (tag) {
      case 'instant': return { type: tag, value: instantText(value) }
      case 'bytes': return { type: tag, value: base64Encode(bytesOf(value)) }
      case 'bigint': return { type: tag, value: checkText(tag, String(value)) }
      case 'map': case 'set': {
        // Each a level, as an array is (#279).
        if (depth >= MAX_DEPTH) throw tooDeep()
        const entries = tag === 'map' ? [...value] : [...value].map(element => [element])
        checkKeys(entries.map(([key]) => keyOf(key)), tag === 'map' ? 'map key' : 'set element')
        const item = item => this.#encode(item, grants, business, depth + 1)
        // Undefined in a map is null, as in data (design V10).
        return { type: tag, value: entries.map(([key, entry]) => tag === 'set' ? item(key) : [item(key), entry === undefined ? { type: 'data', value: null } : item(entry)]) }
      }
      default: return { type: tag, value: checkText(tag, value.value) }
    }
  }
  #rollback(grants) {
    for (const id of grants) { const entry = this.#exports.get(id); if (--entry.grants === 0) this.#exports.delete(id) }
  }
  // What is being decoded: the first value this side does not take, and the
  // references imported (`#decodeValue`).
  #decoding
  // Decode `wire`, a call's arguments or result, as `{ value }`; or as
  // `{ refused }`, why only that call fails: a tagged value not in its
  // format, or map keys that collide. Decoding goes on past a refused
  // value, so every reference in it is imported, and then released again,
  // as for a value dropped. A frame not in the protocol's shape still ends
  // the session.
  #decodeValue(wire) {
    const decoding = { refused: undefined, imported: [] }, outer = this.#decoding
    this.#decoding = decoding
    let value
    try { value = this.#decode(wire) } finally { this.#decoding = outer }
    if (!decoding.refused) return { value }
    const counts = new Map()
    for (const record of decoding.imported) counts.set(record, (counts.get(record) ?? 0) + 1)
    for (const [record, count] of counts) {
      if (record.released) continue
      if (count >= record.grants) { this.#finalizer.unregister(record); this.#release(record); continue }
      record.grants -= count
      try { this.#write({ op: 'release', reference: record.id, count }) } catch (error) { this.#fault(error) }
    }
    return { refused: decoding.refused }
  }
  #refuse(error) {
    if (!(error instanceof ValueError) || !this.#decoding) throw error
    this.#decoding.refused ??= error
    return undefined
  }
  // A tagged value's text, as `read` makes it this side's value.
  #text(wire, read) {
    if (typeof wire.value !== 'string') throw new Error(`invalid ${wire.type} value`)
    try { return read(wire.value) } catch (error) { return this.#refuse(error) }
  }
  // A map's entries or a set's elements, made into a Map or Set once all
  // are decoded and checked as keys; undefined when the call is refused.
  #collection(wire, what, make) {
    const entries = wire.type === 'map'
      ? wire.value.map(([key, value]) => [key, this.#decode(key), this.#decode(value)])
      : wire.value.map(element => [element, this.#decode(element)])
    if (this.#decoding?.refused) return undefined
    // Undefined in a map is null, as in data (design V10).
    const made = make(entries.map(([, key, value]) => wire.type === 'map' ? [key, value === undefined ? null : value] : key))
    try { checkKeys(entries.map(([key, value]) => [key?.type, value]), what, made) } catch (error) { return this.#refuse(error) }
    return made
  }
  #decode(wire) {
    switch (wire?.type) {
      case 'instant': return this.#text(wire, instantFromText)
      // Strings in JavaScript, as written (design §2).
      case 'datetime': case 'date': case 'time': case 'decimal': case 'uuid': return this.#text(wire, text => checkText(wire.type, text))
      case 'bigint': return this.#text(wire, text => BigInt(checkText('bigint', text)))
      case 'bytes': return this.#text(wire, base64Decode)
      case 'map':
        if (!Array.isArray(wire.value) || wire.value.some(entry => !Array.isArray(entry) || entry.length !== 2)) throw new Error('invalid map')
        return this.#collection(wire, 'map key', entries => new Map(entries))
      case 'set':
        if (!Array.isArray(wire.value)) throw new Error('invalid set')
        return this.#collection(wire, 'set element', elements => new Set(elements))
      case 'undefined': return undefined
      case 'data': return wire.value
      case 'list': return wire.value.map(value => this.#decode(value))
      case 'signal': {
        // The caller may cancel this call: the method sees a real AbortSignal.
        if (!this.#decodingFor) throw new Error('a signal is only valid as a call argument')
        let controller = this.#signals.get(this.#decodingFor)
        if (!controller) this.#signals.set(this.#decodingFor, controller = new AbortController())
        return controller.signal
      }
      case 'record': {
        if (wire.value === null || typeof wire.value !== 'object' || Array.isArray(wire.value)) throw new Error('invalid record')
        return Object.fromEntries(Object.entries(wire.value).map(([key, value]) => [key, this.#decode(value)]))
      }
      case 'reference': {
        const { id, home, kind, origin } = wire.value
        if (!Number.isSafeInteger(id) || id <= 0 || !['function', 'future', 'object', 'plugin'].includes(kind) || typeof home !== 'boolean' || !Array.isArray(origin) || origin.some(id => !this.#callId(id))) throw new Error('invalid reference')
        if (home) return this.#export(id, kind).value
        // This side cannot call an object's methods: only the call it came
        // in fails (an older host may send one, #675), and it is released.
        if (kind === 'object') {
          this.#decoding?.imported.push({ id, kind, grants: 1, released: false })
          return this.#refuse(new ValueError('received an object with methods, which JavaScript cannot take; pass plain data or a function instead'))
        }
        // Rust does not export plugin definitions; only this side's travel
        // back home.
        if (kind === 'plugin') throw new Error('plugin definitions exported by Rust are not supported')
        let proxy = this.#imports.get(id)?.deref()
        if (proxy) {
          const record = this.#proxies.get(proxy)
          if (record.kind !== kind || JSON.stringify(record.origin) !== JSON.stringify(origin) || !Number.isSafeInteger(record.grants + 1)) throw new Error('invalid repeated grant')
          record.grants++; this.#decoding?.imported.push(record); return proxy
        }
        const record = { id, kind, origin, grants: 1, released: false }
        const check = () => { if (record.released) throw new Error('reference released') }
        proxy = kind === 'function'
          ? (...args) => { check(); return this.#requestSync('call', { reference: id }, args) }
          : new RemotePromise(() => { check(); return this.#requestAsync('await', { reference: id }, undefined, origin) })
        record.weak = new WeakRef(proxy)
        this.#imports.set(id, record.weak); this.#proxies.set(proxy, record)
        this.#decoding?.imported.push(record)
        this.#finalizer.register(proxy, record, record)
        return proxy
      }
      default: throw new Error('invalid wire value')
    }
  }
  #export(id, kind) {
    const entry = this.#exports.get(id)
    if (!entry || (kind && entry.kind !== kind)) throw new Error('unknown, released or mismatched reference')
    return entry
  }
  #release(record) {
    if (record.released) return
    record.released = true
    if (this.#imports.get(record.id) === record.weak) this.#imports.delete(record.id)
    if (!this.#closed) { try { this.#write({ op: 'release', reference: record.id, count: record.grants }) } catch (error) { this.#fault(error) } }
  }
  // Reference table sizes: exported (granted, not yet released by the far
  // end) and imported (not yet released). Diagnostics for tests (risk P4).
  references() { return { exports: this.#exports.size, imports: this.#imports.size } }
  // Explicit release is an adapter/test operation. Session close does not wait
  // for JS GC; ordinary native functions are retained through their proxies.
  release(proxy) {
    const record = this.#proxies.get(proxy)
    if (!record) throw new Error('not an imported reference')
    this.#finalizer.unregister(record); this.#release(record)
  }
  #request(op, fields, args, finish, origin = [], sync = false) {
    const id = this.#allocate(), grants = []
    try {
      const call = () => describeCall(op, fields)
      const frame: any = { op, id, path: [...new Set([...this.#path(), ...origin])], ...fields, ...(op === 'await' ? {} : { args: this.#encodeArgs(args, grants, call) }) }
      // This thread waits for the reply: a host that checks for wait
      // cycles needs to know (#228); older hosts would refuse the field.
      if (sync && this.#declared.includes('sync-wait')) frame.sync = true
      const encoded = encode(frame)
      // Over the limit, only this call fails, before it is sent (#675).
      const size = sizeOver(encoded, this.#max)
      if (size > this.#max) {
        const items = frame.args?.type === 'list' ? frame.args.value : []
        const sizes = items.map(item => Buffer.byteLength(encode(item)))
        const largest = sizes.indexOf(Math.max(...sizes))
        const what = largest < 0 ? `the arguments of ${call()}` : `argument ${largest + 1} of ${call()}`
        throw new RangeError(`${what}: ${overLimit(size, this.#max)}`)
      }
      this.#pending.set(id, finish); this.#send(encoded)
    } catch (error) { this.#pending.delete(id); this.#rollback(grants); throw error }
    return id
  }
  // A call's arguments, each nested at most MAX_DEPTH deep: what cannot be
  // sent says which argument of `call` it is.
  #encodeArgs(args, grants, call) {
    if (!Array.isArray(args)) return this.#encode(args, grants, true, -1)
    return { type: 'list', value: Array.from(args, (arg, index) => {
      try { return this.#encode(arg, grants, true, 0) } catch (error) { throw ofArgument(error, index, call()) }
    }) }
  }
  #requestSync(op, fields, args) {
    let result
    const id = this.#request(op, fields, args, value => { result = value }, [], true)
    this.#waiting.push(id)
    try {
      for (const job of this.#queued) this.#run(job)
      while (!result) this.#pump(() => !!result)
    }
    finally {
      this.#waiting.pop()
      if (!this.#waiting.length) for (const job of this.#queued) queueMicrotask(() => this.#run(job))
    }
    if (!result.ok) throw result.value
    return result.value
  }
  #requestAsync(op, fields, args, origin) {
    return new Promise((resolve, reject) => this.#request(op, fields, args, result => result.ok ? resolve(result.value) : reject(result.value), origin))
  }
  invoke(target, method, args) { return this.#requestSync('invoke', { target, method }, args) }
  // Promise assimilation here is intentional: this is the generated async
  // method API. Synchronous invoke returns the Promise reference itself.
  invokeAsync(target, method, args) { return this.#requestAsync('invoke', { target, method }, args) }
  // Send a call nobody waits for; `answered({ ok, value })` is told its
  // reply as the reply's frame is taken, before the next frame (or the
  // session's end), not on a later tick (events protocol §5.5).
  invokeAnswered(target, method, args, answered) { this.#request('invoke', { target, method }, args, answered) }
  // Call an imported function asynchronously (a waterfall's `next`, #464);
  // calling the function itself waits synchronously.
  callFunctionAsync(proxy, args) {
    const record = this.#proxies.get(proxy)
    if (!record || record.kind !== 'function') return Promise.reject(new TypeError('not an imported function'))
    if (record.released) return Promise.reject(new Error('reference released'))
    return this.#requestAsync('call', { reference: record.id }, args)
  }
  drain() { return this.#active ? new Promise(resolve => this.#draining.push(resolve)) : Promise.resolve() }
  #finish() { if (--this.#active === 0) for (const resolve of this.#draining.splice(0)) resolve() }
  receive(frame) {
    if (this.#closed) return
    try {
      if (frame.op === 'hello') {
        if (this.#handshake) throw new Error('duplicate protocol handshake')
        this.#greet(frame)
        this.#handshake = true; this.#resolveReady(); return
      }
      if (!this.#handshake) throw new Error('request before protocol handshake')
      if (frame.op === 'return' || frame.op === 'throw') {
        // Decode/pin before admitting a subsequent release frame.
        if (frame.op === 'throw' && (typeof frame.error?.name !== 'string' || typeof frame.error.message !== 'string')) throw new Error('invalid error')
        // A result this side does not take fails the call waiting for it.
        const { value, refused } = frame.op === 'return' ? this.#decodeValue(frame.value) : { value: decodeError(frame.error), refused: undefined }
        const pending = this.#pending.get(frame.id)
        if (!pending) throw new Error('response for unknown call')
        this.#pending.delete(frame.id); pending(refused ? { ok: false, value: refused } : { ok: frame.op === 'return', value }); return
      }
      if (frame.op === 'cancel') {
        if (typeof frame.id !== 'string') throw new Error('invalid cancel')
        // The caller gave up; the method decides how to honour its signal.
        const call = this.#signals.has(frame.id) ? frame.id : this.#awaits.get(frame.id)
        this.#signals.get(call)?.abort(new DOMException('The operation was cancelled by the caller', 'AbortError'))
        return
      }
      if (frame.op === 'release') {
        const entry = this.#export(frame.reference)
        if (!Number.isSafeInteger(frame.count) || frame.count <= 0 || frame.count > entry.grants) throw new Error('invalid reference release count')
        if ((entry.grants -= frame.count) === 0) this.#exports.delete(frame.reference)
        return
      }
      if (!['invoke', 'call', 'get', 'await'].includes(frame.op) || typeof frame.id !== 'string' || !frame.id.startsWith(this.#remote) || !Array.isArray(frame.path) || frame.path.some(id => typeof id !== 'string') || frame.path.includes(frame.id)) throw new Error('invalid invocation')
      const digits = frame.id.slice(this.#remote.length), sequence = Number(digits)
      if (!/^[1-9][0-9]*$/.test(digits) || !Number.isSafeInteger(sequence) || sequence <= this.#received) throw new Error('invalid or repeated invocation identity')
      this.#received = sequence
      if (frame.op === 'invoke' && (typeof frame.target !== 'string' || typeof frame.method !== 'string')) throw new Error('invalid target or method')
      if (frame.op === 'call' && frame.method !== undefined && typeof frame.method !== 'string') throw new Error('invalid method')
      if (frame.op === 'get' && typeof frame.property !== 'string') throw new Error('invalid property')
      let decoded
      this.#decodingFor = frame.id
      try { decoded = frame.op === 'await' || frame.op === 'get' ? { value: undefined } : this.#decodeValue(frame.args) } finally { this.#decodingFor = undefined }
      const kind = frame.op === 'await' ? 'future' : frame.op === 'get' || frame.method !== undefined ? 'object' : 'function'
      const entry = frame.op === 'invoke' ? undefined : this.#export(frame.reference, kind)
      // Arguments this side does not take: only this call fails.
      if (decoded.refused) {
        this.#signals.delete(frame.id)
        this.#write({ op: 'throw', id: frame.id, error: encodeError(decoded.refused) })
        return
      }
      const args = decoded.value
      const business = entry?.business ?? frame.target !== ''
      if (business) this.#active++
      const job = { frame, args, entry, business }
      this.#queued.add(job)
      if (frame.op === 'invoke' && frame.target === '' && this.#inline.has(frame.method)) {
        this.#queued.delete(job)
        this.#execute(frame, args, entry, business)
        return
      }
      if (this.#waiting.length) this.#run(job)
      else queueMicrotask(() => this.#run(job))
    } catch (error) { this.#fault(error) }
  }
  #related(job) { return this.#waiting.some(id => job.frame.path.includes(id)) }
  #run(job) {
    if (!this.#queued.has(job)) return
    this.#queued.delete(job)
    this.#execute(job.frame, job.args, job.entry, job.business)
  }
  #execute(frame, args, entry, business) {
    if (this.#closed) { if (business) this.#finish(); return }
    const path = [...frame.path, frame.id]
    const previous = this.#syncPath
    this.#syncPath = path
    // The call answered, for messages: an await's is the call that
    // returned the Promise, when it was one of the far end's.
    const call = frame.op === 'await' ? this.#resultOf.get(entry.value) : describeCall(frame.op, frame)
    try {
      this.#context.run(path, () => {
        if (frame.op === 'await') {
          if (entry.call) this.#awaits.set(frame.id, entry.call)
          if (entry.result) this.#respond(frame.id, entry.result, business, call)
          // The future settles only when the event loop runs, which the
          // synchronous call it belongs to holds.
          else if (this.#waiting.length && this.#related({ frame })) this.#respond(frame.id, { ok: false, value: Object.assign(new Error(`await requires the Bun thread occupied by its parent synchronous call; path: ${path.join(' -> ')}`), { name: 'SyncWaitCycle' }) }, business)
          else entry.value.then(value => this.#respond(frame.id, { ok: true, value }, business, call), value => this.#respond(frame.id, { ok: false, value }, business, call))
          return
        }
        try {
          let value
          try {
            value = frame.op === 'get' ? entry.value[frame.property]
              : frame.op === 'call' && frame.method !== undefined ? Reflect.apply(entry.value[frame.method], entry.value, args)
                : frame.op === 'call' ? Reflect.apply(entry.value, undefined, args)
                  : this.#dispatch(frame.target, frame.method, args)
          } finally {
            if (frame.op !== 'invoke' && !(value instanceof Promise)) this.#settled?.()
          }
          if (frame.op !== 'invoke' && value instanceof Promise) value.then(() => this.#settled?.(), () => this.#settled?.())
          if (value instanceof Promise) this.#resultOf.set(value, call)
          // A returned Promise keeps the call's signal until it settles.
          if (value instanceof Promise && this.#signals.has(frame.id)) {
            const call = frame.id
            this.#promiseCalls.set(value, call)
            value.then(() => this.#signals.delete(call), () => this.#signals.delete(call))
          }
          this.#respond(frame.id, { ok: true, value }, business, call)
        } catch (value) { this.#respond(frame.id, { ok: false, value }, business, call) }
      })
    } finally { this.#syncPath = previous }
  }
  // Answer the far end's call `id` (of `call`, for messages): a result that
  // cannot be sent (a value the far end does not take, a message over the
  // limit) is answered with an error saying why instead (#675).
  #respond(id, result, business, call?) {
    this.#awaits.delete(id)
    if (!(result.ok && result.value instanceof Promise && this.#promiseCalls.has(result.value))) this.#signals.delete(id)
    const grants = []
    try {
      if (this.#closed) return
      let frame
      try {
        frame = result.ok ? { op: 'return', id, value: this.#encode(result.value, grants, business) } : { op: 'throw', id, error: encodeError(result.value) }
        frame = encode(frame)
      } catch (error) {
        this.#rollback(grants); grants.length = 0
        frame = encode({ op: 'throw', id, error: encodeError(result.ok ? ofResult(error, call) : error) })
      }
      const size = sizeOver(frame, this.#max)
      if (size > this.#max) {
        this.#rollback(grants); grants.length = 0
        const what = result.ok ? 'the result' : `the error ${result.value?.name ?? 'ThrownValue'}`
        frame = encode({ op: 'throw', id, error: encodeError(new RangeError(`${call ? `${what} of ${call}` : what}: ${overLimit(size, this.#max)}`)) })
      }
      this.#send(frame)
    } catch (error) { this.#fault(error) }
    finally { if (business) this.#finish() }
  }
}
