import { createRequire } from 'node:module'
import { dirname, isAbsolute, join } from 'node:path'
import { readFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'
import { peerService } from './link.mjs'
import { toJsonSchema } from '../schema.mjs'
import { isLeaf, supported, toCordis } from '../leaf.mjs'
import { Context } from '@deepseek-ai/cordis'
import { Bus } from '../events.mjs'
import { install } from '../cordis-events.mjs'

// The node features of a Cordis application, each gated on its link's peer
// (`rutisPeer.<id>`), so each session gets them anew. They speak what the
// Rust ones do (rutis-bridge): services cross as records of functions with
// their shape, plugins are hosted by name, and events are shared as
// entries of each other's bus (docs/design-events-protocol-2026-10-18.md §9).

const fiberOf = wrapped => Object.hasOwn(wrapped, 'then') ? Object.getPrototypeOf(wrapped) : wrapped

// Each of these plugins runs a gated child: its config is static, but the
// peer it injects is named in the config.
function gated(name, peer, apply) {
  return { name, inject: [peerService(peer)], apply(ctx) { apply(ctx, ctx.get(peerService(peer))) } }
}

// export: { peer, services: { name: { method: 'sync' | 'async' } } }
// Announces the Cordis services to the peer while they are provided, and
// once the peer offers `services`; withdraws them when they go.
export const Export = {
  name: 'rutis-bridge/export',
  reusable: true,
  apply(ctx, { peer, services }) {
    ctx.plugin(gated(`export:${peer}`, peer, (scope, link) => {
      for (const [name, shape] of Object.entries(services ?? {})) {
        scope.plugin({
          name: `export:${peer}:${name}`,
          inject: [name],
          apply(one) {
            // Calls read the service when made: a live view of it.
            const record = Object.fromEntries(Object.keys(shape).map(method =>
              [method, (...args) => one.get(name)[method](...args)]))
            const announced = link.nextVersion()
            // Announced once to every offer of `services` (an importer
            // started again is a new one).
            let sentTo
            const off = link.onOffers((_families, epoch) => {
              const offered = epoch('services')
              if (offered === sentTo) return
              sentTo = offered
              if (offered === undefined) return
              // The export list names services, not their plugins: the service's
              // name stands for its plugin (this path goes with the node rows).
              link.callAsync('', 'services.announce', [{ name, service: record, shape, version: announced, plugin: name }])
                .catch(error => process.stderr.write(`rutis-bridge: cannot announce ${name} to ${peer}: ${error.message}\n`))
            })
            one.effect(() => async () => {
              off()
              // Withdrawn there before this ends; a peer gone has nothing to withdraw.
              await link.callAsync('', 'services.withdraw', [{ name, version: link.nextVersion() }]).catch(() => {})
            })
          },
        })
      }
    }))
  },
}

// import: { peer, services: [name] }
// Provides the peer's announced services as Cordis services; never replaces
// one provided here.
export const Import = {
  name: 'rutis-bridge/import',
  reusable: true,
  apply(ctx, { peer, services }) {
    const wanted = new Set(services ?? [])
    ctx.plugin(gated(`import:${peer}`, peer, (scope, link) => {
      // The newest the peer said of each name: provided at `version`
      // (`withdraw` set), or withdrawn at it. A withdrawal is kept so an
      // older announcement arriving after it does not bring the service back.
      const current = new Map()
      const unregister = link.register('services', (_target, method, [fields]) => {
        const { name, version } = fields ?? {}
        if (method === 'services.withdraw') {
          if (!wanted.has(name)) return
          const known = current.get(name)
          if (known && known.version >= version) return
          current.set(name, { version, withdraw: undefined })
          known?.withdraw?.()
          return
        }
        if (method !== 'services.announce') throw new Error(`${method} is not offered here`)
        if (!wanted.has(name)) return
        const known = current.get(name)
        if (known && known.version >= version) return
        if (!known?.withdraw && scope.get(name, false) !== undefined) {
          throw new Error(`${name} is already provided here: the import from ${peer} is refused`)
        }
        const { service, shape } = fields
        const object = Object.fromEntries(Object.keys(shape ?? {}).map(method => [method, (...args) => service[method](...args)]))
        known?.withdraw?.()
        current.set(name, { version, withdraw: scope.provide(name, object) })
      })
      scope.effect(() => () => {
        unregister()
        for (const { withdraw } of current.values()) withdraw?.()
        current.clear()
      })
    }))
  },
}

// The package.json above `file`, if any.
function manifestOf(file) {
  for (let dir = dirname(file); ; dir = dirname(dir)) {
    try { return JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')) } catch {}
    if (dirname(dir) === dir) return undefined
  }
}

// host: { peer, anchor? }
// Loads the npm plugins installed here (resolved from `anchor`, a
// package.json; the working directory's by default) for the peer. What it
// loads is its own subtree. Opening a host gives the peer the management of
// what is installed here: open it only to trusted peers.
export const Host = {
  name: 'rutis-bridge/host',
  reusable: true,
  apply(ctx, { peer, anchor }) {
    const require = createRequire(anchor ?? join(process.cwd(), 'package.json'))
    async function locate(name) {
      if (typeof name !== 'string' || name.startsWith('file:') || isAbsolute(name)) {
        throw new Error(`${name}: a host loads installed plugins only, not files`)
      }
      let file
      try { file = require.resolve(name) }
      catch { throw Object.assign(new Error(`no plugin ${name} is installed here`), { name: 'NotFound' }) }
      const module = await import(pathToFileURL(file).href)
      const declared = typeof module.apply === 'function' ? module : module.default
      supported(declared, name)
      return { plugin: isLeaf(declared) ? toCordis(declared) : declared, declared, version: manifestOf(file)?.version ?? null }
    }
    ctx.plugin(gated(`host:${peer}`, peer, (scope, link) => {
      const loaded = new Map()
      // A hosted row runs in its scope: each [name, label] of `isolate`
      // isolated (labels are the peer's, apart from other peers'), gated on
      // the services `inject` names, as the runner runs a row. Behind a
      // gate, the plugin runs in a fiber of its own (`inner`), whose
      // failure the gate does not share: `settled` waits for both, so a
      // gated plugin's failure fails the load as an ungated one's (#584).
      const start = entry => {
        entry.inner = undefined
        const fiber = entry.inject.length
          ? entry.scope.plugin({ name: `host:${peer}:${entry.key}`, inject: entry.inject, apply(gate) { entry.inner = fiberOf(gate.plugin(entry.plugin, entry.config)) } })
          : entry.scope.plugin(entry.plugin, entry.config)
        entry.fiber = fiberOf(fiber)
      }
      const settled = async entry => {
        await entry.fiber.await()
        await entry.inner?.await()
      }
      const unregister = link.register('plugins', async (_target, method, args) => {
        if (method === 'plugins.describe') {
          const { declared, version } = await locate(args[0])
          return { schema: declared?.Config ? toJsonSchema(declared.Config) : null, version, integrity: null }
        }
        if (method === 'plugins.load') {
          const [key, name, config, isolate, inject] = args
          if (loaded.has(key)) throw new Error(`${key} is already loaded`)
          const { plugin } = await locate(name)
          let rowScope = scope
          for (const [service, label] of isolate ?? []) rowScope = rowScope.isolate(service, Symbol.for(`rutis-peer:${peer}/${label}`))
          const entry = { key, plugin, config: config ?? {}, scope: rowScope, inject: inject ?? [], fiber: undefined, inner: undefined }
          start(entry)
          loaded.set(key, entry)
          await settled(entry)
          return null
        }
        if (method === 'plugins.update') {
          const [key, config] = args
          const entry = loaded.get(key)
          if (!entry) throw new Error(`${key} is not loaded`)
          // A new config restarts it, in the same scope.
          await entry.fiber.dispose()
          entry.config = config ?? {}
          start(entry)
          await settled(entry)
          return null
        }
        if (method === 'plugins.unload') {
          const entry = loaded.get(args[0])
          loaded.delete(args[0])
          await entry?.fiber.dispose()
          return null
        }
        throw new Error(`${method} is not offered here`)
      })
      scope.effect(() => () => {
        unregister()
        loaded.clear()
      })
    }))
  },
}

// The bus of a Cordis application (#464): its Context dispatches through
// it, so the listeners of linked nodes are entries of it (§9). Installed
// once per application root, before its plugins register listeners (`bus`
// installs it; the `Events` feature does when it starts).
const buses = new WeakMap()
export function bus(ctx) {
  const root = ctx.root ?? ctx
  let found = buses.get(root)
  if (!found) {
    found = new Bus()
    buses.set(root, found)
    install(root, Context, found, () => ({ row: null, scope: [], signal: undefined }))
  }
  return found
}

// An item of `export` / `import`: a name, or a prefix ending in `*`.
const itemOf = item => item.endsWith('*') ? { prefix: item.slice(0, -1) } : { name: item }

// What both an entry (`name`, a prefix when `prefix`) and `item` select
// (§9): [name, prefix] or nothing.
function narrowed(name, prefix, item) {
  if (!prefix) {
    if (item.name !== undefined) return item.name === name ? [name, false] : undefined
    return name.startsWith(item.prefix) ? [name, false] : undefined
  }
  if (item.name !== undefined) return item.name.startsWith(name) ? [item.name, false] : undefined
  if (item.prefix.startsWith(name)) return [item.prefix, true]
  if (name.startsWith(item.prefix)) return [name, true]
  return undefined
}

function selected(name, prefix, items) {
  const out = []
  for (const item of items) {
    const found = narrowed(name, prefix, item)
    if (found && !out.some(([n, p]) => n === found[0] && p === found[1])) out.push(found)
  }
  const prefixes = out.filter(([, p]) => p).map(([n]) => n)
  return out.filter(([n, p]) => p || !prefixes.some(prefix => n.startsWith(prefix)))
}

// The offer an announced id was made under, `<epoch>.<n>`, if it says.
const epochOf = id => {
  const match = /^(\d+)\.\d+$/.exec(id)
  return match ? Number(match[1]) : undefined
}

const REMOVED_EVENTS = 'the peer config field events was removed in rutis 0.13: list events: { out } in export and events: { in } in import (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)'
const REMOVED_OUT_IN = 'the Events options out / in were removed in rutis 0.13: use export / import (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)'

// events: { peer, export: [item], import: [item] } (§9)
// This application's emissions of what `export` names reach the peer's
// listeners, and its listeners of what `import` names hear the peer's: its
// own listeners matching `import` are announced to the peer (never the
// ones it imported: one hop), and the peer's announced listeners matching
// `export` become entries of its bus, called with `events.call`.
export const Events = {
  name: 'rutis-bridge/events',
  reusable: true,
  apply(ctx, config) {
    const { peer } = config
    if (Object.hasOwn(config, 'out') || Object.hasOwn(config, 'in')) throw new Error(REMOVED_OUT_IN)
    if (Object.hasOwn(config, 'events')) throw new Error(REMOVED_EVENTS)
    const exported = (config.export ?? []).map(itemOf)
    const imported = (config.import ?? []).map(itemOf)
    const events = bus(ctx)
    ctx.plugin(gated(`events:${peer}`, peer, (scope, link) => {
      // The peer's entries accepted here, by its id.
      const accepted = new Map()
      // This application's entries announced: entry -> [announced id].
      const announced = new Map()
      // Announced id -> entry.
      const byId = new Map()
      let next = 0, calls = 0, offer
      // This end's offer of `events` (the start of each id it announces,
      // `<epoch>.<n>`, so the peer tells one offer's announcements from
      // another's), and the latest offer of the peer's seen here: what the
      // peer announced under an older one is gone with it.
      let epoch = 0, seen = 0
      // The latest offer of the peer's its offers showed (`seen` can be
      // ahead of it: an announcement can come before its offer).
      let shown
      const send = (method, args) => link.callAsync('', method, args)
        .catch(error => process.stderr.write(`rutis-bridge: ${method} to ${peer} failed: ${error.message}\n`))
      const announce = entry => {
        if (entry.remote || entry.scope?.length || announced.has(entry)) return
        const ids = []
        for (const [name, prefix] of selected(entry.name, Boolean(entry.prefix), imported)) {
          const id = `${epoch}.${++next}`
          byId.set(id, entry)
          ids.push(id)
          send('events.on', [{ id, name, prefix, plugin: entry.plugin, row: entry.row, runtime: null, kind: entry.kind, prepend: false, global: entry.global }])
        }
        if (ids.length) announced.set(entry, ids)
      }
      const withdraw = entry => {
        const ids = announced.get(entry)
        if (!ids) return
        announced.delete(entry)
        for (const id of ids) byId.delete(id)
        send('events.off', [ids])
      }
      const forget = id => {
        for (const entry of accepted.get(id) ?? []) events.off(entry)
        accepted.delete(id)
      }
      // Forget the peer's entries announced under an offer before `below`
      // (an id without an epoch, from a node that does not say, is
      // replaced by id only).
      const olderThan = below => {
        for (const id of [...accepted.keys()]) {
          const of = epochOf(id)
          if (of !== undefined && of < below) forget(id)
        }
      }
      // The peer's offer `now` is seen: its older offers' entries are gone.
      const newer = now => {
        if (now <= seen) return
        seen = now
        olderThan(now)
      }
      const unregister = link.register('events', (_target, method, [argument]) => {
        if (method === 'events.on') {
          const { id, name, prefix = false, plugin = 'plugin', row = null, runtime = null, kind = 'any', global = false } = argument ?? {}
          if (typeof name !== 'string' || name.startsWith('internal/')) throw Object.assign(new Error(`event ${name} is internal to its runtime`), { name: 'BindingError' })
          // Of an offer already gone: not taken. Of a newer one than seen
          // (its announcement may come before the offer): the older
          // offers' entries are gone.
          const of = epochOf(String(id))
          if (of !== undefined) {
            if (of < seen) return undefined
            newer(of)
          }
          forget(id)
          const entries = selected(name, Boolean(prefix), exported).map(([selectedName, selectedPrefix]) => events.onRemote({
            name: selectedName, prefix: selectedPrefix, kind, global, plugin, row, runtime, node: peer, scope: [],
            invoke(mode, dispatched, args) {
              const waterfall = mode === 'waterfall'
              const values = waterfall ? args.slice(0, -1) : args
              const request = { listener: id, dispatch: `d${++calls}`, own: false, name: dispatched, mode, args: values }
              if (waterfall) request.next = args.at(-1)
              if (mode === 'bail') return link.call('', 'events.call', [request])
              return link.callAsync('', 'events.call', [request])
            },
          }))
          if (entries.length) accepted.set(id, entries)
          return undefined
        }
        if (method === 'events.off') {
          for (const id of argument ?? []) forget(id)
          return undefined
        }
        if (method === 'events.call') {
          const entry = byId.get(String(argument?.listener))
          if (!entry?.live) throw Object.assign(new Error(`listener ${argument?.listener} of ${argument?.name} is withdrawn`), { name: 'BindingError' })
          return events.call({ ...argument, listener: entry.id, own: false })
        }
        throw new Error(`${method} is not offered here`)
      })
      epoch = link.epoch?.('events') ?? 0
      const unwatch = events.watch(change => {
        if (offer === undefined) return
        if (change.added) announce(change.added)
        else withdraw(change.removed)
      })
      // Announced to each offer of `events` anew; what the peer announced
      // under an older offer (or one it withdrew) is gone.
      const off = link.onOffers((_families, epochOfPeer) => {
        const now = epochOfPeer('events')
        // The offer shown last is withdrawn: what was announced under it
        // or before goes. What a later offer announced, come before that
        // offer (the peer may send its offers after its announcements),
        // stays (#530).
        if (now === undefined) {
          if (shown !== undefined) olderThan(shown + 1)
        } else {
          shown = Math.max(shown ?? now, now)
          newer(now)
        }
        if (now === undefined || now === offer) return
        offer = now
        announced.clear()
        byId.clear()
        for (const entry of events.entries()) announce(entry)
      })
      scope.effect(() => () => {
        unregister()
        unwatch()
        off()
        for (const id of [...accepted.keys()]) forget(id)
      })
    }))
  },
}
