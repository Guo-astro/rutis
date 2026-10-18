import { createRequire } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { isAbsolute } from 'node:path'
import { statSync } from 'node:fs'
import { AsyncLocalStorage } from 'node:async_hooks'
import { Process } from './client.mjs'
import { toJsonSchema } from './schema.mjs'
import { isLeaf, refuseInternal, supported, toCordis } from './leaf.mjs'

// `<channel> [--id <endpoint>] [--peer <endpoint>] [--format endpoint] <first plugin or anchor>`.
// Local channels (`fd:3`, a socket path) speak the compat protocol unless
// `--format endpoint`; network channels (`ws://`, `wss://`) the endpoint
// format, as `--id`, expecting `--peer` as the controller when given.
// `listen:…` serves controllers one session at a time (serve.mjs);
// `dial:ws://…` / `dial:wss://…` dials a rutis access point and dials
// again (dial.mjs). Both run each session in a child runner; a dialing
// runtime's (`--supervisor dial`), on SIGINT or SIGTERM, unloads its rows
// and ends its session.
const { channelSpec, pluginPath, endpoint, flags } = (() => {
  const [channelSpec, ...rest] = process.argv.slice(2)
  const flags = {}
  while (rest[0]?.startsWith('--')) flags[rest.shift().slice(2)] = rest.shift()
  const network = /^(wss?|listen|dial):/.test(channelSpec) || flags.format === 'endpoint'
  if (network && !flags.id) throw new Error(`a network channel needs --id <endpoint>: ${channelSpec}`)
  // A runner is a runtime: the controller manages its rows.
  const endpoint = network ? { local: flags.id, expected: flags.peer, declare: ['runtime'] } : undefined
  return { channelSpec, pluginPath: rest[0], endpoint, flags }
})()

if (channelSpec.startsWith('listen:')) {
  const { serve } = await import('./serve.mjs')
  await serve({ spec: channelSpec.slice('listen:'.length), id: flags.id, peer: flags.peer, anchor: pluginPath })
}
if (channelSpec.startsWith('dial:')) {
  const { dialOut } = await import('./dial.mjs')
  const code = await dialOut({ spec: channelSpec.slice('dial:'.length), id: flags.id, peer: flags.peer, anchor: pluginPath })
  await new Promise(resolve => process.stderr.write('', resolve))
  process.exit(code)
}
// The session of a dialing runtime: its supervisor reports what ends it.
const dialed = flags.supervisor === 'dial'

// The plugins' Service classes must come from the same Cordis instance as the
// Context, so prefer the Cordis that the (first) plugin itself resolves.
function cordisOf(entry) {
  try { return createRequire(entry).resolve('@deepseek-ai/cordis') } catch { return undefined }
}
const cordisPath = cordisOf(pluginPath)
const { Context, Inject, Service, resolveConfig } = await import(cordisPath ? pathToFileURL(cordisPath).href : '@deepseek-ai/cordis')
// Volatile config helpers from the cosmokit that Cordis itself uses.
const cosmokit = await import(pathToFileURL(createRequire(cordisPath ?? fileURLToPath(import.meta.resolve('@deepseek-ai/cordis'))).resolve('@deepseek-ai/cosmokit')).href).catch(() => ({}))
let peer
const ctx = new Context()
let fibers
let mounted = false
let closing = false
let disposing
let version = 0
let emits = new Set() // events the rutis side may emit here
// The host takes the services rows provide without declaring them, each
// announced with its methods (mount `announce`, feature `rows.announce`).
// A host that did not ask (0.11) hears only the declared ones.
let announce = false
// Rows: plugins rutis-loader manages one by one in this Context (`rows.*`).
const rows = new Map() // key -> { fiber, inner, config, exports, loading, scope, labelOf }
// rutis services registered one by one (`hosts.*`): id -> withdraw
const hosts = new Map()
// What this runner supports beyond protocol 2, reported by `mount`.
// events: leaf plugins receive what rutis emits (RuntimePlugin::emits).
// rows.announce: the services a row provides (a Cordis plugin's, or a leaf
// plugin's `ctx.provide`) reach rutis without a declaration, announced with
// the methods they have (`service` with a fourth argument, to a host whose
// mount asked for it).
const FEATURES = ['rows.v2', 'hosts', 'leaf.js', 'scopes', 'events', 'rows.announce']
// Test support, not part of the protocol: with RUTIS_TEST_INSPECT set, the
// control method `inspect` reports what this runner holds (rows, host
// proxies, export slots, object handles, Cordis plugins, fibers and
// services), so a test can check that nothing of the rows it unloaded is
// left in the process. Without it the method is unknown.
const INSPECT = Boolean(process.env.RUTIS_TEST_INSPECT)

