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

import { PLUGIN, PLUGIN_API } from './advanced.mjs'

// `config` is a JSON Schema object: the host fills its defaults and checks
// the config with it before `apply`. A Standard Schema object (schemastery,
// zod, …) was never applied: refused (config schema design §2.3).
function checkSchema(config) {
  const standard = config?.['~standard']
  if (standard !== undefined) {
    throw new TypeError(`config of definePlugin should be a JSON Schema object, got ${standard?.vendor === 'schemastery' ? 'a schemastery Schema' : 'a Standard Schema'}`)
  }
}

// The module that defined a plugin, as a file URL: the test tool
// (`@arcships/rutis/testing`) has a real runtime load the plugin from it.
const FILE = Symbol.for('rutis.plugin-file')
function definedIn() {
  const prepare = Error.prepareStackTrace
  try {
    Error.prepareStackTrace = (_, sites) => sites
    const holder = {}
    Error.captureStackTrace(holder, definePlugin)
    const file = holder.stack?.[0]?.getFileName?.()
    return typeof file === 'string' ? file : undefined
  } catch {
    return undefined
  } finally {
    Error.prepareStackTrace = prepare
  }
}

export function definePlugin(spec) {
  if (typeof spec?.apply !== 'function') throw new TypeError('a plugin needs apply(ctx, config)')
  checkSchema(spec.config)
  const inject = spec.inject ?? []
  if (!Array.isArray(inject) || inject.some(name => typeof name !== 'string')) throw new TypeError('inject must be a list of service names')
  for (const [name, methods] of Object.entries(spec.provides ?? {})) {
    for (const [method, kind] of Object.entries(methods ?? {})) {
      if (kind !== 'sync' && kind !== 'async') throw new TypeError(`${name}.${method}: kind must be 'sync' or 'async'`)
    }
  }
  // `announces`: what it provides beyond `provides` (all of it, without
  // one) reaches rutis by itself, and is known once it runs.
  return Object.freeze({ ...spec, inject, provides: spec.provides ?? {}, announces: true, api: PLUGIN_API, [PLUGIN]: true, [FILE]: definedIn() })
}
