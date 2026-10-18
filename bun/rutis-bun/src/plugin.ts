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

// The plugin API this runtime runs: a plugin written against a newer one is
// refused, naming both.
// 2: ctx.on and ctx.emit (leaf events). A runtime that supports only 1
// refuses such a plugin at load instead of failing in apply.
export const PLUGIN_API = 2
// What `definePlugin` marks its plugins with, across copies of the SDK.
const PLUGIN = Symbol.for('rutis.leaf-plugin')

export type Kind = 'sync' | 'async'
export interface Plugin {
  apply(ctx: unknown, config: unknown): unknown
  inject: string[]
  provides: Record<string, Record<string, Kind>>
  // It declares no `provides`: what it provides is known once it runs.
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
  if (typeof plugin?.apply !== 'function') throw new TypeError(`${entry} is not a rutis plugin: it has no apply(ctx, config)`)
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
  // An SDK's plugin says (0.12 on); a module's own exports, by having none.
  const announces = typeof plugin.announces === 'boolean' ? plugin.announces : plugin.provides === undefined
  return { apply: plugin.apply, inject: [...inject], provides, announces, config: plugin.config ?? null, api }
}

// The plugin a module declares.
export function declared(module: any, entry: string): Plugin {
  if (module?.default?.[PLUGIN] === true) return check(module.default, entry)
  if (typeof module?.apply === 'function') return check(module, entry)
  return check(module?.default, entry)
}

// Plugin modules, imported again when their file changed since (a reload
// asks for the new code). Only the plugin module itself is imported again,
// not the modules it imports.
export class Modules {
  #stamps = new Map<string, string>()
  #require = createRequire(import.meta.url)

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
    try {
      return await import(file)
    } catch (error) {
      throw located(error)
    }
  }
}

// Bun reports a syntax or resolution error as a BuildMessage or
// ResolveMessage whose message has no location; its `position` has it. The
// error that crosses to the host says where (`file:line:column`), as tsx's
// and Python's do.
function located(error: any): unknown {
  const at = error?.position
  if (!at || typeof at.file !== 'string' || typeof at.line !== 'number') return error
  const where = `${at.file}:${at.line}:${at.column ?? 0}`
  const text = typeof at.lineText === 'string' && at.lineText.trim() ? `\n  ${at.lineText.trim()}` : ''
  return Object.assign(new Error(`${error.message} (${where})${text}`), { name: error.name, cause: error })
}