// A service in a scope: rows isolating `name` with `label` share it, and the
// same name in another scope (another instance, say) is another service.
// Export slots, host proxies and `host:<id>` targets go by this id
// (`scopes`): the name outside any scope (no label: undefined or null), the
// name, a NUL and the label inside one. Neither may contain NUL and a label
// may not be empty, so no two pairs share an id.
const scopedId = (name, label) => {
  if (name.includes('\0') || label?.includes('\0')) throw new Error(`service ${JSON.stringify(name)} or its scope label contains NUL`)
  if (label === '') throw new Error(`service ${JSON.stringify(name)} has an empty scope label`)
  return label == null ? name : `${name}\0${label}`
}
// The handle of a slot's `generation`th object. A scoped id's handles are
// marked with a NUL too: a label may contain `#`.
const handleOf = (id, generation) => generation === 1 ? id : `${id}${id.includes('\0') ? '\0' : '#'}${generation}`
// Cordis keys an isolated service by its symbol alone, so the symbol names
// the service as well as the label: two names isolated with one label are
// two services.
const isolated = (name, label) => Symbol.for(`rutis-row:${JSON.stringify([label, name])}`)

// Each exported service slot is projected as a sequence of object handles.
// A handle always addresses the object it was created for; when the slot
// changes, the Rust side receives a new handle and replaces its native proxy.
// The first object of a slot uses its id (scopedId) as its handle.
const slots = new Map() // id -> { name, methods, scope, object, identity, handle, generation, exporter, row? }
const handles = new Map() // handle -> { name: slot id, object, current, released }

// Cordis wraps Service instances in a new tracing proxy on every read; the
// proxy reports its target under this symbol. Compare targets, not wrappers.
const ORIGINAL = Symbol.for('cordis.original')
const identity = value => (value !== null && (typeof value === 'object' || typeof value === 'function')) ? (value[ORIGINAL] ?? value) : value

// Every exported service is read through its own exporter fiber that injects
// it, like any native consumer: Cordis gates it on availability including
// Service.check(), and effects that service methods create through the
// caller's context belong to this fiber and are disposed with it.
function exporter(name, slot, parent = ctx) {
  return parent.plugin({
    name: `interop-export:${name}`,
    inject: [name],
    apply(scope) {
      slot.scope = scope
      refresh()
      scope.effect(() => () => {
        if (slot.scope !== scope) return
        slot.scope = undefined
        refresh()
      })
    },
  })
}

function read(slot, name) {
  if (!slot.scope) return undefined
  try { return slot.scope.get(name) } catch { return undefined }
}

function retire(handle) {
  const entry = handle && handles.get(handle)
  if (!entry) return
  entry.current = false
  if (entry.released) handles.delete(handle)
}

// Re-read every slot after any signal that may change it. Cordis emits
// internal/service for provide, withdrawal and provider activation, and
// internal/set for property assignment; a direct ctx.set() emits nothing and
// is observed after the next call into this process.
function refresh() {
  for (const [id, slot] of slots) {
    const object = read(slot, slot.name)
    const current = identity(object)
    if (current === slot.identity) {
      // Same service, possibly read through a new exporter scope: calls must
      // use the live scope, but the handle and Rust proxy stay.
      if (object !== undefined && slot.handle) handles.get(slot.handle).object = object
      continue
    }
    retire(slot.handle)
    slot.object = object
    slot.identity = current
    slot.handle = null
    if (object !== undefined) {
      slot.generation++
      slot.handle = handleOf(id, slot.generation)
      handles.set(slot.handle, { name: id, object, current: true, released: false })
      // A service no declaration describes offers the methods it has.
      if (slot.row !== undefined) slot.methods = methodsOf(current)
    }
    slot.version = ++version
    if (mounted && !closing) {
      // An announced service says whose it is and what it offers.
      const args = slot.row === undefined
        ? [id, slot.handle, slot.version]
        : [id, slot.handle, slot.version, { row: slot.row, methods: Object.fromEntries([...slot.methods].map(method => [method, 'async'])) }]
      // Kept, so a row loading answers only once rutis has heard it.
      slot.announcing = peer.callAsync('', 'service', args).catch(() => {})
    }
  }
}

