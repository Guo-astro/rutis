import { createRequire } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { isAbsolute } from 'node:path'
import { statSync, writeSync } from 'node:fs'
import { Worker } from 'node:worker_threads'
import { Process } from './client.mjs'
import { toJsonSchema } from './schema.mjs'
import { cordisConfig } from './cordis-config.mjs'
import { isLeaf, oncePerName, supported, toCordis } from './leaf.mjs'
import { Bus, EVENTS_V2, NO_EVENTS_V2, refuseRemoved } from './events.mjs'
import { install } from './cordis-events.mjs'
import { rowTracking, install as handleUnhandled, failureText, kindText, realFile, reportText, stackFiles, unhandledError } from './unhandled.mjs'
import { cannotLoad, encodeError, entryGone, fileShown, noMethod, notDefinePlugin, serviceName } from './errors.mjs'
import { prefixOutput, say, watchBlocking } from './output.mjs'

// `<channel> --id <endpoint> [--peer <endpoint>] <first plugin or anchor>`.
// The session greets as `--id`, expecting `--peer` as the controller when
// given, on a local channel (`fd:3`, a socket path) or a network one.
// `listen:…` serves controllers one session at a time (serve.mjs);
// `dial:ws://…` / `dial:wss://…` dials a rutis access point and dials
// again (dial.mjs). Both run each session in a child runner; a dialing
// runtime's (`--supervisor dial`), on SIGINT or SIGTERM, unloads its rows
// and ends its session.
const { channelSpec, pluginPath, endpoint, flags } = (() => {
  const [channelSpec, ...rest] = process.argv.slice(2)
  const flags = {}
  while (rest[0]?.startsWith('--')) flags[rest.shift().slice(2)] = rest.shift()
  const network = /^(wss?|listen|dial):/.test(channelSpec)
  if (!flags.id && network) throw new Error(`a network channel needs --id <endpoint>: ${channelSpec}`)
  // Only a host older than 0.19 starts a local runtime without --id: one
  // line saying so, no stack.
  if (!flags.id) {
    writeSync(2, `a local channel needs --id <endpoint>: ${channelSpec}; a host older than rutis 0.19 started this runtime: upgrade the host\n`)
    process.exit(1)
  }
  // A runner is a runtime: the controller manages its rows.
  const endpoint = { local: flags.id, expected: flags.peer, declare: ['runtime'] }
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
const { Context, Fiber, Inject, Service, resolveConfig } = await import(cordisPath ? pathToFileURL(cordisPath).href : '@deepseek-ai/cordis')
// Volatile config helpers from the cosmokit that Cordis itself uses.
const cosmokit = await import(pathToFileURL(createRequire(cordisPath ?? fileURLToPath(import.meta.resolve('@deepseek-ai/cordis'))).resolve('@deepseek-ai/cosmokit')).href).catch(() => ({}))
let peer
const ctx = new Context()
let fibers
let mounted = false
let closing = false
let disposing
let version = 0
// The host takes the services rows provide without declaring them, each
// announced with its methods (mount `announce`, feature `rows.announce`).
// A host that did not ask (0.11) hears only the declared ones.
let announce = false
// Rows: plugins rutis-loader manages one by one in this Context (`rows.*`).
const rows = new Map() // key -> { fiber, inner, config, exports, loading, scope, labelOf, chain, abort }
// rutis services registered one by one (`hosts.*`): id -> withdraw
const hosts = new Map()
// Those whose name a row's plugin provided first in their scope (#682):
// rutis refuses that row's service, since the host has the name, and the
// row fails; until its service is withdrawn, the host's waits here, so
// the rows leasing it do not fail for the other row's mistake (they see
// it in a scope of their own). id -> { scope, name, proxy }
const waitingHosts = new Map()
// What this runner supports, reported by `mount`.
// rows.announce: the services a row provides (a Cordis plugin's, or a leaf
// plugin's `ctx.provide`) reach rutis without a declaration, announced with
// the methods they have (`service` with a fourth argument, to a host whose
// mount asked for it).
// plugin-values: a plugin definition this runtime handed over comes back to
// be described (`plugins.read`) and loaded as a row's entry
// (docs/design-plugin-values-2026-10-25.md §5).
const FEATURES = ['rows.v2', 'hosts', 'leaf.js', 'scopes', 'rows.announce', 'plugin-values']

// The logical event bus (#464, ./events.mjs, ./cordis-events.mjs): every
// Cordis listener that is not internal/* is registered with the host, and
// every emission runs by the bus's rules. The plugins `mount` loads are in
// its `scope`, with a signal of their own; each row has its scope chain
// (`rows.load`'s seventh argument) and signal.
let mountScope = []
const mountAbort = new AbortController()
// The row whose plugin is being created (its fiber not known yet).
let creating
// The fibers of rows unloaded, with their row: a plugin keeping its
// context past its row emits from that row, which the bus refuses (§3).
const unloaded = new WeakMap()
// Under `rutis-host dev`, which row each piece of asynchronous work is
// (#688, ./unhandled.mjs): an error it leaves unhandled fails that row.
// With RUTIS_PREFIX_OUTPUT=1 (#678, ./output.mjs) the row running is traced
// too: each line its code writes starts with its name.
const prefixing = process.env.RUTIS_PREFIX_OUTPUT === '1'
const tracking = rowTracking(process.env.RUTIS_DEV === '1', { output: prefixing })
// The bus's lines (a listener that failed in `emit`) are the runtime's own:
// never prefixed with a row's name.
const bus = new Bus({ loading: key => key === null ? !mounted : Boolean(rows.get(key)?.loading), within: tracking.within, report: line => say(`${line}\n`) })
if (prefixing) {
  prefixOutput(() => {
    const key = tracking.running()
    return key === undefined ? runtimeName() : bus.rowId(key)
  })
}
// A row's method holding this thread for 5 s is said (#678).
const blocking = watchBlocking({ runtime: () => runtimeName() })
function whose(context) {
  const owner = rowOf(context?.fiber) ?? (creating ? [creating.key, creating] : undefined)
  if (!owner) {
    const gone = goneRowOf(context?.fiber)
    if (gone) return { row: gone.key, scope: gone.chain, signal: gone.abort.signal, gone: true }
    return { row: null, scope: mountScope, signal: mountAbort.signal }
  }
  const [key, row] = owner
  return { row: key, scope: row.chain, signal: row.abort.signal }
}
function goneRowOf(fiber) {
  for (let current = fiber; current; current = current.runtime ? current.parent?.fiber : undefined) {
    const row = unloaded.get(current)
    if (row) return row
  }
}
install(ctx, Context, bus, whose)
// A fiber's apply and effects run as its row's: also those Cordis starts
// later, from another row's code (a gated row whose inject came, say).
if (tracking.traced && typeof Fiber?.prototype?._execute === 'function') {
  const execute = Fiber.prototype._execute
  Fiber.prototype._execute = function (runner) {
    const row = rowOf(this)?.[0] ?? (creating ? creating.key : undefined)
    return tracking.within(row, () => execute.call(this, runner))
  }
}
// How long an unloading row's listeners are waited for (§7.1).
const deadline = shutdownDeadline()
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
// A host service waits for a name in any scope, an isolated one included,
// whose changes the root's listener above does not hear (Cordis filters
// internal/service by isolation): this one hears them all.
ctx.on('internal/service', () => provideWaitingLater(), { global: true })

// What holds `name` in `scope`, if anything does.
function holder(scope, name) {
  const key = scope[Context.isolate][name]
  return key ? ctx.reflect.store[key] : undefined
}

// Who provides `name` in `scope`: a row (its name), or the host.
function providerOf(scope, name) {
  const impl = holder(scope, name)
  const row = impl && rowOf(impl.fiber)
  return row ? bus.rowId(row[0]) : 'the host'
}

// Whether a row's plugin (or a fiber it started) holds `name` in `scope`.
function takenByRow(scope, name) {
  const impl = holder(scope, name)
  return Boolean(impl && rowOf(impl.fiber))
}

// Register the host services whose name was taken once it is free again
// (the row unloading may have left `rows` before its service goes).
// Cordis emits internal/service from inside its own bookkeeping: they are
// registered once it is done.
let providingWaiting = false
function provideWaitingLater() {
  if (providingWaiting || !waitingHosts.size) return
  providingWaiting = true
  queueMicrotask(() => {
    providingWaiting = false
    for (const [id, { scope, name, proxy }] of [...waitingHosts]) {
      if (holder(scope, name)) continue
      waitingHosts.delete(id)
      try { hosts.set(id, scope.provide(name, proxy)) } catch (error) {
        say(`runtime node: cannot register host service ${serviceName(id)}: ${error?.message ?? error}\n`)
      }
    }
  })
}
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
    for (const [key, row] of [...rows].reverse()) {
      bus.withdraw(key)
      row.abort.abort(new DOMException('the row is unloading', 'AbortError'))
      await bus.settle(key, deadline)
      await row.fiber.dispose()
    }
    rows.clear()
    bus.withdraw(null)
    mountAbort.abort(new DOMException('the plugins are unloading', 'AbortError'))
    await bus.settle(null, deadline)
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

// A leaf plugin providing a service it does not declare, under a host that
// mounted without `announce` (one older than 0.12): it is told, once per
// name, that the host does not take it.
const tellOldHost = oncePerName(text => say(text))
const undeclared = name => {
  if (fibers && !announce) tellOldHost(name)
}

// `rowInject`: the names the row's own `inject` adds, which a leaf plugin
// may use too.
async function pluginOf(entry, rowInject = []) {
  entry = located(entry)
  const declared = await declaredOf(entry, described)
  return isLeaf(declared) ? toCordis(declared, undeclared, rowInject, providerOf) : declared
}

// A plugin definition this runtime handed over, back as a row's entry (a
// copy) or to be described: a leaf plugin, or a Cordis plugin by Cordis's
// own rule, a function or a class, or an object with `apply`. A leaf one is
// wrapped once per row `inject` (`rowInject`), so its copies are one plugin
// in Cordis.
const isCordisPlugin = value => typeof value === 'function' || (value !== null && typeof value === 'object' && typeof value.apply === 'function')
const wrappedDefinitions = new WeakMap()
function definedPlugin(value, rowInject = []) {
  if (isLeaf(value)) {
    let wrappings = wrappedDefinitions.get(value)
    if (!wrappings) wrappedDefinitions.set(value, wrappings = new Map())
    const key = rowInject.join('\0')
    let wrapped = wrappings.get(key)
    if (!wrapped) {
      wrapped = toCordis(supported(value, value.name ?? 'plugin definition'), undeclared, rowInject, providerOf)
      wrappings.set(key, wrapped)
    }
    return wrapped
  }
  if (isCordisPlugin(value)) return value
  throw new Error('not a plugin definition: expected a definePlugin result, a function, a class, or an object with apply')
}

// What a plugin definition declares (`plugins.read`), as `describe` says of
// a module, with its name. Answered at once: the call may come while this
// process waits on a synchronous call.
function describeDefinition(value) {
  if (isLeaf(value)) {
    supported(value, value.name ?? 'plugin definition')
    return { name: value.name ?? null, config: value.config ?? null, inject: value.inject, provides: value.provides, announces: value.announces === true }
  }
  if (!isCordisPlugin(value)) throw new Error('not a plugin definition: expected a definePlugin result, a function, a class, or an object with apply')
  const schema = value.Config ?? value.schema
  const name = typeof value.name === 'string' && value.name !== '' && value.name !== 'apply' ? value.name : null
  return {
    name,
    config: schema ? toJsonSchema(schema) : null,
    inject: Object.keys(Inject.resolve(value.inject)),
    // A value has no package.json: what it provides is announced.
    provides: {},
    announces: true,
    cordis: true,
  }
}

// The module's plugin as written: a leaf plugin (`definePlugin`), or a
// Cordis plugin. `version` picks the module URL: `fresh` describes the file
// as it is now, `described` loads the module a row was described with.
async function declaredOf(entry, version = fresh) {
  const own = cordisOf(entry)
  if (cordisPath && own && own !== cordisPath) {
    throw new Error(`${fileShown(entry)} uses its own copy of @deepseek-ai/cordis (${fileShown(own)}), not the one ${fileShown(pluginPath)} uses (${fileShown(cordisPath)}): install a single copy`)
  }
  // An entry that was there and is no more is said so, not loaded from
  // the module cache (§8.1).
  if (stamps.has(entry) && !existing(entry)) throw entryGone(entry)
  const href = version(entry)
  let module
  try {
    module = await import(href)
  } catch (error) {
    // Said of the file that threw (a submodule, say), not of the entry.
    throw cannotLoad(error, entry)
  }
  const plugin = supported(pluginIn(module, entry), entry)
  imported.set(entry, href)
  return plugin
}

const existing = file => { try { statSync(file); return true } catch { return false } }

// The plugin a module declares: its own `apply` (a Cordis plugin written
// as a module), or its default export. A default export that is neither a
// definePlugin result nor a Cordis plugin is said to be so, at once; a
// plain object with `apply` is a Cordis plugin to Cordis, so the plugin
// written as a leaf one without definePlugin is told when its apply uses
// what Cordis's context does not have (`ctx.use`, say) (#680).
function pluginIn(module, entry) {
  if (typeof module.apply === 'function') return module
  const plugin = module.default
  if (isLeaf(plugin) || typeof plugin === 'function') return plugin
  if (plugin === null || typeof plugin !== 'object' || typeof plugin.apply !== 'function') throw notDefinePlugin(entry)
  const prototype = Object.getPrototypeOf(plugin)
  if (prototype !== Object.prototype && prototype !== null) return plugin
  let wrapped = guardedDefaults.get(plugin)
  if (wrapped) return wrapped
  const apply = plugin.apply
  const translate = error => {
    if (error instanceof Error && /^cannot get property "[^"]*" without inject$/.test(error.message)) throw Object.assign(notDefinePlugin(entry), { cause: error })
    throw error
  }
  wrapped = Object.create(prototype, Object.getOwnPropertyDescriptors(plugin))
  Object.defineProperty(wrapped, 'apply', {
    value(...args) {
      let result
      try { result = Reflect.apply(apply, this, args) } catch (error) { translate(error) }
      return result && typeof result.then === 'function' ? result.then(value => value, translate) : result
    },
    configurable: true, writable: true, enumerable: true,
  })
  guardedDefaults.set(plugin, wrapped)
  return wrapped
}
const guardedDefaults = new WeakMap()

// The module URL to describe: the plain one the first time, and one that
// bypasses the module cache once the file changed since (rutis-loader's
// reload describes the row again, asking for the new code). Only the
// plugin's own module is imported again, not the modules it imports, as for
// Python plugins.
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

// The module URL a row loads. Under `rutis-host dev` (RUTIS_DEV=1), the last
// one imported as a plugin without failing, which its description imported:
// a row that loads again for another reason (a service it uses came back,
// its config changed) runs the code it was described with, not an edit made
// since, which may not even import; the edit runs once a reload describes it
// (#665). The file as it is now when this process imported none of it yet,
// and always outside `dev`, where an edited file is what a restart runs.
const imported = new Map()
const development = process.env.RUTIS_DEV === '1'
function described(entry) {
  return (development && imported.get(entry)) || fresh(entry)
}

// `plugin()` returns a thenable `Object.create(fiber)`; state written through
// it (`update` sets `_config`) would land on the wrapper and never reach the
// fiber that later activates. Rows keep the fiber itself.
const fiberOf = wrapped => Object.hasOwn(wrapped, 'then') ? Object.getPrototypeOf(wrapped) : wrapped

// One row: `isolate` as [name, label] pairs (rows naming a label share its
// scope), `inject` as extra service names gating the row. With `inject`, the
// plugin runs inside a gate fiber (`inner`), from the row's latest config.
async function loadRow([key, entry, config, isolate, inject, exports, place]) {
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
  // A module's path, or a plugin definition this runtime handed over.
  const plugin = typeof entry === 'string' ? await pluginOf(entry, inject ?? []) : definedPlugin(entry, inject ?? [])
  cordisConfig(plugin, config)
  let scope = ctx
  for (const [name, label] of isolate ?? []) {
    scopedId(name, label)
    scope = scope.isolate(name, isolated(name, label))
  }
  const row = { key, plugin, fiber: undefined, inner: undefined, config, exports: ids, loading: true, scope, labelOf, chain: chainOf(place?.scope), abort: new AbortController() }
  // Its plugin's file, by which a call of process.exit names it (#688).
  row.file = typeof entry === 'string' ? realFile(located(entry)) : undefined
  // An error the row leaves unhandled while it loads fails the load (#688):
  // also one that comes once the load is done but not yet answered.
  let settled = false
  const failing = new Promise((_, reject) => {
    row.failLoad = (kind, thrown) => {
      const error = unhandledError(kind, thrown)
      if (settled) row.lateFailure ??= error
      else reject(error)
      return true
    }
  })
  bus.nameRow(key, place?.id)
  let fiber
  creating = row
  try {
    fiber = fiberOf(inject?.length
      ? scope.plugin({ name: `row:${key}`, inject, apply(gated) { row.inner = fiberOf(gated.plugin(plugin, row.config)) } })
      : scope.plugin(plugin, config))
  } finally { creating = undefined }
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
  const loading = (async () => {
    await fiber.await()
    // A gated row: the gate applied, and its plugin runs in a fiber of its
    // own, whose failure the gate does not share. The row waits for it, so
    // it fails with its plugin's error as an ungated row does (#584). (A
    // gate still waiting for its inject has no plugin fiber yet: the row
    // loads, as an ungated row waiting for its plugin's inject does.)
    if (row.inner !== fiber) await row.inner?.await()
    // What the row provides without declaring it is announced before the
    // reply, as the declared services are.
    discover()
    const announced = row.exports.map(id => slots.get(id)).filter(slot => slot?.row === key)
    await Promise.all(announced.map(slot => slot.exporter.await()))
    // rutis has published them (or refused one, which fails the row there)
    // before it hears that the row loaded; so are the listeners it
    // registered (§4.4 2).
    await Promise.all(announced.map(slot => slot.announcing))
    await Promise.all(bus.registrations(key))
  })()
  // Failed meanwhile: what is left of the load settles unobserved.
  loading.catch(() => {})
  // Before the race hears it: a failure from then on is a late one.
  loading.then(() => { settled = true }, () => { settled = true })
  try {
    await Promise.race([loading, failing])
    if (row.lateFailure) throw row.lateFailure
  } catch (error) {
    row.failLoad = undefined
    await unloadRow(key)
    throw error
  }
  row.failLoad = undefined
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
  // Its listeners are off the bus at once; those running are signalled
  // and waited for, at most the shutdown deadline (§7.1).
  bus.withdraw(key)
  rows.delete(key)
  for (const fiber of [row.fiber, row.inner]) if (fiber) unloaded.set(fiber, row)
  for (const id of row.exports) {
    const slot = slots.get(id)
    await slot?.exporter?.dispose()
    if (slot) retire(slot.handle)
    slots.delete(id)
  }
  row.abort.abort(new DOMException('the row is unloading', 'AbortError'))
  await bus.settle(key, deadline)
  await row.fiber.dispose()
  bus.forgetRow(key)
  // What it provided is gone: a host service waiting for the name takes it.
  provideWaitingLater()
  return null
}

// A scope chain as rows.load and mount give it: [{ id, group }], root [].
function chainOf(scope) {
  if (scope === undefined || scope === null) return []
  if (!Array.isArray(scope) || scope.some(item => typeof item?.id !== 'string' || typeof item?.group !== 'string')) throw new TypeError('a scope is a list of { id, group }')
  return scope.map(({ id, group }) => ({ id, group }))
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
  cordisConfig(row.plugin, config)
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
  // A restart is an unload and a load, as in every runtime: the row's
  // listeners leave (here and on the host), those running are signalled
  // and waited for (§7.1); then the plugin applies again, and the update
  // is answered once its new listeners are registered (§4.3, §4.4 2).
  bus.withdraw(key, { restart: true })
  row.abort.abort(new DOMException('the row is unloading', 'AbortError'))
  await bus.settle(key, deadline)
  row.abort = new AbortController()
  row.loading = true
  try {
    fiber.update(config, true)
    await fiber.await()
    await Promise.all(bus.registrations(key))
  } finally {
    row.loading = false
  }
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

// A host's service seen from Cordis: bound methods call the host
// (synchronously or returning a Promise, as declared); any other method
// fails as one the service does not have, rather than silently missing. It is not an
// instance of the class the plugin declares (boundary rule 7). `id` is the
// service's id (scopedId), which its calls target.
function hostProxy(id, methods) {
  const name = serviceName(id)
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
      return () => { throw noMethod(name, property, Object.keys(methods)) }
    },
  })
}

