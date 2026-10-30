// The plugin that drives a test (`@arcships/rutis/testing`), in the real
// runtime next to the plugins under test: `rutis-host test` loads it first.
// It is a leaf plugin like any other: it uses the host's `rutis-host/test`
// to load, update and unload rows (the test's services among them, each a
// row of its own: ./service.mjs), and `ctx.use`, `ctx.on` and the five
// ways to emit for the test, which reaches it on a session of its
// own (the address in RUTIS_TEST_CONTROL, ./start.mjs). The test's services
// and listeners stay in the test's process: calling them here waits for the
// test, as a call of another process's does.
import { Process } from '../client.mjs'
import { rmSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { PLUGIN_API, USES_ANY } from '../leaf.mjs'
import { CONTROL } from './service.mjs'

// The plugin a service of the test is a row of (./service.mjs).
const SERVICE = fileURLToPath(new URL('./service.mjs', import.meta.url))

const LEAF = Symbol.for('rutis.leaf-plugin')
// The driver's methods the test calls synchronously (`@arcships/rutis/testing`).
const SYNCHRONOUS = ['provided', 'methods', 'on', 'off', 'emit', 'bail', 'filter.emit', 'filter.bail']

// The methods rutis sees on a service no declaration describes: its own
// and inherited functions, up to Object's, not the constructor; all async.
function inferred(object) {
  const shape = {}
  if (object === null || (typeof object !== 'object' && typeof object !== 'function')) return shape
  const seen = new Set()
  for (let level = object; level != null && level !== Object.prototype && level !== Function.prototype; level = Object.getPrototypeOf(level)) {
    for (const name of Object.getOwnPropertyNames(level)) {
      if (seen.has(name)) continue
      seen.add(name)
      if (name !== 'constructor' && typeof Object.getOwnPropertyDescriptor(level, name)?.value === 'function') shape[name] = 'async'
    }
  }
  return shape
}

export default Object.freeze({
  [LEAF]: true,
  // The test's calls name any service (`t.service`): declared or not.
  [USES_ANY]: true,
  name: 'rutis-test',
  inject: ['rutis-host/test'],
  provides: {},
  announces: true,
  api: PLUGIN_API,
  async apply(ctx) {
    const harness = ctx.use('rutis-host/test')
    // Who provides each service now, as the host says (`rutis/service`).
    const providers = new Map()
    ctx.on('rutis/service', event => {
      if (event?.available) providers.set(event.name, event.row)
      else providers.delete(event?.name)
    })
    // The test's services, name → its row, and its listeners here, id →
    // withdrawal.
    const services = new Map()
    const listeners = new Map()
    let control
    // The five ways to emit, from this plugin: `<mode>(name, ...values)`;
    // `filter.<mode>(keep, name, ...values)` with the test's filter; a
    // waterfall's terminal first after it (`waterfall.with`).
    const emitting = {}
    for (const filtered of [false, true]) {
      for (const mode of ['emit', 'parallel', 'serial', 'waterfall', 'bail', 'waterfall.with']) {
        emitting[`${filtered ? 'filter.' : ''}${mode}`] = (...args) => {
          const on = filtered ? ctx.filter(descriptor => args[0](descriptor)) : ctx
          const rest = filtered ? args.slice(1) : args
          if (mode === 'waterfall.with') return on.waterfall.with(rest[0])(...rest.slice(1))
          return on[mode](...rest)
        }
      }
    }
    const methods = {
      ...emitting,
      load: (file, config, options) => harness.load(file, config ?? null, options ?? null),
      update: (id, config) => harness.update(id, config),
      unload: id => harness.unload(id),
      reported: id => harness.reported(id),
      provided: id => [...providers].filter(([, row]) => row === id).map(([name]) => name).sort(),
      methods: name => inferred(ctx.use(name)),
      // A service of the test, as a row of its own (./service.mjs): a
      // name rutis has already fails that row's load only. Provided here,
      // by this plugin, it failed this plugin, and the test with it (#988).
      provide: async (name, names) => {
        const id = await harness.load(SERVICE, { name, methods: names }, { id: `rutis-test/${name}` })
        services.set(name, id)
      },
      withdraw: async name => {
        const id = services.get(name)
        services.delete(name)
        if (id !== undefined) await harness.unload(id)
      },
      on: (id, name, options) => {
        listeners.set(id, ctx.on(name, (...args) => control.call(id, 'call', args), options))
      },
      off: id => {
        const off = listeners.get(id)
        listeners.delete(id)
        off?.()
      },
    }
    // The test's calls: its own (no target), or of a service here, by name
    // (`weather.today`), as a plugin next to it calls it.
    control = await Process.connect(`unix:${process.env.RUTIS_TEST_CONTROL}`, (target, method, args) => {
      if (target !== '') return ctx.use(target)[method](...(args ?? []))
      if (!Object.hasOwn(methods, method)) throw new Error(`the test driver has no method ${method}`)
      return methods[method](...(args ?? []))
    }, undefined, { local: 'rutis-test' }, {
      // The test's synchronous calls run as they arrive, also while this
      // side waits for the test (a listener of the test's, called for an
      // emit of a service call that returned): the test, waiting for them,
      // answers nothing meanwhile (#984).
      inline: SYNCHRONOUS,
      // Each side may wait for the other on two call chains at once: the
      // test calling a callback of a plugin's while that plugin calls one
      // of the test's services. Each runs the other's synchronous calls
      // while it waits (#987).
      answersWaiting: true,
    })
    globalThis[CONTROL] = control
    // The test process went (done, failed or killed): the test is over.
    // Its temporary directory goes, and this runtime ends, after which
    // `rutis-host test` stops and exits (as a driver ends a test).
    control.closed().then(() => {
      if (process.env.RUTIS_TEST_DIR) rmSync(process.env.RUTIS_TEST_DIR, { recursive: true, force: true })
      process.exit(0)
    })
    ctx.effect(() => { control.end() })
  },
})