// The methods a service object offers when nothing declares them: its own
// and inherited functions, up to Object's (and Cordis's Service base, whose
// machinery is not the service's), not the constructor. A property that
// is not a function hides an inherited function of the same name.
function methodsOf(object) {
  const methods = new Set()
  if (object === null || (typeof object !== 'object' && typeof object !== 'function')) return methods
  const seen = new Set()
  const stops = [Object.prototype, Function.prototype, Service?.prototype]
  for (let level = object; level != null && !stops.includes(level); level = Object.getPrototypeOf(level)) {
    for (const name of Object.getOwnPropertyNames(level)) {
      if (seen.has(name)) continue
      seen.add(name)
      if (name === 'constructor') continue
      if (typeof Object.getOwnPropertyDescriptor(level, name)?.value === 'function') methods.add(name)
    }
  }
  return methods
}

// The row whose plugin `fiber` belongs to: its own fiber, its gate's, or a
// fiber its plugin started.
function rowOf(fiber) {
  for (let current = fiber; current; current = current.runtime ? current.parent?.fiber : undefined) {
    for (const [key, row] of rows) {
      if (row.fiber === current || row.inner === current) return [key, row]
    }
  }
}

// Find the services rows provide that nothing exports yet, and export each
// from its row (announced, see `refresh`): read through the row's scope,
// like the declared ones, so a service provided in a scope the row does not
// see (a plugin isolating it further) stays where it is. Returns the new
// slots.
function discover() {
  const found = []
  if (!announce || closing) return found
  const store = ctx.reflect.store
  for (const key of Reflect.ownKeys(store)) {
    const impl = store[key]
    const owner = rowOf(impl?.fiber)
    if (!owner) continue
    const [rowKey, row] = owner
    const name = impl.name
    if (typeof name !== 'string' || name.includes('#') || name.includes('\0')) continue
    if (row.scope[Context.isolate]?.[name] !== key) continue
    const id = scopedId(name, row.labelOf(name))
    if (row.exports.includes(id) || slots.has(id)) continue
    const slot = { name, methods: new Set(), scope: undefined, object: undefined, identity: undefined, handle: null, generation: 0, version: 0, row: rowKey }
    row.exports.push(id)
    slots.set(id, slot)
    slot.exporter = exporter(name, slot, row.scope)
    found.push(slot)
  }
  return found
}

// Services appear as Cordis emits internal/service, from inside its own
// bookkeeping: they are looked for once it is done.
let discovering = false
function discoverLater() {
  if (discovering || !announce || !rows.size) return
  discovering = true
  queueMicrotask(() => {
    discovering = false
    discover()
  })
}

// Cordis's FiberState is a const enum, gone at run time.
const ACTIVE = 2
const DISPOSED = 4
const UNLOADING = 5

// A row whose plugin disposes its own fiber has ended: what is left of it
// goes, and rutis hears it (`rows.ended`) and disposes the row there, as a
// Rust plugin disposing itself. Disposals that are not the plugin's own do
// not count: `unloadRow` forgets the row first, and a gate unloading takes
// its plugin with it. A row still loading is left to `loadRow`, which
// answers first: a load that fails reports only its error.
const hasEnded = row => row.inner?.uid === null && row.fiber.state !== UNLOADING

function endRow(key) {
  unloadRow(key).catch(() => {}).then(() => {
    if (!closing) peer.callAsync('', 'rows.ended', [key]).catch(() => {})
  })
}

ctx.on('internal/status', fiber => {
  if (fiber.state !== DISPOSED || closing) return
  const ended = [...rows].find(([, row]) => row.inner === fiber && !row.loading && hasEnded(row))
  if (ended) endRow(ended[0])
})

