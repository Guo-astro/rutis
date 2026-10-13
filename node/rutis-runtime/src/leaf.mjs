// Leaf plugins, as the SDK (@arcships/rutis) makes them: a JavaScript or
// TypeScript plugin that needs no Cordis. The runtime loads it into its
// Cordis Context like any plugin, so it shares services — and the event
// bus — with Cordis plugins in the same process; the plugin itself only
// sees `ctx.use`, `ctx.provide`, `ctx.effect`, `ctx.on` and `ctx.emit`.
// The runtime does not import the SDK: it recognises its plugins by the
// symbol the SDK marks them with.

// The plugin APIs this runtime runs: a plugin written against a newer one
// is refused, naming both.
// 2: ctx.on and ctx.emit (leaf events). A runtime that supports only 1
// refuses such a plugin at load instead of failing in apply.
export const PLUGIN_API = 2

const PLUGIN = Symbol.for('rutis.leaf-plugin')

export const isLeaf = value => value?.[PLUGIN] === true

// Refuse a plugin that needs a newer plugin API than this runtime's.
export function supported(plugin, name) {
  const api = plugin?.api ?? 1
  if (api > PLUGIN_API) {
    throw new Error(`plugin ${name} needs plugin API ${api}; this runtime supports ${PLUGIN_API}: upgrade @arcships/rutis-runtime where the host runs`)
  }
  return plugin
}

// Cordis's internal/* events drive the runner itself and do not exist on the
// other runtimes, so a leaf plugin can neither listen to nor emit them
// (boundary rule 5: internal/* stays internal).
export function reserved(name) {
  if (typeof name === 'string' && name.startsWith('internal/')) throw new Error(`event ${name} is internal to the runtime`)
  return name
}

// The Cordis plugin that runs a leaf plugin. Events share Cordis's process
// bus; `context.on` owns listener cleanup through the plugin's fiber.
export function toCordis(spec) {
  return {
    name: spec.name ?? 'leaf',
    inject: spec.inject,
    async apply(context, config) {
      const ctx = {
        use(name) {
          const value = context.get(name)
          if (value === undefined) throw new Error(`service ${name} is not available`)
          return value
        },
        provide: (name, value) => context.provide(name, value),
        effect: cleanup => { context.effect(() => cleanup) },
        on: (name, handler) => context.on(reserved(name), handler),
        emit: (name, ...args) => context.emit(reserved(name), ...args),
      }
      const cleanup = await spec.apply(ctx, config)
      if (cleanup !== undefined && cleanup !== null) {
        if (typeof cleanup !== 'function') throw new TypeError('apply must return a cleanup function or nothing')
        context.effect(() => cleanup)
      }
    },
  }
}
