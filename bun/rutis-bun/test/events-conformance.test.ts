// The Bun runtime against the event expectations (#464), in one process:
// every case of crates/rutis-bridge/src/runtime/testing/events.expected.json
// the testing tools run (`tool` not false), every participant a row of one
// Runtime (location X, the fast path), with the Bun conformance plugin
// (fixtures/conformance-events.ts). Format: docs/design-events-protocol-
// 2026-10-18.en.md §13. Across processes the Rust session matrix checks it.
//
// guarantee: events/bun-runtime (conformance, in-process cases)
import { expect, test } from 'bun:test'
import { mkdtempSync, readFileSync, rmSync, writeFileSync, copyFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Runtime } from '../src/runner.ts'

const expected = JSON.parse(readFileSync(new URL('../../../crates/rutis-bridge/src/runtime/testing/events.expected.json', import.meta.url), 'utf8'))
const LANGUAGE = 'bun'

const readers: Record<string, (v: any) => unknown> = {
  $instant: (text: string) => new Date(text),
  $bytes: (text: string) => new Uint8Array(Buffer.from(text, 'base64')),
  $map: (entries: [unknown, unknown][]) => new Map(entries.map(([k, v]) => [received(k), received(v)])),
  $set: (elements: unknown[]) => new Set(elements.map(received)),
  $bigint: BigInt,
  $datetime: String, $date: String, $time: String, $decimal: String, $uuid: String,
}
function received(value: any): any {
  if (Array.isArray(value)) return value.map(received)
  if (value === null || typeof value !== 'object') return value
  const keys = Object.keys(value)
  if (keys.length === 1 && Object.hasOwn(readers, keys[0])) return readers[keys[0]](value[keys[0]])
  return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, received(v)]))
}

function skipped(entry: any): string | undefined {
  if (entry.tool === false) return 'tool: false'
  for (const [field, run] of [['only', (l: string[]) => l.includes(LANGUAGE)], ['except', (l: string[]) => !l.includes(LANGUAGE)]] as const) {
    for (const rule of entry[field] ?? []) {
      for (const [label, langs] of Object.entries(rule)) if (label !== 'why' && !run(langs as string[])) return `${field} ${label}`
    }
  }
  if (entry.steps.some((step: any) => step.emit?.via === 'thisArg')) return 'Context.filter is a Cordis plugin\'s'
  return undefined
}

const subsequence = (log: string[], wanted: string[]) => {
  let at = 0
  for (const item of log) if (item === wanted[at]) at++
  return at === wanted.length
}

async function settled(check: () => void) {
  const deadline = Date.now() + 5000
  for (;;) {
    try { return check() } catch (error) {
      if (Date.now() >= deadline) throw error
      await Bun.sleep(10)
    }
  }
}

async function outcome(promise: Promise<unknown>) {
  try { return { returns: await promise } as any } catch (thrown) { return { thrown } as any }
}

// In one process nothing crosses: an SDK constructor's value is compared
// as the text another process receives.
function texts(value: any): any {
  if (Array.isArray(value)) return value.map(texts)
  if (value?.[Symbol.for('rutis.value')]) return value.value
  return value
}

function checkOutcome(expectation: any, done: any) {
  if ('returns' in expectation) {
    expect(done.thrown).toBeUndefined()
    expect(texts(done.returns === undefined ? null : done.returns)).toEqual(received(expectation.returns))
  }
  if (expectation.throws) {
    expect(done.thrown).toBeDefined()
    const { name, message, errors } = expectation.throws
    if (name) expect(done.thrown.name).toBe(name)
    if (message) expect(done.thrown.message).toContain(message)
    if (errors) expect(done.thrown.errors.map((e: any) => e.name)).toEqual(errors.map((e: any) => e.name))
  }
}

function checkLog(expectation: any, log: string[], reported: any[]) {
  if (expectation.log) expect(log).toEqual(expectation.log)
  if (expectation.logUnordered) expect([...log].sort()).toEqual([...expectation.logUnordered].sort())
  if (expectation.logContains) expect(subsequence(log, expectation.logContains)).toBe(true)
  if (expectation.logLacks) for (const item of expectation.logLacks) expect(log).not.toContain(item)
  if (expectation.filterSees) expect(log.filter(e => e.startsWith('filter:')).map(e => e.slice(7))).toEqual(expectation.filterSees)
  if (expectation.reported) expect(reported).toEqual(expectation.reported)
}