ctx.on('internal/service', () => {
  if (slots.size) refresh()
  discoverLater()
})
ctx.on('internal/set', (_ctx, _name, _value, _error, next) => {
  const result = next()
  if (slots.size) refresh()
  return result
})

// Exporters go first (they consume the services), then rows and the
// plugins in reverse load order, like a native composition unwinding.
function dispose() {
  return disposing ??= (async () => {
    for (const slot of slots.values()) await slot.exporter?.dispose()
    for (const { fiber } of [...rows.values()].reverse()) await fiber.dispose()
    rows.clear()
    for (const fiber of [...(fibers ?? [])].reverse()) await fiber.dispose()
  })()
}

// A plugin module: an `apply` export is a function plugin; otherwise the
// default export, which is how packaged plugins ship their Service class.
// A plugin path, from a row's entry: a file path, a file URL, or (for a
// controller elsewhere, which cannot see this machine's files) an npm name
// or subpath resolved from this runtime's anchor. An unknown name is a
// NotFound the controller reports as unresolved.
function located(entry) {
  if (entry.startsWith('file:')) return fileURLToPath(entry)
  if (isAbsolute(entry)) return entry
  try { return createRequire(pluginPath).resolve(entry) }
  catch (error) {
    if (error.code !== 'MODULE_NOT_FOUND' && error.code !== 'ERR_PACKAGE_PATH_NOT_EXPORTED') throw error
    const missing = new Error(`no plugin ${entry} here`)
    missing.name = 'NotFound'
    throw missing
  }
}

async function pluginOf(entry) {
  entry = located(entry)
  const declared = await declaredOf(entry)
  return isLeaf(declared) ? toCordis(declared) : declared
}

// The module's plugin as written: a leaf plugin (`definePlugin`), or a
// Cordis plugin.
async function declaredOf(entry) {
  const own = cordisOf(entry)
  if (cordisPath && own && own !== cordisPath) {
    throw new Error(`${entry} resolves a different Cordis (${own}) than ${pluginPath} (${cordisPath})`)
  }
  const module = await import(fresh(entry))
  return supported(typeof module.apply === 'function' ? module : (module.default ?? module), entry)
}

// The module URL to import: the plain one the first time, and one that
// bypasses the module cache once the file changed since (rutis-loader's
// reload asks for the new code). Only the plugin's own module is imported
// again, not the modules it imports, as for Python plugins.
const stamps = new Map()
function fresh(entry) {
  const href = pathToFileURL(entry).href
  let stat
  try { stat = statSync(entry) } catch { return href }
  const stamp = `${stat.mtimeMs}-${stat.size}`
  const seen = stamps.get(entry)
  if (seen === undefined) { stamps.set(entry, { stamp, href }); return href }
  if (seen.stamp !== stamp) stamps.set(entry, { stamp, href: `${href}?rutis-reload=${stamp}` })
  return stamps.get(entry).href
}

// `plugin()` returns a thenable `Object.create(fiber)`; state written through
// it (`update` sets `_config`) would land on the wrapper and never reach the
// fiber that later activates. Rows keep the fiber itself.
const fiberOf = wrapped => Object.hasOwn(wrapped, 'then') ? Object.getPrototypeOf(wrapped) : wrapped

