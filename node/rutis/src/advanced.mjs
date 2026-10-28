// The names a plugin author rarely writes (#692): the plugin API version,
// the marks the runtimes recognise plugins and values by, and the value
// constructors. `ctx` and `definePlugin` are in `@arcships/rutis`.
//
//   import { datetime, decimal } from '@arcships/rutis/advanced'
//   return { at: datetime('2026-10-15T14:30'), price: decimal('1.10') }

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

// Whether `value` is a plugin definePlugin made.
export const isPlugin = value => value?.[PLUGIN] === true

// Values JavaScript has no type for, to send as what they are (#308): a
// local date and time, a date, a time of day, a decimal, a UUID. Received,
// they are strings; a plain string is sent as a string.
//
// A Date, Uint8Array, Map, Set and bigint need no constructor: they cross
// as an instant, bytes, a map, a set and a big integer.
export { datetime, date, time, decimal, uuid } from './values.mjs'
// The mark on what they make, across copies of this package and in the
// runtimes: `Symbol.for('rutis.value')`.
export { MARK as VALUE } from './values.mjs'
