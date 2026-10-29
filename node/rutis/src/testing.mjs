// Test a plugin on a real host: give it the services it injects, call the
// services it provides, unload it.
//
//   const t = await load(plugin, { config: { city: 'Oslo' }, services: { llm } })
//   assert.equal(await t.service('weather').today(), 'sunny in Oslo')
//   assert.equal(await t.serial('weather/ask', 'Oslo'), 'sunny')
//   await t.unload()
//
// Several plugins on one `host()`:
//
//   const h = host({ services: { llm } })
//   const chat = await h.load(chatPlugin)
//   await h.parallel('session/created', id)
//
// The plugin runs in the real runtime (`@arcships/rutis-runtime`, or
// `@arcships/rutis-bun` under Bun) of a real host (`rutis-host test`, from
// RUTIS_HOST or PATH), started once per test process (platform cleanup
// §7.5): the host checks its config and fills its defaults, gates it on
// the services it injects, and dispatches its events; what crosses between
// it and the test crosses between processes. The test's services, its
// listeners (`on`) and what it emits are those of a plugin next to it in
// the same runtime, the test tool's own. The plugin is loaded from its
// module, which must export it as default.

import { PLUGIN_API, isPlugin } from './advanced.mjs'

export class PluginTestError extends Error {
  name = 'PluginTestError'
}

const FILE = Symbol.for('rutis.plugin-file')

// What the test serves the driver: its services' methods, by the
// service's name, and its listeners, by `listener:<id>`.
const served = new Map()
let nextListener = 0
function dispatch(target, method, args) {
  const object = served.get(target)
  if (object === undefined) throw new Error(`${target} is gone`)
  return target.startsWith('listener:') ? object(...args) : object[method](...args)
}

// The host and runtime of this process, started at the first `load`.
let started
function runtime() {
  started ??= (async () => {
    const bun = typeof globalThis.Bun !== 'undefined'
    const name = bun ? '@arcships/rutis-bun' : '@arcships/rutis-runtime'
    let start
    try {
      ({ start } = await import(`${name}/testing`))
    } catch (error) {
      if (error?.code !== 'ERR_MODULE_NOT_FOUND' && error?.code !== 'MODULE_NOT_FOUND' && !/Cannot find (module|package)/.test(error?.message)) throw error
      throw new PluginTestError(`the test tool runs plugins in ${name}: add it as a dev dependency`)
    }
    const harness = await start({ dispatch })
    harness.control.unref()
    return harness.control
  })()
  return started
}

// The driver, synchronously (the test waits) or not. While an asynchronous
// call is out, the session keeps the process running.
let control
let out = 0
// `target`: the driver itself (''), or a service by its name.
function call(method, ...args) {
  return control.call('', method, args)
}
function ask(method, ...args) {
  return askOf('', method, args)
}
async function askOf(target, method, args) {
  if (out++ === 0) control.ref()
  try {
    return await control.callAsync(target, method, args)
  } finally {
    if (--out === 0) control.unref()
  }
}

// The methods rutis sees on a service no declaration describes: its own
// and inherited functions, up to Object's, not the constructor.
function methodNames(object) {
  const names = new Set()
  for (let level = object; level != null && level !== Object.prototype && level !== Function.prototype; level = Object.getPrototypeOf(level)) {
    for (const name of Object.getOwnPropertyNames(level)) {
      if (name !== 'constructor' && typeof Object.getOwnPropertyDescriptor(level, name)?.value === 'function') names.add(name)
    }
  }
  return [...names]
}

// The test's services `services`, provided next to the plugins; the
// function that withdraws them.
async function provide(services) {
  const names = []
  try {
    for (const [name, object] of Object.entries(services)) {
      served.set(name, object)
      names.push(name)
      await ask('provide', name, methodNames(object))
    }
  } catch (error) {
    await withdraw(names)
    throw error
  }
  return () => withdraw(names)
}
async function withdraw(names) {
  for (const name of names.reverse()) {
    served.delete(name)
    await ask('withdraw', name)
  }
}

// The five ways to emit and `filter`, from the test's plugin; `keep`, the
// filter.
function emitters(keep) {
  // `<mode>(name, ...values)`, or `filter.<mode>(keep, name, ...values)`.
  const method = mode => keep ? [`filter.${mode}`, keep] : [mode]
  const sync = mode => (...args) => call(...method(mode), ...args)
  const async = mode => (...args) => ask(...method(mode), ...args)
  return {
    emit: (...args) => { sync('emit')(...args) },
    parallel: async('parallel'),
    serial: async('serial'),
    waterfall: Object.assign(async('waterfall'), {
      with: terminal => {
        if (typeof terminal !== 'function') throw new PluginTestError('waterfall.with needs a terminal function')
        return (...args) => async('waterfall.with')(terminal, ...args)
      },
    }),
    bail: sync('bail'),
    filter: predicate => {
      if (typeof predicate !== 'function') throw new PluginTestError('filter needs a predicate function')
      const { filter: _, ...five } = emitters(predicate)
      return five
    },
  }
}