// One row: `isolate` as [name, label] pairs (rows naming a label share its
// scope), `inject` as extra service names gating the row. With `inject`, the
// plugin runs inside a gate fiber (`inner`), from the row's latest config.
async function loadRow([key, entry, config, isolate, inject, exports]) {
  if (rows.has(key)) throw new Error(`row ${key} is already loaded`)
  const names = Object.keys(exports ?? {})
  // A name the row isolates is exported in the scope of its label.
  const labelOf = name => (isolate ?? []).find(([isolatedName]) => isolatedName === name)?.[1]
  const ids = names.map(name => scopedId(name, labelOf(name)))
  for (const [index, name] of names.entries()) {
    if (name.includes('#')) throw new Error(`service name ${name} cannot be projected`)
    const id = ids[index]
    const owner = [...rows].find(([, row]) => row.exports.includes(id))
    if (owner || slots.has(id)) throw new Error(`service ${id} is already exported${owner ? ` by row ${owner[0]}` : ''}`)
  }
  const plugin = await pluginOf(entry)
  let scope = ctx
  for (const [name, label] of isolate ?? []) {
    scopedId(name, label)
    scope = scope.isolate(name, isolated(name, label))
  }
  const row = { fiber: undefined, inner: undefined, config, exports: ids, loading: true, scope, labelOf }
  const fiber = fiberOf(inject?.length
    ? scope.plugin({ name: `row:${key}`, inject, apply(gated) { row.inner = fiberOf(gated.plugin(plugin, row.config)) } })
    : scope.plugin(plugin, config))
  row.fiber = fiber
  if (!inject?.length) row.inner = fiber
  rows.set(key, row)
  // The row's services are read from its own scope, like a consumer of it
  // would, so an isolated row exports the service of its isolated scope.
  for (const [index, name] of names.entries()) {
    const slot = { name, methods: new Set(Object.keys(exports[name] ?? {})), scope: undefined, object: undefined, identity: undefined, handle: null, generation: 0, version: 0 }
    slots.set(ids[index], slot)
    slot.exporter = exporter(name, slot, scope)
  }
  try {
    await fiber.await()
    // What the row provides without declaring it is announced before the
    // reply, as the declared services are.
    discover()
    const announced = row.exports.map(id => slots.get(id)).filter(slot => slot?.row === key)
    await Promise.all(announced.map(slot => slot.exporter.await()))
    // rutis has published them (or refused one, which fails the row there)
    // before it hears that the row loaded.
    await Promise.all(announced.map(slot => slot.announcing))
  } catch (error) {
    await unloadRow(key)
    throw error
  }
  row.loading = false
  // Ended while it loaded (rutis unloading it meanwhile forgot it).
  if (rows.get(key) === row && hasEnded(row)) endRow(key)
  return null
}

// Exporters first: their withdrawal reaches rutis (the service's consumers
// stop) before the plugin itself goes away.
async function unloadRow(key) {
  const row = rows.get(key)
  if (!row) return null
  rows.delete(key)
  for (const id of row.exports) {
    const slot = slots.get(id)
    await slot?.exporter?.dispose()
    if (slot) retire(slot.handle)
    slots.delete(id)
  }
  await row.fiber.dispose()
  return null
}

// What rutis-loader needs before loading a plugin: its config schema, the
// services it injects (all required in Cordis), and the services it provides
// to rutis with their method kinds, from `rutis.provides` in its package.json.
// A Cordis plugin whose package declares none (a published one, say) may
// provide services all the same: they reach rutis when it provides them
// (`announces`), so what it provides is known only then.
async function describe(entry) {
  entry = located(entry)
  const declared = await declaredOf(entry)
  // A leaf plugin declares in code; one that declares no `provides` (an
  // SDK of 0.12 or later says so) has its services announced.
  if (isLeaf(declared)) return { config: declared.config ?? null, inject: declared.inject, provides: declared.provides, announces: declared.announces === true }
  const plugin = declared
  const schema = plugin.Config ?? plugin.schema
  const provides = await providesOf(entry)
  return {
    config: schema ? toJsonSchema(schema) : null,
    inject: Object.keys(Inject.resolve(plugin.inject)),
    provides: provides ?? {},
    announces: provides === undefined,
    // A Cordis plugin, not a leaf one: what it injects from rows of this
    // runtime resolves in Cordis.
    cordis: true,
  }
}

// `rutis.provides` in the package.json nearest the plugin, if it declares it.
async function providesOf(entry) {
  const { readFile } = await import('node:fs/promises')
  const { dirname, join } = await import('node:path')
  for (let dir = dirname(entry); ; dir = dirname(dir)) {
    const text = await readFile(join(dir, 'package.json'), 'utf8').catch(() => undefined)
    if (text !== undefined) return JSON.parse(text).rutis?.provides
    if (dirname(dir) === dir) return undefined
  }
}