// A mount is a group of plugins sharing one Context, so dependencies between
// them resolve natively. `args.plugins` lists [{ entry, config }] in load
// order; a single-plugin mount passes only `args.config`.
function mount(args) {
  // Listeners register themselves (#464): the static forwarding is gone.
  refuseRemoved(args)
  if (fibers) throw new Error('plugins are already mounted')
  mountScope = chainOf(args.scope)
  // This runtime's name on the host, for descriptors, when it says.
  if (typeof args.runtime === 'string') bus.runtime = args.runtime
  fibers = []
  for (const [name, methods] of Object.entries(args.services ?? {})) {
    if (name.includes('#')) throw new Error(`service name ${name} cannot be projected`)
    slots.set(name, { name, methods: new Set(methods), scope: undefined, object: undefined, identity: undefined, handle: null, generation: 0, version: 0 })
  }
  const plugins = args.plugins ?? [{ entry: pluginPath, config: args.config }]
  announce = args.announce === true
  // Services the rutis application provides are registered before the
  // plugins load, so the plugins' dependencies on them resolve natively.
  for (const [name, methods] of Object.entries(args.provided ?? {})) ctx.provide(name, hostProxy(name, methods))
  return (async () => {
    for (const { entry } of plugins) {
      const own = cordisOf(entry)
      if (cordisPath && own && own !== cordisPath) {
        throw new Error(`${fileShown(entry)} uses its own copy of @deepseek-ai/cordis (${fileShown(own)}), not the one ${fileShown(pluginPath)} uses (${fileShown(cordisPath)}): install a single copy`)
      }
    }
    for (const { entry, config } of plugins) {
      const plugin = await pluginOf(entry)
      cordisConfig(plugin, config)
      fibers.push(ctx.plugin(plugin, config))
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
    // The listeners the plugins registered are on the host's bus first.
    await Promise.all(bus.registrations(null))
    mounted = true
    return { services: Object.fromEntries([...slots].map(([name, slot]) => [name, [slot.handle, slot.version]])), features: FEATURES }
  })()
}

// A call runs as the row whose service it calls; a control call as no
// row's, even arriving while a row's synchronous call waits (#688).
function dispatch(target, method, args) {
  if (!tracking.traced) return dispatchCall(target, method, args)
  return tracking.within(target === '' ? undefined : serviceRow(target), () => dispatchCall(target, method, args))
}

// The row whose service the object `handle` is, if a row's.
function serviceRow(handle) {
  const id = handles.get(handle)?.name
  if (id === undefined) return undefined
  const slot = slots.get(id)
  if (slot?.row !== undefined) return slot.row
  for (const [key, row] of rows) if (row.exports.includes(id)) return key
  return undefined
}

function dispatchCall(target, method, args) {
  if (closing) throw new Error('plugin is closing')
  if (target === '') {
    switch (method) {
      case 'mount': return mount(args)
      case 'dispose':
        closing = true
        return Promise.all([dispose(), peer.drain()]).then(() => null)
      // The logical bus (#464): `events.elsewhere` runs as its frame
      // arrives (the session's `inline`); `events.call` runs a listener.
      case 'events.elsewhere': bus.elsewhere(args?.[0] ?? {}); return undefined
      case 'events.call': return bus.call(args?.[0] ?? {})
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
      case 'plugins.read': return describeDefinition(args?.[0])
      case 'hosts.provide': {
        // A rutis service, registered for the rows that use it; rutis counts
        // the users and withdraws it after the last one. With a label, only
        // rows isolating the name with that label see it.
        const [name, methods, label] = args ?? []
        const id = scopedId(name, label)
        if (hosts.has(id) || waitingHosts.has(id)) throw new Error(`host service ${id} is already provided`)
        const scope = label ? ctx.isolate(name, isolated(name, label)) : ctx
        const proxy = hostProxy(id, methods ?? {})
        // A row's plugin provided the name here first: the host's waits
        // for it to go (see `waitingHosts`).
        if (takenByRow(scope, name)) waitingHosts.set(id, { scope, name, proxy })
        else hosts.set(id, scope.provide(name, proxy))
        return null
      }
      case 'hosts.withdraw': {
        waitingHosts.delete(args?.[0])
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
          hosts: [...hosts.keys(), ...waitingHosts.keys()],
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
  if (!slots.get(entry.name).methods.has(method)) throw noMethod(serviceName(entry.name), method, slots.get(entry.name).methods)
  if (!Array.isArray(args)) throw new TypeError('method arguments must be an array')
  let result
  try {
    const row = serviceRow(target)
    result = blocking.held(row === undefined ? runtimeName() : bus.rowId(row), `${serviceName(entry.name)}.${method}`, () => Reflect.apply(entry.object[method], entry.object, args))
  } finally {
    // Also after a throw: the method may have replaced the service first.
    if (!(result instanceof Promise)) refresh()
  }
  if (result instanceof Promise) result.then(refresh, refresh)
  return result
}

// A watchdog on a thread of its own: once the channel ended, this process
// unloads its rows and exits within the shutdown deadline (below). A
// plugin keeping this thread busy keeps it from both, and from even seeing
// that the session ended, which the I/O thread sees: a second after the
// deadline, counted from when the I/O thread saw the end, the watchdog
// ends the process, so it never outlives its host. It sleeps until then
// (the wait's timeout is the clock), and process.exit ends it as any other.
const channelEnded = new Int32Array(new SharedArrayBuffer(4))
watch(channelEnded, deadline + 1000)
function watch(ended, after) {
  const watchdog = new Worker(`
    const { workerData: { ended, after } } = require('node:worker_threads')
    const { writeSync } = require('node:fs')
    Atomics.wait(ended, 0, 0)
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, after)
    try {
      writeSync(2, 'runtime node: the host disconnected ' + after / 1000 + ' s ago and this process has not exited (a plugin keeps it busy); ending it\\n')
    } catch {}
    process.kill(process.pid, 'SIGKILL')
  `, { eval: true, workerData: { ended, after } })
  watchdog.unref()
  watchdog.on('error', error => say(`runtime node: the watchdog failed: ${error?.message ?? error}\n`))
}

// Errors nobody handled (#688, ./unhandled.mjs): the row they are traced
// to fails (rutis-host restarts it, and the restart fails with why); one
// traced to no row is reported here. The process goes on either way.
const runtimeName = () => bus.runtime ?? 'node'
function charge(kind, error, traced) {
  const key = chargedRow(traced, error)
  const row = key === undefined ? undefined : rows.get(key)
  if (!row || closing || !peer) {
    say(`runtime ${runtimeName()}: an unhandled error from an unknown row: ${reportText(error)}\n`)
    return
  }
  const why = failureText(kind, error)
  if (row.failLoad) { row.failLoad(kind, error); return }
  // Once: the row is on its way out; what else it leaves unhandled until
  // it has gone is written here.
  if (row.failing) {
    say(`runtime ${runtimeName()}: row ${bus.rowId(key)}: ${why}\n`)
    return
  }
  row.failing = true
  // What it was, and the error as a thrown one crosses: its code, data
  // and stack, which the host prints under it.
  let encoded
  try { encoded = encodeError(error) } catch {}
  peer.callAsync('', 'rows.failed', encoded ? [key, kindText(kind, error), encoded] : [key, kindText(kind, error)]).catch(() => {
    // A host older than 0.18 fails no row so: the row goes on.
    say(`runtime ${runtimeName()}: row ${bus.rowId(key)}: ${why}\n`)
  })
}
// The row an error traced to row `key` is charged to: that row, unless the
// innermost plugin file on the error's stack is another row's only. Then
// it came from code of a row this one called directly, in this process,
// and which of the two left it unhandled cannot be told (the stack says
// where it was thrown, not who dropped it): no row is charged.
function chargedRow(key, error) {
  if (key === undefined) return undefined
  for (const file of stackFiles(error?.stack)) {
    const owners = [...rows].filter(([, row]) => row.file === file)
    if (!owners.length) continue
    return owners.some(([owner]) => owner === key) ? key : undefined
  }
  return key
}

// The id of the row calling process.exit: the row running, else the one
// row whose plugin file is on the stack; undefined when it cannot tell.
function exitingRow(key, files) {
  if (key !== undefined && rows.has(key)) return bus.rowId(key)
  const found = [...rows].filter(([, row]) => row.file !== undefined && files.has(row.file))
  return found.length === 1 ? bus.rowId(found[0][0]) : undefined
}

// Calls on exported objects and functions may replace services too.
try {
  peer = await Process.connect(channelSpec, dispatch, () => { if (slots.size && !closing) refresh() }, endpoint, {
    // The channel's end, seen by the watchdog.
    ended: channelEnded,
    // `plugins.read` only reads: answered even while a synchronous call
    // waits (a definition handed to a synchronous host method).
    inline: ['events.elsewhere', 'plugins.read'],
    // An `emit`-mode `events.call` waits while a synchronous call of this
    // process does, as every call not of its chain does; a local emit of
    // the name goes behind it (the bus's `behind`, §5.5).
    holds: (method, args) => method === 'events.call' && args?.[0]?.mode === 'emit' ? args[0].name : undefined,
    requires: [EVENTS_V2],
    report: bus.report,
    // A function or object a row hands over runs as that row's when
    // called (#688).
    owner: tracking.running,
    within: tracking.within,
  })
} catch (error) {
  // A host older than 0.13 (protocol §2.2).
  if (error?.message === NO_EVENTS_V2) {
    say(`rutis: refused: incompatible (${NO_EVENTS_V2}); stopping\n`)
    process.exit(1)
  }
  // The supervisor ended the session before it greeted, and said why.
  if (dialed) process.exit(1)
  throw error
}
// Installed once connected: a failure before that is the runtime's own,
// and ends it.
handleUnhandled(tracking, {
  charge,
  // Written before the process exits, as Node exits at once.
  exiting: (key, files, code) => {
    const row = exitingRow(key, files)
    if (row !== undefined) writeSync(2, `runtime ${runtimeName()}: row ${row} called process.exit(${code})\n`)
  },
})
bus.peer = {
  call: (target, method, args) => peer.call(target, method, args),
  callAsync: (target, method, args) => peer.callAsync(target, method, args),
  callAnswered: (target, method, args, answered) => peer.callAnswered(target, method, args, answered),
  callFunctionAsync: (fn, args) => peer.callFunctionAsync(fn, args),
  behind: (name, run) => peer.behind(name, run),
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
bus.ended()
// A session this side ended because the far end broke the protocol is a
// failure, said on stderr and in the exit status: ending normally made the
// host report "exited normally" and nothing else (#225).
if (peer.fault) {
  say(`runtime node: disconnected from the host: ${peer.fault.message}\n`)
  process.exitCode = 1
}
// The rows go now, unless the host disposed them first: it went away (it
// was killed, say) and nothing else ends this process. Cleanups that do not
// finish within the deadline are given up (exit status 1). Then the process
// exits, also when a plugin left a timer or a socket that would keep it.
const late = setTimeout(() => {
  say(`runtime node: cleanups did not finish within ${deadline / 1000} s after the host disconnected; exiting\n`)
  process.exit(1)
}, deadline)
await dispose().catch(error => {
  say(`rutis runtime: unloading failed: ${error?.stack ?? error}\n`)
  process.exitCode = 1
})
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