for (const entry of expected.cases) {
  const why = skipped(entry)
  test.skipIf(why !== undefined)(`${entry.case} (${entry.rule})`, async () => {
    const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-events-'))
    writeFileSync(join(dir, 'package.json'), '{}')
    copyFileSync(new URL('./fixtures/conformance-events.ts', import.meta.url).pathname, join(dir, 'conformance-events.ts'))
    writeFileSync(join(dir, 'log.ts'), 'export function apply(ctx) { ctx.provide("events-log", { add(item) { globalThis.__eventsLog.push(item) }, filter(description) { globalThis.__eventsLog.push(`filter:${description.row}`) } }) }')
    const log: string[] = (globalThis as any).__eventsLog = []
    const reported: any[] = []
    const write = process.stderr.write
    process.stderr.write = ((text: string) => {
      const match = /^rutis: listener of \S+ in row (\S+) failed: (\w+):/.exec(String(text))
      if (match) reported.push({ label: match[1], name: match[2] })
      return true
    }) as typeof write
    const runtime = new Runtime(dir)
    const chainOf = (path?: string) => {
      if (!path || path === 'root') return []
      const parts = path.split('/')
      return parts.map((part, i) => ({ id: parts.slice(0, i + 1).join('/'), group: expected.scopes[parts.slice(0, i + 1).join('/')] ?? part }))
    }
    try {
      runtime.dispatch('', 'mount', { announce: true })
      await runtime.load('log', './log.ts', {}, null, null)
      const modes = entry.steps.flatMap((step: any) => step.emit ? [step.emit.mode] : [])
      const sync = modes.length > 0 && modes.every((mode: string) => mode === 'bail' || mode === 'waterfall-sync')
      const configs = new Map<string, any>()
      const loadListener = (listener: any) => {
        const config = { ...listener, name: listener.name ?? entry.name, kind: listener.kind ?? (sync ? 'sync' : 'async') }
        configs.set(listener.label, config)
        return runtime.load(listener.label, './conformance-events.ts', config, null, null, chainOf(listener.scope))
      }
      let generation = 0
      for (const listener of entry.listeners) await loadListener(listener)
      const emitters = new Set<string>()
      const emit = async (spec: any) => {
        const key = !spec.scope || spec.scope === 'root' ? (spec.row ?? 'e') : `${spec.row ?? 'e'}@${spec.scope}`
        if (!emitters.has(key)) {
          await runtime.load(key, './conformance-events.ts', { emitter: true, label: key }, null, null, chainOf(spec.scope))
          emitters.add(key)
        }
        const service: any = runtime.lookup(`emitter-${key}`)
        const full = { ...spec, name: spec.name ?? entry.name }
        return full.mode === 'bail' ? service.dispatch_sync(full) : service.dispatch(full)
      }
      let background: Promise<any> | undefined
      for (const step of entry.steps) {
        if (step.emit) {
          if (step.background) { background = outcome(emit(step.emit)); continue }
          const done = await outcome(emit(step.emit))
          if (step.expect) {
            checkOutcome(step.expect, done)
            await settled(() => checkLog(step.expect, log, reported))
          }
        } else if (step.load) {
          await loadListener(step.load)
          if (step.expect) await settled(() => checkLog(step.expect, log, reported))
        } else if (step.update !== undefined) {
          // A new config: the row registers its listeners again (rows.update).
          const config = { ...configs.get(step.update), generation: ++generation }
          configs.set(step.update, config)
          await runtime.update(step.update, config)
          if (step.expect) await settled(() => checkLog(step.expect, log, reported))
        } else if (step.unload !== undefined) {
          if (step.when) await settled(() => expect(log).toContain(step.when))
          await runtime.unload(step.unload)
          const done = background ? await background : undefined
          background = undefined
          if (step.expect) {
            if (done) checkOutcome(step.expect, done)
            await settled(() => checkLog(step.expect, log, reported))
          }
        } else {
          throw new Error(`cannot run ${JSON.stringify(Object.keys(step))}`)
        }
      }
    } finally {
      await runtime.dispose()
      process.stderr.write = write
      delete (globalThis as any).__eventsLog
      rmSync(dir, { recursive: true, force: true })
    }
  })
}