// A new config for a row. Volatile values are committed into the running
// plugin's references and announced with `loader/volatile-update`, as
// cordis-plugin-loader's `_commitVolatile` does. Whenever that cannot apply
// the change — ordinary values changed too, or the parsed config holds no
// volatile references to commit into — the row takes an ordinary update
// (a restart), so the plugin never keeps running on the old values.
// (rutis-loader sends only changes its schema calls volatile; Cordis's parse
// of the config has the last word.)
async function updateRow([key, config]) {
  const row = rows.get(key)
  if (!row) throw new Error(`row ${key} is not loaded`)
  row.config = config
  const fiber = row.inner
  // A gate between two plugin fibers (its inject went and came back)
  // re-applies from row.config.
  if (!fiber || fiber.uid === null) return null
  // Not running (waiting for its own inject, or failed): store the config
  // in the fiber, which activates from it.
  if (fiber.state !== ACTIVE) {
    fiber.update(config, true)
    return null
  }
  if (commitVolatile(fiber, config)) return null
  fiber.update(config, true)
    await fiber.await()
  return null
}

// Whether the change was committed in place. Throws when the config does
// not validate.
function commitVolatile(fiber, raw) {
  const { volatileEntries, updateVolatile, deepEqual } = cosmokit
  if (!volatileEntries || !resolveConfig) return false
  const refs = volatileEntries(fiber.config)
  if (!refs.length) return false
  const candidate = resolveConfig(fiber.runtime, fiber.ctx.waterfall(fiber, 'internal/config', raw, () => raw))
  if (!deepEqual(fiber.config, candidate, true)) return false
  fiber._config = raw
  const paths = refs.flatMap(({ path, ref }) => {
    const source = path.reduce((value, key) => Reflect.get(value, key), candidate)
    if (deepEqual(ref.get(), source.get(), true)) return []
    updateVolatile(ref, source)
    return [path]
  })
  if (paths.length) {
    const self = Object.create(fiber.ctx)
    // Only the row's own fiber hears it.
    self[Context.filter] = owner => owner.fiber === fiber
    fiber.ctx.emit(self, 'loader/volatile-update', paths)
  }
  return true
}

// A rutis service seen from Cordis: bound methods call the Rust host
// (synchronously or returning a Promise, as declared); any other method is
// reported as not provided rather than silently missing. It is not an
// instance of the class the plugin declares (boundary rule 7). `id` is the
// service's id (scopedId), which its calls target.
function hostProxy(id, methods) {
  const name = id
  const target = {}
  for (const [method, kind] of Object.entries(methods)) {
    target[method] = kind === 'async'
      ? (...args) => peer.callAsync(`host:${id}`, method, args)
      : (...args) => peer.call(`host:${id}`, method, args)
  }
  const passthrough = new Set(['then', 'toJSON', 'constructor'])
  return new Proxy(target, {
    get(target, property, receiver) {
      if (typeof property !== 'string' || property in target || passthrough.has(property)) return Reflect.get(target, property, receiver)
      return () => { throw new Error(`${name}.${property} is not provided by the rutis host`) }
    },
  })
}

// How each forwarded event is being dispatched, from `internal/dispatch`,
// which Cordis emits before it calls the listeners. The record follows the
// dispatch's own execution (a serial or waterfall may reach the forwarding
// listener after an await), and other dispatches running meanwhile keep
// theirs: event name -> { type, values }. `values` is the dispatch's own
// argument array, which a waterfall extends with its `next` before calling
// the listeners.
const dispatches = new AsyncLocalStorage()
const sameValues = (a, b) => a.length === b.length && a.every((value, index) => Object.is(value, b[index]))

// The forwarding listener of a Cordis event: the rutis listeners as one
// group (boundary rule 4). `emit` ignores the returned Promise (fire and
// forget); parallel / serial wait for the rutis listeners. A synchronous
// bail cannot wait for them, so it fails; in a waterfall the rutis group is
// one asynchronous link that calls `next` once when it is done. A listener
// before this one dispatching the same event again with the same arguments
// leaves that dispatch's record, so a bail then gets the Promise (#340).
function forward(name, values) {
  const record = dispatches.getStore()?.get(name)
  const type = record && sameValues(record.values, values) ? record.type : 'emit'
  if (type === 'bail') {
    throw new Error(`event ${name} is forwarded to rutis, whose listeners run asynchronously: a synchronous bail cannot wait for them (use serial)`)
  }
  const next = type === 'waterfall' ? values.at(-1) : undefined
  const done = peer.callAsync('', 'event', [name, next ? values.slice(0, -1) : values])
  if (next) return done.then(() => next())
  done.catch(() => {}) // an ignored emit must not become an unhandled rejection
  return done
}

