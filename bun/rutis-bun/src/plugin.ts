// Leaf plugins: what a plugin module declares, and finding the module.
//
// A plugin module either exports the plugin `definePlugin` (from
// `@arcships/rutis`) made as its default export, or declares one with its own
// exports: `apply(ctx, config)`, and optionally `inject`, `provides` and
// `config` (a JSON Schema). `provides` is optional and gives each method's
// kind: `{ weather: { today: 'sync', later: 'async' } }`; what a plugin
// provides without declaring it reaches rutis all the same (`announces`).
import { realpathSync, readFileSync, statSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, isAbsolute, join, resolve as resolvePath } from 'node:path'
import { fileURLToPath } from 'node:url'
import { cannotLoad, entryGone, notDefinePlugin } from './errors.ts'

// The plugin API this runtime runs: a plugin written against a newer one is
// refused, naming both.
// 2: ctx.on and ctx.emit (leaf events). A runtime that supports only 1
// refuses such a plugin at load instead of failing in apply.
// 3: the unified events of rutis 0.13 (#464): ctx.parallel, serial,
// waterfall, bail and filter, and ctx.signal. Plugins of API 2 still load.
export const PLUGIN_API = 3
// What `definePlugin` marks its plugins with, across copies of the SDK.
const PLUGIN = Symbol.for('rutis.leaf-plugin')

export type Kind = 'sync' | 'async'
export interface Plugin {
  // Its name, for listener descriptors; null when it gives none.
  name: string | null
  apply(ctx: unknown, config: unknown): unknown
  inject: string[]
  provides: Record<string, Record<string, Kind>>
  // What it provides beyond `provides` reaches rutis by itself, known once
  // it runs: every plugin of a 0.12 SDK, and every module.
  announces: boolean
  config: unknown
  api: number
}

function notFound(entry: string): Error {
  return Object.assign(new Error(`no plugin ${entry} here`), { name: 'NotFound' })
}

// The file of the plugin `entry` names: an absolute path or `file:` URL; a
// path relative to the project (`./plugin.ts`); or an npm package name or
// subpath, resolved from the project. Not found: an error named `NotFound`,
// which the host reports as an unresolved row.
//
// Files are named by their real path: Bun imports a file created after it
// started only by its real path, not through a symbolic link to its
// directory (macOS's /var is /private/var), and keeps its module registry
// by real path too.
export function locate(entry: string, project: string): string {
  let path: string
  if (entry.startsWith('file:')) path = fileURLToPath(entry)
  else if (isAbsolute(entry)) path = entry
  else if (entry.startsWith('./') || entry.startsWith('../')) path = resolvePath(project, entry)
  else {
    try { return Bun.resolveSync(entry, project) } catch { throw notFound(entry) }
  }
  try { return realpathSync(path) } catch { throw notFound(entry) }
}

// The version of the package an npm name belongs to; nothing for files.
export function versionOf(entry: string, file: string): string | null {
  if (entry.startsWith('.') || entry.startsWith('file:') || isAbsolute(entry)) return null
  const name = entry.startsWith('@') ? entry.split('/').slice(0, 2).join('/') : entry.split('/')[0]
  for (let dir = dirname(file); ; dir = dirname(dir)) {
    try {
      const manifest = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
      if (manifest.name === name) return typeof manifest.version === 'string' ? manifest.version : null
    } catch {}
    if (dirname(dir) === dir) return null
  }
}

