// Write a rutis plugin in JavaScript or TypeScript. A plugin declares the
// services it uses (`inject`) and provides (`provides`), and `apply(ctx,
// config)` uses and provides them through `ctx`; rutis decides when it
// starts, stops and restarts. The runtime that runs it is installed by the
// host (`@arcships/rutis-runtime` in Node, `@arcships/rutis-bun` in Bun), not
// by the plugin.
//
//   export default definePlugin({
//     inject: ['llm'],
//     provides: { weather: { today: 'async' } },
//     config: { type: 'object', properties: { city: { type: 'string' } } },
//     apply(ctx, config) {
//       ctx.provide('weather', new Weather(ctx.use('llm'), config.city))
//       return () => {}
//     },
//   })

// The plugin API this SDK writes plugins against. A runtime refuses a plugin
// that needs a newer one, naming both.
// 2: ctx.on and ctx.emit (leaf events). A runtime that supports only 1
// refuses such a plugin at load instead of failing in apply.
export const PLUGIN_API = 2

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
  return Object.freeze({ ...spec, inject, provides: spec.provides ?? {}, api: PLUGIN_API, [PLUGIN]: true })
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