// A mount is a group of plugins sharing one Context, so dependencies between
// them resolve natively. `args.plugins` lists [{ entry, config }] in load
// order; a single-plugin mount passes only `args.config`.
function mount(args) {
  // Boundary rule 5: Cordis's own events stay on the Cordis side.
  refuseInternal('emits', args.emits)
  refuseInternal('events', args.events)
  if (fibers) throw new Error('plugins are already mounted')
  fibers = []
  for (const [name, methods] of Object.entries(args.services ?? {})) {
    if (name.includes('#')) throw new Error(`service name ${name} cannot be projected`)
    slots.set(name, { name, methods: new Set(methods), scope: undefined, object: undefined, identity: undefined, handle: null, generation: 0, version: 0 })
  }
  const plugins = args.plugins ?? [{ entry: pluginPath, config: args.config }]
  emits = new Set(args.emits ?? [])
  announce = args.announce === true
  // Services the rutis application provides are registered before the
  // plugins load, so the plugins' dependencies on them resolve natively.
  for (const [name, methods] of Object.entries(args.provided ?? {})) ctx.provide(name, hostProxy(name, methods))
  return (async () => {
    for (const { entry } of plugins) {
      const own = cordisOf(entry)
      if (cordisPath && own && own !== cordisPath) {
        throw new Error(`${entry} resolves a different Cordis (${own}) than ${pluginPath} (${cordisPath})`)
      }
    }
    for (const { entry, config } of plugins) {
      fibers.push(ctx.plugin(await pluginOf(entry), config))
    }
    for (const [name, slot] of slots) slot.exporter = exporter(name, slot)
    await Promise.all([...fibers, ...[...slots.values()].map(slot => slot.exporter)].map(fiber => fiber.await()))
    // Only this group runs in the process, and rutis cannot provide services
    // to it yet, so a dependency unresolved within the group never resolves.
    const unresolved = fibers.flatMap((fiber, index) => fiber.store ? [] : [
      `${plugins[index].entry} (${Object.keys(fiber.inject ?? {}).filter(name => ctx.get(name, false) === undefined).join(', ') || 'unknown'})`,
    ])
    if (unresolved.length) throw new Error(`native plugin dependencies are unresolved: ${unresolved.join('; ')}`)
    refresh()
    mounted = true
    // Forward selected Cordis events to rutis. Registered after the plugins
    // started, so the rutis side runs as one group after the listeners the
    // plugins registered while starting (boundary rules 3 and 4).
    const forwarded = new Set(args.events ?? [])
    if (forwarded.size) {
      ctx.on('internal/dispatch', (type, name, values) => {
        if (forwarded.has(name)) dispatches.enterWith(new Map(dispatches.getStore()).set(name, { type, values }))
      })
    }
    for (const name of forwarded) ctx.on(name, (...values) => forward(name, values))
    return { services: Object.fromEntries([...slots].map(([name, slot]) => [name, [slot.handle, slot.version]])), features: FEATURES }
  })()
}