function check(plugin: any, entry: string): Plugin {
  if (typeof plugin?.apply !== 'function') throw new TypeError(`${entry} is not a plugin: it has no apply(ctx, config)`)
  const inject = plugin.inject ?? []
  if (!Array.isArray(inject) || inject.some((name: unknown) => typeof name !== 'string')) throw new TypeError(`${entry}: inject must be a list of service names`)
  const provides = plugin.provides ?? {}
  for (const [name, methods] of Object.entries(provides)) {
    for (const [method, kind] of Object.entries(methods ?? {})) {
      if (kind !== 'sync' && kind !== 'async') throw new TypeError(`${entry}: ${name}.${method}: kind must be 'sync' or 'async'`)
    }
  }
  const api = plugin.api ?? PLUGIN_API
  if (api > PLUGIN_API) {
    throw new Error(`plugin ${entry} needs plugin API ${api}; this runtime supports ${PLUGIN_API}: upgrade @arcships/rutis-bun where the host runs`)
  }
  // An SDK's plugin says (0.12 on); a module's own exports may always
  // provide more than they declare.
  const announces = typeof plugin.announces === 'boolean' ? plugin.announces : true
  return { name: typeof plugin.name === 'string' ? plugin.name : null, apply: plugin.apply, inject: [...inject], provides, announces, config: plugin.config ?? null, api }
}

// A plugin definition this runtime handed over, back to be described or
// loaded as a row's entry: an SDK plugin (`definePlugin`), or an object with
// `apply` written like a module's exports. Bun has no Cordis plugins.
export function definition(value: any): Plugin {
  if (value === null || typeof value !== 'object') throw new TypeError('not a plugin definition: expected a definePlugin result or an object with apply')
  return check(value, typeof value.name === 'string' ? value.name : 'plugin definition')
}

// The plugin a module declares.
// `file`: the module's file, which the developer is told of.
export function declared(module: any, entry: string, file?: string): Plugin {
  if (module?.default?.[PLUGIN] === true) return check(module.default, entry)
  if (typeof module?.apply === 'function') return check(module, entry)
  // Neither: its default export is not a plugin (§8.2).
  if (typeof module?.default?.apply !== 'function') throw notDefinePlugin(file ?? entry)
  return check(module.default, entry)
}

// Plugin modules, imported again when their file changed since (a reload
// describes the row again, asking for the new code). Only the plugin module
// itself is imported again, not the modules it imports.
export class Modules {
  #stamps = new Map<string, string>()
  #imported = new Map<string, any>()
  #require = createRequire(import.meta.url)
  #development: boolean

  // `development`: under `rutis-host dev` (RUTIS_DEV=1), a row loads the
  // module it was last described with (`described`).
  constructor(development = process.env.RUTIS_DEV === '1') {
    this.#development = development
  }

  // The plugin `entry` names, from its file as it is now (a description);
  // the module is recorded for `described` once it declares a plugin.
  async plugin(file: string, entry: string): Promise<Plugin> {
    const module = await this.import(file)
    const plugin = declared(module, entry, file)
    this.#imported.set(file, module)
    return plugin
  }

  // The plugin a row loads. Under `dev`, the module its last description
  // recorded: a row that loads again for another reason (a service it uses
  // came back, its config changed) runs the code it was described with,
  // not an edit made since, which may not even import; the edit runs once
  // a reload describes it (#665). Outside `dev`, and before any
  // description, the file as it is now.
  async described(file: string, entry: string): Promise<Plugin> {
    if (this.#development && this.#imported.has(file)) return declared(this.#imported.get(file), entry, file)
    return this.plugin(file, entry)
  }

  async import(file: string): Promise<any> {
    let stamp: string | undefined
    try { const stat = statSync(file); stamp = `${stat.mtimeMs}-${stat.size}` } catch {}
    const seen = this.#stamps.get(file)
    if (seen !== undefined && stamp !== undefined && seen !== stamp) {
      // Bun keeps the module in its registry under its real path (`file`
      // is one); a query string would only add another copy that is never
      // freed.
      delete this.#require.cache[file]
    }
    if (stamp !== undefined) this.#stamps.set(file, stamp)
    // An entry that was there and is no more is said so, not loaded from
    // what is left of it in the registry.
    if (seen !== undefined && stamp === undefined) throw entryGone(file)
    try {
      return await import(file)
    } catch (error) {
      // Said of the file that threw (a submodule, say), not of the entry (§8.1).
      throw cannotLoad(error, file)
    }
  }
}
