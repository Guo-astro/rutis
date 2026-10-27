// Errors nobody handled (#688; docs/design-diagnostics-2026-10-26.en.md
// §4.7). The plugins of a runtime share its process, so what an unhandled
// promise rejection or an uncaught exception does is the runtime's to say,
// not Node's default (ending the process, and every row with it):
//
// - Under `rutis-host dev` (RUTIS_DEV=1) the runtime records which row each
//   piece of asynchronous work belongs to: its entry points (a fiber's
//   apply and effects, a service method, a listener, a function exported by
//   reference) run in the row's async context, and every promise remembers
//   the row it was created in (`async_hooks`). An error left unhandled
//   fails that row; the others go on.
// - Under `run` none of this is on (it costs something on every promise),
//   and an error left unhandled is reported as from an unknown row.
// - Either way the process does not exit. A plugin calling `process.exit`
//   still ends it; the line before says which row called it: the row
//   running (under `dev`), else the one row whose plugin file is on the
//   stack.
import { AsyncLocalStorage, createHook } from 'node:async_hooks'
import { realpathSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { inspect } from 'node:util'
import { reportedStack } from './errors.mjs'

// Rows are traced only when `enabled`: `within(row, run)` runs `run` as
// the row's (undefined: no row's), `current()` is the row running now,
// `owner(promise)` the row a promise was created in.
// An error traced to a row whose stack shows only another row's plugin
// file is charged to no row (the runner's `chargedRow`).
export function rowTracking(enabled) {
  if (!enabled) return { enabled: false, within: (_row, run) => run(), current: () => undefined, owner: () => undefined }
  const store = new AsyncLocalStorage()
  const owners = new WeakMap()
  createHook({
    init(_id, type, _trigger, resource) {
      if (type !== 'PROMISE') return
      const row = store.getStore()
      if (row !== undefined) owners.set(resource, row)
    },
  }).enable()
  return {
    enabled: true,
    within: (row, run) => store.run(row ?? undefined, run),
    current: () => store.getStore(),
    // A promise created outside every row is no row's, whichever row's
    // code rejected it.
    owner: promise => (promise ? owners.get(promise) : undefined),
  }
}

// What an unhandled value says: an error's name and message, anything
// else as Node shows it.
export function errorText(error) {
  if (error instanceof Error || (error && typeof error === 'object' && typeof error.message === 'string' && typeof error.stack === 'string')) {
    return `${error.name ?? 'Error'}: ${error.message}`
  }
  return inspect(error, { depth: 2, breakLength: Infinity })
}

// Where an error was thrown: its stack's first frame, `file:line:column`.
export function errorSite(error) {
  const stack = typeof error?.stack === 'string' ? error.stack : ''
  for (const line of stack.split('\n')) {
    const frame = line.trim()
    if (!frame.startsWith('at ')) continue
    const inParens = /\((.*)\)$/.exec(frame)
    return inParens ? inParens[1] : frame.slice(3)
  }
  return undefined
}

// The error as a row's failure: what kind of failure, the error, where.
export function failureText(kind, error) {
  const site = errorSite(error)
  return `${kind}: ${errorText(error)}${site ? ` (at ${site})` : ''}`
}

// What failed, the error's own text: `unhandled rejection: Error: boom`
// (where, the host prints from the error's stack).
export function kindText(kind, error) {
  return `${kind}: ${errorText(error)}`
}

// The error a load fails with when the row's code left `error` unhandled
// while it loaded: an `UnhandledError` saying what, with the error's own
// frames, code and data, and the error itself as its cause, so that its
// own `cause` and an AggregateError's `errors` cross with it as they do
// for an error once the row has loaded. (The host prints the root's text
// and frames, not its causes, so the error is not printed twice.)
export function unhandledError(kind, error) {
  const failure = new Error(kindText(kind, error))
  failure.name = 'UnhandledError'
  const frames = typeof error?.stack === 'string' ? error.stack.split('\n').filter(line => /^\s*at /.test(line)) : []
  failure.stack = [`UnhandledError: ${failure.message}`, ...frames].join('\n')
  if (error && typeof error === 'object') {
    if (typeof error.code === 'string') failure.code = error.code
    if (Object.hasOwn(error, 'data')) failure.data = error.data
    failure.cause = error
  }
  return failure
}

// The error as reported for no row: its stack with only the plugin's
// frames, or the whole stack when none is the plugin's (it is then all
// there is to find its source by).
export function reportText(error) {
  if (typeof error?.stack === 'string' && error.stack.length) return reportedStack(error.stack)
  return errorText(error)
}

// The source files on a stack, as real paths (a module URL's query, such
// as a reload's, left out).
export function stackFiles(stack) {
  const files = new Set()
  for (const line of String(stack ?? '').split('\n')) {
    const frame = line.trim()
    if (!frame.startsWith('at ')) continue
    let where = /\((.*)\)$/.exec(frame)?.[1] ?? frame.slice(3)
    where = where.replace(/(:\d+){1,2}$/, '').replace(/\?.*$/, '')
    try {
      if (where.startsWith('file:')) where = fileURLToPath(where)
      else if (!where.startsWith('/') && !/^[A-Za-z]:[\\/]/.test(where)) continue
      files.add(realpathSync(where))
    } catch {}
  }
  return files
}

// `file` as a real path, or undefined.
export function realFile(file) {
  try { return file === undefined ? undefined : realpathSync(file) } catch { return undefined }
}

// Install the process's handlers. `charge(kind, error, row)` is told of
// every error nobody handled, with the row it is traced to (undefined:
// none); `exiting(row, files, code)` of a call of `process.exit`, before
// the process exits, with the row running (when rows are traced) and the
// source files on the stack.
export function install(tracking, { charge, exiting }) {
  process.on('unhandledRejection', (reason, promise) => charge('unhandled rejection', reason, tracking.owner(promise)))
  process.on('uncaughtException', error => charge('uncaught exception', error, tracking.current()))
  const exit = process.exit
  process.exit = function (code) {
    try { exiting(tracking.current(), stackFiles(new Error().stack), code ?? process.exitCode ?? 0) } catch {}
    return Reflect.apply(exit, this, arguments)
  }
}