function dispatch(target, method, args) {
  if (closing) throw new Error('plugin is closing')
  if (target === '') {
    switch (method) {
      case 'mount': return mount(args)
      case 'dispose':
        closing = true
        return Promise.all([dispose(), peer.drain()]).then(() => null)
      case 'emit': {
        // A rutis event re-emitted for the Cordis listeners.
        const [name, values] = args ?? []
        if (!emits.has(name)) throw new Error(`event ${name} is not declared for emission from rutis`)
        return ctx.parallel(name, ...(values ?? []))
      }
      case 'get': {
        // A live read of a declared service property.
        const [handle, property] = args ?? []
        const entry = handles.get(handle)
        if (!entry) throw new Error(`unknown or released service object ${handle}`)
        if (!slots.get(entry.name).methods.has(property)) throw new Error(`unknown service property ${entry.name}.${property}`)
        return entry.object[property]
      }
      case 'rows.load': return loadRow(args ?? [])
      case 'rows.update': return updateRow(args ?? [])
      case 'rows.unload': return unloadRow(args?.[0])
      case 'rows.schema': return describe(args?.[0])
      case 'hosts.provide': {
        // A rutis service, registered for the rows that use it; rutis counts
        // the users and withdraws it after the last one. With a label, only
        // rows isolating the name with that label see it.
        const [name, methods, label] = args ?? []
        const id = scopedId(name, label)
        if (hosts.has(id)) throw new Error(`host service ${id} is already provided`)
        const scope = label ? ctx.isolate(name, isolated(name, label)) : ctx
        hosts.set(id, scope.provide(name, hostProxy(id, methods ?? {})))
        return null
      }
      case 'hosts.withdraw': {
        const withdraw = hosts.get(args?.[0])
        hosts.delete(args?.[0])
        return withdraw ? Promise.resolve(withdraw()).then(() => null) : null
      }
      case 'inspect': {
        if (!INSPECT) throw new Error(`unknown control method ${method}`)
        let live = 0
        for (const runtime of ctx.registry.values()) live += runtime.fibers.length
        return {
          rows: [...rows.keys()],
          hosts: [...hosts.keys()],
          slots: [...slots.keys()],
          handles: [...handles.keys()],
          plugins: ctx.registry.size,
          fibers: live,
          services: Reflect.ownKeys(ctx.reflect.store).length,
        }
      }
      case 'release': {
        const entry = handles.get(args?.[0])
        if (entry) { entry.released = true; if (!entry.current) handles.delete(args[0]) }
        return null
      }
      default: throw new Error(`unknown control method ${method}`)
    }
  }
  const entry = handles.get(target)
  if (!entry) throw new Error(`unknown or released service object ${target}`)
  if (!slots.get(entry.name).methods.has(method)) throw new Error(`unknown service method ${entry.name}.${method}`)
  if (!Array.isArray(args)) throw new TypeError('method arguments must be an array')
  let result
  try {
    result = Reflect.apply(entry.object[method], entry.object, args)
  } finally {
    // Also after a throw: the method may have replaced the service first.
    if (!(result instanceof Promise)) refresh()
  }
  if (result instanceof Promise) result.then(refresh, refresh)
  return result
}

// Calls on exported objects and functions may replace services too.
try {
  peer = await Process.connect(channelSpec, dispatch, () => { if (slots.size && !closing) refresh() }, endpoint)
} catch (error) {
  // The supervisor ended the session before it greeted, and said why.
  if (dialed) process.exit(1)
  throw error
}
if (dialed) {
  // Stopped: the rows go while rutis still hears from this end, then the
  // session ends (and so does this process, below).
  const stop = () => {
    if (closing) return
    closing = true
    dispose().catch(() => {}).then(() => peer.end())
  }
  process.on('SIGINT', stop)
  process.on('SIGTERM', stop)
}
// Spent once connected: what this process starts does not inherit it.
delete process.env.RUTIS_CHANNEL_TOKEN
await peer.closed()
closing = true
// A session this side ended because the far end broke the protocol is a
// failure, said on stderr and in the exit status: ending normally made the
// host report "exited normally" and nothing else (#225).
if (peer.fault) {
  console.error(`rutis runtime: the session ended: ${peer.fault.message}`)
  process.exitCode = 1
}
// The rows go now, unless the host disposed them first: it went away (it
// was killed, say) and nothing else ends this process. Cleanups that do not
// finish within the deadline are given up (exit status 1). Then the process
// exits, also when a plugin left a timer or a socket that would keep it.
const deadline = shutdownDeadline()
const late = setTimeout(() => {
  console.error(`rutis runtime: cleanups did not finish within ${deadline / 1000} s after the session ended; exiting`)
  process.exit(1)
}, deadline)
await dispose()
// What is still being written goes out first.
await Promise.all([process.stdout, process.stderr].map(stream => new Promise(resolve => stream.write('', resolve))))
clearTimeout(late)
process.exit()

// How long cleanups may take once the session ended: RUTIS_SHUTDOWN_TIMEOUT
// seconds (rutis-host passes its own deadline on), 10 by default; in ms.
function shutdownDeadline() {
  const seconds = Number(process.env.RUTIS_SHUTDOWN_TIMEOUT)
  return Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : 10_000
}
