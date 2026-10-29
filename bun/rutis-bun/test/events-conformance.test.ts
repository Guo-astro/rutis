// The Bun runtime against the event expectations (#464), in one process:
// every case of crates/rutis-bridge/src/runtime/testing/events.expected.json
// the testing tools run (`tool` not false, and `only` / `except` as for
// `bun`), every participant a row of one Runtime (location X, the fast
// path), with the Bun conformance plugin (fixtures/conformance-events.ts).
// Format: docs/design-events-protocol-2026-10-18.en.md §13; what each
// expectation means is crates/rutis-bridge/src/runtime/testing/events.rs.
// Across processes the Rust session matrix checks it.
//
// guarantee: events/bun-runtime (conformance, in-process cases)
import { expect, test } from 'bun:test'
import { mkdtempSync, readFileSync, rmSync, writeFileSync, copyFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Runtime } from '../src/runner.ts'

const expected = JSON.parse(readFileSync(new URL('../../../crates/rutis-bridge/src/runtime/testing/events.expected.json', import.meta.url), 'utf8'))
const LANGUAGE = 'bun'

// How long `logLacks` / `reported` wait after the rest held (events.rs LACKS).
const LACKS = 300
// How long a log expectation may take to hold (events.rs SETTLE).
const SETTLE = 5000

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

// The emits of a step: itself, or its concurrent ones.
const emitsOf = (step: any): any[] => step.emit ? [step.emit] : (step.concurrent ?? []).flatMap(emitsOf)

// The labels of a case (events.rs `places`): its listeners, its loaded
// participants and its emitter rows.
function labelsOf(entry: any): Set<string> {
  const labels = new Set<string>(entry.listeners.map((listener: any) => listener.label))
  for (const step of entry.steps) {
    if (step.load) labels.add(step.load.label)
    for (const spec of emitsOf(step)) labels.add(spec.row ?? 'e')
  }
  return labels
}

// Whether a limit holds with every label at X in this language
// (events.rs `limit_holds`): every label it names (X, Y or the case's) is
// of one of its languages.
function holds(limit: any, labels: Set<string>): boolean {
  return Object.entries(limit).filter(([key]) => key !== 'why')
    .every(([label, languages]) => (label === 'X' || label === 'Y' || labels.has(label)) && (languages as string[]).includes(LANGUAGE))
}

// Why the tool does not run a case, if it does not (events.rs `tool_combo`).
function skipped(entry: any): string | undefined {
  if (entry.tool === false) return 'tool: false'
  const labels = labelsOf(entry)
  const only = (entry.only ?? []).find((limit: any) => !holds(limit, labels))
  if (only) return `only: ${only.why}`
  const except = (entry.except ?? []).find((limit: any) => holds(limit, labels))
  if (except) return `except: ${except.why}`
  return undefined
}

const subsequence = (log: string[], wanted: string[]) => {
  let at = 0
  for (const item of log) if (item === wanted[at]) at++
  return at === wanted.length
}

