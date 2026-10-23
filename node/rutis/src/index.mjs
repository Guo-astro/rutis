// Write a rutis plugin in JavaScript or TypeScript. A plugin declares the
// services it uses (`inject`), and `apply(ctx, config)` uses and provides
// them through `ctx`; rutis decides when it starts, stops and restarts. The
// runtime that runs it is installed by the host (`@arcships/rutis-runtime`
// in Node, `@arcships/rutis-bun` in Bun), not by the plugin.
//
//   export default definePlugin({
//     inject: ['llm'],
//     config: { type: 'object', properties: { city: { type: 'string' } } },
//     apply(ctx, config) {
//       ctx.provide('weather', new Weather(ctx.use('llm'), config.city))
//       ctx.on('weather/ask', city => city === config.city ? 'sunny' : undefined)
//       return () => {}
//     },
//   })
//
// Events cross every process and language on the host (#464): `ctx.on`
// listens to every event of the name on the flat bus, and
// `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail` reach every
// listener, wherever it runs, as if all were in this process.
//
// What `ctx.provide` provides reaches rutis by itself: its methods are the
// object's functions (its own and its prototype chain's, not `constructor`),
// called as `async`. `provides` is optional: `{ weather: { unit: 'sync' } }`
// marks methods `sync` and tells `rutis-host check` what the plugin
// provides before it runs.

// The plugin API this SDK writes plugins against. A runtime refuses a plugin
// that needs a newer one, naming both.
// 2: ctx.on and ctx.emit (leaf events). A runtime that supports only 1
// refuses such a plugin at load instead of failing in apply.
// 3: the unified events of rutis 0.13 (#464): ctx.parallel, serial,
// waterfall, bail and filter, ctx.signal, and `prepend` / `global` on
// ctx.on. A runtime of 0.12 or older refuses such a plugin at load.
export const PLUGIN_API = 3

// Marks what definePlugin returns, across copies of this package.
export const PLUGIN = Symbol.for('rutis.leaf-plugin')

export function definePlugin(spec) {
  if (typeof spec?.apply !== 'function') throw new TypeError('a plugin needs apply(ctx, config)')
  const inject = spec.inject ?? []
  if (!Array.isArray(inject) || inject.some(name => typeof name !== 'string')) throw new TypeError('inject must be a list of service names')
  for (const [name, methods] of Object.entries(spec.provides ?? {})) {
    for (const [method, kind] of Object.entries(methods ?? {})) {
      if (kind !== 'sync' && kind !== 'async') throw new TypeError(`${name}.${method}: kind must be 'sync' or 'async'`)
    }
  }
  // `announces`: what it provides beyond `provides` (all of it, without
  // one) reaches rutis by itself, and is known once it runs.
  return Object.freeze({ ...spec, inject, provides: spec.provides ?? {}, announces: true, api: PLUGIN_API, [PLUGIN]: true })
}

// Whether `value` is a plugin definePlugin made.
export const isPlugin = value => value?.[PLUGIN] === true

// Values JavaScript has no type for, to send as what they are (#308): a
// local date and time, a date, a time of day, a decimal, a UUID. Received,
// they are strings; a plain string is sent as a string.
//
//   import * as rutis from '@arcships/rutis'
//   return { at: rutis.datetime('2026-10-15T14:30'), price: rutis.decimal('1.10') }
//
// A Date, Uint8Array, Map, Set and bigint need no constructor: they cross
// as an instant, bytes, a map, a set and a big integer.
export { datetime, date, time, decimal, uuid } from './values.mjs'
// The mark on what they make, across copies of this package and in the
// runtimes: `Symbol.for('rutis.value')`.
export { MARK as VALUE } from './values.mjs'