// The module that exports `plugin` as default, as a real runtime loads it.
async function fileOf(plugin) {
  const file = plugin[FILE]
  let module
  try {
    module = file && await import(file)
  } catch {}
  if (module?.default !== plugin) {
    throw new PluginTestError(`load needs a plugin that its module exports as default${file ? `: ${file} does not` : ''}`)
  }
  return file
}

/**
 * Plugins on the host, sharing its bus. `services` are the test's, given
 * to every plugin that injects them.
 */
export function host({ services = {} } = {}) {
  const rows = []
  let withdrawShared
  let closed = false
  const self = {
    /** The failures the host reported of the rows here (§5.6): `{ line }`. */
    async reported() { return (await Promise.all(rows.map(row => row.api.reported()))).flat() },
    /** Load `plugin` as a row, with `config` and its own `services`. */
    async load(plugin, { config, services: own = {} } = {}) {
      if (closed) throw new PluginTestError('the host is unloaded')
      plugin = isPlugin(plugin) ? plugin : plugin?.default
      if (!isPlugin(plugin)) throw new PluginTestError('load needs a plugin made with definePlugin')
      if (plugin.api > PLUGIN_API) {
        throw new PluginTestError(`the plugin needs plugin API ${plugin.api}; this SDK supports ${PLUGIN_API}`)
      }
      const file = await fileOf(plugin)
      control = await runtime()
      withdrawShared ??= await provide(services)
      const withdrawOwn = await provide(own)
      let id
      try {
        id = await ask('load', file, config ?? null)
      } catch (error) {
        await withdrawOwn()
        throw new PluginTestError(error?.message ?? String(error))
      }
      const row = loaded(id, plugin, withdrawOwn)
      rows.push(row)
      return row.api
    },
    /** Unload every row, latest first. */
    async unload() {
      closed = true
      for (const row of [...rows].reverse()) await row.unload()
      await withdrawShared?.()
      withdrawShared = undefined
    },
    ...emitters(undefined),
  }

  function loaded(id, plugin, withdrawOwn) {
    // This row's test listeners: withdrawn when it unloads.
    const mine = new Set()
    let unloaded = false
    const shape = name => {
      if (!call('provided', id).includes(name)) throw new PluginTestError(`the plugin does not provide ${name} (now)`)
      return plugin.provides[name] ?? call('methods', name)
    }
    // Unload the row as the host does, then withdraw its services and the
    // test's listeners of it.
    const unload = async () => {
      if (unloaded) return
      unloaded = true
      rows.splice(rows.indexOf(record), 1)
      await ask('unload', id)
      for (const off of [...mine]) off()
      await withdrawOwn()
    }
    const api = {
      /** The row's id on the host. */
      row: id,
      /** The service `name` the plugin provides, as rutis sees it: its declared methods, or, undeclared, the object's methods, all async. */
      service(name) {
        if (unloaded) throw new PluginTestError('the plugin is unloaded')
        return Object.fromEntries(Object.entries(shape(name)).map(([method, kind]) => [method, kind === 'sync'
          ? (...args) => control.call(name, method, args)
          : (...args) => askOf(name, method, args)]))
      },
      /** The methods rutis sees on the service `name`: `{ method: 'sync' | 'async' }`. */
      methods: name => ({ ...shape(name) }),
      /** The names of the services the plugin provides now. */
      provided: () => call('provided', id),
      /**
       * The failures the host reported of this row (a listener failing in an
       * `emit`, a filter): `{ line }`, `line` as the host prints it.
       */
      async reported() { return unloaded ? [] : (await ask('reported', id)).map(line => ({ line })) },
      /** The host this row is loaded on. */
      host: self,
      ...emitters(undefined),
      /**
       * Listen to the event `name` as a plugin next to it does, until the
       * returned function is called or the row unloads. Options as
       * `ctx.on`: `prepend`, `global`.
       */
      on(name, listener, options) {
        if (unloaded) throw new PluginTestError('the plugin is unloaded')
        if (typeof listener !== 'function') throw new PluginTestError('on needs a listener function')
        const target = `listener:${++nextListener}`
        served.set(target, listener)
        try {
          call('on', target, name, { prepend: Boolean(options?.prepend), global: Boolean(options?.global) })
        } catch (error) {
          served.delete(target)
          throw error
        }
        const off = () => {
          if (!mine.delete(off)) return
          served.delete(target)
          call('off', target)
        }
        mine.add(off)
        return off
      },
      /** Change the row's config, as a host does. */
      async update(config) {
        if (unloaded) throw new PluginTestError('the plugin is unloaded')
        try {
          await ask('update', id, config ?? null)
        } catch (error) {
          throw new PluginTestError(error?.message ?? String(error))
        }
      },
      /** Unload the row as the host does, then withdraw its services and the test's listeners of it. */
      unload,
    }
    const record = { api, unload }
    return record
  }

  return self
}

/**
 * Load `plugin` (what definePlugin returns, or a module whose default export
 * it is) with `config` and the services in `services`, on a `host()` of its
 * own.
 */
export async function load(plugin, { config, services = {} } = {}) {
  const h = host({ services })
  const t = await h.load(plugin, { config }).catch(async error => {
    await h.unload()
    throw error
  })
  // Unloading it unloads its host too, with the test's services.
  t.unload = () => h.unload()
  return t
}