async function settled(check: () => void) {
  const deadline = Date.now() + SETTLE
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

// `returns` and `throws` against the dispatches that end in a step
// (events.rs `check_outcomes`): without `throws` every one must succeed.
function checkOutcomes(expectation: any, outcomes: any[]) {
  if (expectation.cancelled) throw new Error('cancelled: an emitter in JavaScript cannot give up a dispatch')
  if (('returns' in expectation || expectation.throws) && outcomes.length === 0) throw new Error('expects a result, but no dispatch ends in this step')
  for (const done of outcomes) {
    if (!expectation.throws && done.thrown !== undefined) throw new Error(`expected the dispatch to succeed, it threw ${done.thrown?.stack ?? done.thrown}`)
    if ('returns' in expectation) expect(texts(done.returns === undefined ? null : done.returns)).toEqual(received(expectation.returns))
  }
  if (expectation.throws) {
    const done = outcomes.find(done => done.thrown !== undefined)
    expect(done).toBeDefined()
    const { name, message, errors } = expectation.throws
    if (name) expect(done.thrown.name).toBe(name)
    if (message) expect(done.thrown.message).toContain(message)
    if (errors) {
      expect(done.thrown.errors?.length).toBe(errors.length)
      errors.forEach((sub: any, i: number) => {
        if (sub.name) expect(done.thrown.errors[i].name).toBe(sub.name)
        if (sub.message) expect(done.thrown.errors[i].message).toContain(sub.message)
      })
    }
  }
}

// The error name of a §5.6 report `line` about `event` naming `row`, or
// undefined when the line is about something else (events.rs `reports`):
// `rutis: listener of <event> in row <row> failed: <Name>: <message>`, or
// `rutis: filter of <event> from row <row> failed: ...`.
function reports(line: string, event: string, filter: boolean, row: string): string | undefined {
  const prefix = `rutis: ${filter ? 'filter' : 'listener'} of ${event} ${filter ? 'from' : 'in'} row ${row} failed: `
  if (!line.startsWith(prefix)) return undefined
  const rest = line.slice(prefix.length)
  const colon = rest.indexOf(': ')
  return colon < 0 ? undefined : rest.slice(0, colon)
}

type Events = { log: string[], seen: any[], lines: string[] }

// The log expectations of a step (events.rs `log_holds`).
function checkLog(expectation: any, { log, seen, lines }: Events, event: string) {
  if (expectation.log) expect(log).toEqual(expectation.log)
  if (expectation.logUnordered) expect([...log].sort()).toEqual([...expectation.logUnordered].sort())
  if (expectation.logContains) expect(subsequence(log, expectation.logContains)).toBe(true)
  if (expectation.logLacks) for (const item of expectation.logLacks) expect(log).not.toContain(item)
  if (expectation.filterSees) expect(seen.map(description => description.row ?? 'null')).toEqual(expectation.filterSees)
  if (expectation.reported) {
    for (const { label, name, filter = false } of expectation.reported) {
      const about = lines.filter(line => reports(line, event, filter, label) !== undefined)
      if (about.length !== 1) throw new Error(`reported: expected one line for ${label} about ${event}, got ${about.length}: ${JSON.stringify(lines)}`)
      expect(reports(about[0], event, filter, label)).toBe(name)
    }
  }
}

// The log expectations until they hold; with `logLacks` or `reported`,
// once more a moment later (events.rs `check_log`).
async function checkLogSettled(expectation: any, events: Events, event: string) {
  await settled(() => checkLog(expectation, events, event))
  if (expectation.logLacks || expectation.reported) {
    await Bun.sleep(LACKS)
    checkLog(expectation, events, event)
  }
}

let ran = 0
const knownFailing = 0
for (const entry of expected.cases) {
  const why = skipped(entry)
  test.skipIf(why !== undefined)(`${entry.case} (${entry.rule})`, async () => {
    ran++
    const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-events-'))
    writeFileSync(join(dir, 'package.json'), '{}')
    copyFileSync(new URL('./fixtures/conformance-events.ts', import.meta.url).pathname, join(dir, 'conformance-events.ts'))
    writeFileSync(join(dir, 'log.ts'), 'export function apply(ctx) { ctx.provide("events-log", { add(item) { globalThis.__eventsLog.add(item) }, filter(description) { globalThis.__eventsLog.filter(description) } }) }')
    const events: Events = { log: [], seen: [], lines: [] }
    ;(globalThis as any).__eventsLog = {
      add(item: string) { events.log.push(item) },
      filter(description: any) { events.seen.push(description); events.log.push(`filter:${description.row ?? 'null'}`) },
    }
    // The runtime reports (§5.6) on standard error, a line each.
    const write = process.stderr.write
    process.stderr.write = ((text: string) => {
      for (const line of String(text).split('\n')) if (line.startsWith('rutis: ')) events.lines.push(line)
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
      const modes = entry.steps.flatMap(emitsOf).map((spec: any) => spec.mode)
      const sync = modes.length > 0 && modes.every((mode: string) => mode === 'bail')
      const configs = new Map<string, any>()
      const loadListener = (listener: any) => {
        const { at: _at, scope, ...rest } = listener
        const config = { ...rest, name: listener.name ?? entry.name, kind: listener.kind ?? (sync ? 'sync' : 'async'), log: listener.log ?? 'label' }
        configs.set(listener.label, config)
        return runtime.load(listener.label, './conformance-events.ts', config, null, null, chainOf(scope))
      }
      let generation = 0
      for (const listener of entry.listeners) await loadListener(listener)
      const emitters = new Set<string>()
      // The spec as the emitter's dispatch receives it (events.rs `Spec::wire`).
      const emit = async (spec: any) => {
        const { from: _from, row = 'e', scope, ...rest } = spec
        const key = !scope || scope === 'root' ? row : `${row}@${scope}`
        if (!emitters.has(key)) {
          await runtime.load(key, './conformance-events.ts', { emitter: true, label: key }, null, null, chainOf(scope))
          emitters.add(key)
        }
        const service: any = runtime.lookup(`emitter-${key}`)
        const wire = { ...rest, name: spec.name ?? entry.name }
        return wire.mode === 'bail' ? service.dispatch_sync(wire) : service.dispatch(wire)
      }
      let background: Promise<any> | undefined
      for (const step of entry.steps) {
        const expectation = step.expect ?? {}
        const event = emitsOf(step)[0]?.name ?? entry.name
        const outcomes: any[] = []
        if (step.emit) {
          if (step.background) {
            background = outcome(emit(step.emit))
            await checkLogSettled(expectation, events, event)
            continue
          }
          outcomes.push(await outcome(emit(step.emit)))
        } else if (step.load) {
          await loadListener(step.load)
        } else if (step.update !== undefined) {
          // A new config: the row registers its listeners again (rows.update).
          const config = { ...configs.get(step.update), generation: ++generation }
          configs.set(step.update, config)
          await runtime.update(step.update, config)
        } else if (step.unload !== undefined) {
          if (step.when) await settled(() => expect(events.log).toContain(step.when))
          const started = Date.now()
          await runtime.unload(step.unload)
          if (background) outcomes.push(await background)
          background = undefined
          if (expectation.settles) expect(Date.now() - started).toBeLessThan(SETTLE)
        } else {
          throw new Error(`cannot run ${JSON.stringify(Object.keys(step))}`)
        }
        checkOutcomes(expectation, outcomes)
        await checkLogSettled(expectation, events, event)
      }
    } finally {
      await runtime.dispose()
      process.stderr.write = write
      delete (globalThis as any).__eventsLog
      rmSync(dir, { recursive: true, force: true })
    }
  })
}

// The skip logic agrees with events.rs `tool_combo`: it ran as many cases
// as the file says it runs.
test('ran every case the file gives bun', () => {
  expect(ran + knownFailing).toBe(expected.toolCases[LANGUAGE])
})
