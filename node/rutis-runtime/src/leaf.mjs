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
// other runtimes, so a leaf plugin can neither listen to nor emit them, and a
// mount cannot declare them for emission (boundary rule 5: internal/* stays
// internal). This is the one place the Node side defines the check: the leaf
// adapter, the runner's mount and the test tool all use the wording here
// (`node/rutis/src/testing.mjs` repeats it, since the SDK is its own package
// and cannot import the runtime). A name is a string: Cordis's `emit` takes
// an object, null or function first argument as `this`, so any other type
// would let `emit(null, 'internal/service')` past the check.
export const INTERNAL_MESSAGE = name => `event ${name} is internal to the runtime`

export function reserved(name) {
  if (typeof name !== 'string') throw new TypeError(`an event name is a string, not ${name === null ? 'null' : typeof name}`)
  if (name.startsWith('internal/')) throw new Error(INTERNAL_MESSAGE(name))
  return name
}

/// Refuse a mount whose `field` (`emits` or `events`) lists internal events,
/// with the sentence of rutis's own check (`Process::mount`, #337), naming
/// every one.
export function refuseInternal(field, names) {
  const internal = (names ?? []).filter(name => String(name).startsWith('internal/'))
  if (internal.length) throw new Error(`framework-internal events do not cross the boundary (mount ${field}): ${internal.join(', ')}`)
}

// The Cordis plugin that runs a leaf plugin. Events share Cordis's process
// bus; `context.on` owns listener cleanup through the plugin's fiber.
// What a leaf plugin providing a service its `provides` does not declare
// is told when the host, older than 0.12, mounted the runtime without
// `announce`: such a host does not take the service, which stays in this
// process.
export const OLD_HOST = name => `rutis: service ${name} is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host`

// A function telling OLD_HOST once per name, with `write`.
export function oncePerName(write) {
  const told = new Set()
  return name => {
    if (told.has(name)) return
    told.add(name)
    write(`${OLD_HOST(name)}\n`)
  }
}

// `undeclared(name)`: the plugin provides `name`, which its `provides` does
// not declare.
export function toCordis(spec, undeclared) {
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
        provide: (name, value) => {
          // A keyed name (`#`) is never announced, by any host.
          if (undeclared && !name.includes('#') && !Object.hasOwn(spec.provides ?? {}, name)) undeclared(name)
          return context.provide(name, value)
        },
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
