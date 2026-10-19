// The testing tool against the event expectations (#464): every case of
// crates/rutis-bridge/src/runtime/testing/events.expected.json that the
// tool runs (`tool` not false, and `only` / `except` as for `node-leaf`),
// with every participant in this one process (location X), using the leaf
// conformance plugin the real Node runtime runs
// (node/rutis-runtime/test/fixtures/conformance-events-leaf{,-e,-f}.mjs).
// Format: docs/design-events-protocol-2026-10-18.en.md §13; what each
// expectation means is crates/rutis-bridge/src/runtime/testing/events.rs.
//
// guarantee: sdk/testing-matches-runtime (events), the testing tool's side.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { host } from '../src/testing.mjs'
import listenerPlugin from '../../rutis-runtime/test/fixtures/conformance-events-leaf.mjs'
import emitterE from '../../rutis-runtime/test/fixtures/conformance-events-leaf-e.mjs'
import emitterF from '../../rutis-runtime/test/fixtures/conformance-events-leaf-f.mjs'

const expected = JSON.parse(readFileSync(new URL('../../../crates/rutis-bridge/src/runtime/testing/events.expected.json', import.meta.url), 'utf8'))
const LANGUAGE = 'node-leaf'
const emitterPlugins = { e: emitterE, f: emitterF }

// How long `logLacks` / `reported` wait after the rest held (events.rs LACKS).
const LACKS = 300
// How long a log expectation may take to hold (events.rs SETTLE).
const SETTLE = 5000

// A value as the expectations write it, as JavaScript receives it.
const readers = {
  $instant: text => new Date(text),
  $bytes: text => new Uint8Array(Buffer.from(text, 'base64')),
  $map: entries => new Map(entries.map(([key, value]) => [received(key), received(value)])),
  $set: elements => new Set(elements.map(received)),
  $bigint: BigInt,
  $datetime: String, $date: String, $time: String, $decimal: String, $uuid: String,
}
function received(value) {
  if (Array.isArray(value)) return value.map(received)
  if (value === null || typeof value !== 'object') return value
  const keys = Object.keys(value)
  if (keys.length === 1 && Object.hasOwn(readers, keys[0])) return readers[keys[0]](value[keys[0]])
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, received(item)]))
}

// An argument as the file writes it (events.rs `argument`): a value, or
// `{"$callback": "upper"}`, a function returning its argument upper-cased.
function argument(value) {
  if (value !== null && typeof value === 'object' && !Array.isArray(value) && Object.keys(value).length === 1 && '$callback' in value) {
    if (value.$callback !== 'upper') throw new Error(`unknown callback ${value.$callback}`)
    return text => String(text).toUpperCase()
  }
  return received(value)
}

// The emits of a step: itself, or its concurrent ones.
const emitsOf = step => step.emit ? [step.emit] : (step.concurrent ?? []).flatMap(emitsOf)

// The labels of a case (events.rs `places`): its listeners, its loaded
// participants and its emitter rows.
function labelsOf(entry) {
  const labels = new Set(entry.listeners.map(listener => listener.label))
  for (const step of entry.steps) {
    if (step.load) labels.add(step.load.label)
    for (const spec of emitsOf(step)) labels.add(spec.row ?? 'e')
  }
  return labels
}

// Whether a limit holds with every label at X in this language
// (events.rs `limit_holds`): every label it names (X, Y or the case's) is
// of one of its languages.
function holds(limit, labels) {
  return Object.entries(limit).filter(([key]) => key !== 'why')
    .every(([label, languages]) => (label === 'X' || label === 'Y' || labels.has(label)) && languages.includes(LANGUAGE))
}

// Why the tool does not run a case, if it does not (events.rs `tool_combo`).
function skipped(entry) {
  if (entry.tool === false) return 'tool: false'
  const labels = labelsOf(entry)
  const only = (entry.only ?? []).find(limit => !holds(limit, labels))
  if (only) return `only: ${only.why}`
  const except = (entry.except ?? []).find(limit => holds(limit, labels))
  if (except) return `except: ${except.why}`
  return undefined
}

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

// Check `check()` until it passes, at most SETTLE (the log may still grow).
async function settled(check) {
  const deadline = Date.now() + SETTLE
  for (;;) {
    try { return check() } catch (error) {
      if (Date.now() >= deadline) throw error
      await sleep(10)
    }
  }
}

async function outcome(promise) {
  try { return { returns: await promise } } catch (thrown) { return { thrown } }
}

const subsequence = (log, wanted) => {
  let at = 0
  for (const entry of log) if (entry === wanted[at]) at++
  return at === wanted.length
}

// `returns` and `throws` against the dispatches that end in a step
// (events.rs `check_outcomes`): without `throws` every one must succeed.
function checkOutcomes(what, expect, outcomes) {
  if (expect.cancelled) throw new Error(`${what}: cancelled: an emitter in JavaScript cannot give up a dispatch`)
  if (('returns' in expect || expect.throws) && outcomes.length === 0) throw new Error(`${what}: expects a result, but no dispatch ends in this step`)
  for (const done of outcomes) {
    if (!expect.throws) assert.equal(done.thrown, undefined, `${what}: expected the dispatch to succeed, it threw ${done.thrown?.stack ?? done.thrown}`)
    if ('returns' in expect) {
      const value = done.returns === undefined ? null : done.returns
      assert.deepStrictEqual(value, received(expect.returns), `${what}: returns`)
    }
  }
  if (expect.throws) {
    const done = outcomes.find(done => done.thrown !== undefined)
    assert.ok(done, `${what}: should throw, returned ${outcomes.map(done => JSON.stringify(done.returns))}`)
    const { name, message, errors } = expect.throws
    if (name) assert.equal(done.thrown.name, name, `${what}: error name (${done.thrown.message})`)
    if (message) assert.ok(done.thrown.message.includes(message), `${what}: ${done.thrown.message} includes ${message}`)
    if (errors) {
      assert.equal(done.thrown.errors?.length, errors.length, `${what}: sub-errors`)
      errors.forEach((sub, i) => {
        if (sub.name) assert.equal(done.thrown.errors[i].name, sub.name, `${what}: sub-error ${i}`)
        if (sub.message) assert.ok(done.thrown.errors[i].message.includes(sub.message), `${what}: sub-error ${i} message`)
      })
    }
  }
}

// The error name of a §5.6 report `line` about `event` naming `row`, or
// undefined when the line is about something else (events.rs `reports`):
// `rutis: listener of <event> in row <row> failed: <Name>: <message>`, or
// `rutis: filter of <event> from row <row> failed: ...`.
function reports(line, event, filter, row) {
  const prefix = `rutis: ${filter ? 'filter' : 'listener'} of ${event} ${filter ? 'from' : 'in'} row ${row} failed: `
  if (!line.startsWith(prefix)) return undefined
  const rest = line.slice(prefix.length)
  const colon = rest.indexOf(': ')
  return colon < 0 ? undefined : rest.slice(0, colon)
}

// The log expectations of a step (events.rs `log_holds`).
function checkLog(what, expect, events, h, event) {
  const { log, seen } = events
  if (expect.log) assert.deepStrictEqual(log, expect.log, `${what}: log`)
  if (expect.logUnordered) assert.deepStrictEqual([...log].sort(), [...expect.logUnordered].sort(), `${what}: logUnordered`)
  if (expect.logContains) assert.ok(subsequence(log, expect.logContains), `${what}: log ${JSON.stringify(log)} contains ${JSON.stringify(expect.logContains)}`)
  if (expect.logLacks) for (const entry of expect.logLacks) assert.ok(!log.includes(entry), `${what}: log ${JSON.stringify(log)} lacks ${entry}`)
  if (expect.filterSees) assert.deepStrictEqual(seen.map(description => description.row ?? 'null'), expect.filterSees, `${what}: filterSees`)
  if (expect.reported) {
    const lines = h.reported.map(report => report.line)
    for (const { label, name, filter = false } of expect.reported) {
      const about = lines.filter(line => reports(line, event, filter, label) !== undefined)
      assert.equal(about.length, 1, `${what}: reported: expected one line for ${label} about ${event}: ${JSON.stringify(lines)}`)
      assert.equal(reports(about[0], event, filter, label), name, `${what}: reported: ${label} should fail with ${name}: ${about[0]}`)
    }
  }
}

// The log expectations until they hold; with `logLacks` or `reported`,
// once more a moment later (events.rs `check_log`).
async function checkLogSettled(what, expect, events, h, event) {
  await settled(() => checkLog(what, expect, events, h, event))
  if (expect.logLacks || expect.reported) {
    await sleep(LACKS)
    checkLog(`${what} (after the step had settled)`, expect, events, h, event)
  }
}

let ran = 0
const knownFailing = 0
for (const entry of expected.cases) {
  const why = skipped(entry)
  test(`${entry.case} (${entry.rule})`, { skip: why }, async () => {
    ran++
    const events = { log: [], seen: [] }
    const waiters = []
    const add = item => {
      events.log.push(item)
      for (const waiter of waiters.splice(0)) waiter()
    }
    const service = {
      add,
      filter(description) { events.seen.push(description); add(`filter:${description.row ?? 'null'}`) },
      async wait(item) { while (!events.log.includes(item)) await new Promise(resolve => waiters.push(resolve)) },
    }
    const h = host({ services: { 'events-log': service }, scopes: expected.scopes })
    const sync = (() => {
      const modes = entry.steps.flatMap(emitsOf).map(spec => spec.mode)
      return modes.length > 0 && modes.every(mode => mode === 'bail' || mode === 'waterfall-sync')
    })()
    const rows = new Map()
    const participants = new Map()
    let generation = 0
    const loadListener = async listener => {
      const { at: _at, scope, ...rest } = listener
      const participant = { ...rest, name: listener.name ?? entry.name, kind: listener.kind ?? (sync ? 'sync' : 'async'), log: listener.log ?? 'label' }
      rows.set(listener.label, await h.load(listenerPlugin, { row: listener.label, scope, config: participant }))
      participants.set(listener.label, participant)
    }
    for (const listener of entry.listeners) await loadListener(listener)
    const emitters = new Map()
    const emitter = async (label, scope) => {
      const key = !scope || scope === 'root' ? label : `${label}@${scope}`
      if (!emitters.has(key)) {
        const loaded = await h.load(emitterPlugins[label], { row: key, scope, config: { label, emitter: true } })
        emitters.set(key, loaded.service(`emitter-${label}`))
      }
      return emitters.get(key)
    }
    // The spec as the emitter's dispatch receives it (events.rs `Spec::wire`).
    const emit = async spec => {
      const { from: _from, row = 'e', scope, ...rest } = spec
      const wire = { ...rest, name: spec.name ?? entry.name, args: spec.args.map(argument) }
      const service = await emitter(row, scope)
      return wire.mode === 'bail' || wire.mode === 'waterfall-sync' ? service.dispatch_sync(wire) : service.dispatch(wire)
    }
    let background
    try {
      for (const [index, step] of entry.steps.entries()) {
        const what = `${entry.case} step ${index}`
        const expect = step.expect ?? {}
        const event = emitsOf(step)[0]?.name ?? entry.name
        const outcomes = []
        if (step.emit) {
          if (step.background) {
            background = outcome(emit(step.emit))
            await checkLogSettled(what, expect, events, h, event)
            continue
          }
          outcomes.push(await outcome(emit(step.emit)))
        } else if (step.update !== undefined) {
          // The row's config changes (a new generation): it registers again.
          await rows.get(step.update).update({ ...participants.get(step.update), generation: ++generation })
        } else if (step.load) {
          await loadListener(step.load)
        } else if (step.unload !== undefined) {
          if (step.when) await settled(() => assert.ok(events.log.includes(step.when), `${what}: ${step.when} started`))
          const started = Date.now()
          await rows.get(step.unload).unload()
          if (background) outcomes.push(await background)
          background = undefined
          if (expect.settles) assert.ok(Date.now() - started < SETTLE, `${what}: settles`)
        } else {
          throw new Error(`${what}: the tool cannot run ${JSON.stringify(Object.keys(step))}`)
        }
        checkOutcomes(what, expect, outcomes)
        await checkLogSettled(what, expect, events, h, event)
      }
    } finally {
      await h.unload()
    }
  })
}

// The tool's skip logic agrees with events.rs `tool_combo`: it ran as many
// cases as the file says it runs.
test('ran every case the file gives node-leaf', () => {
  assert.equal(ran + knownFailing, expected.toolCases[LANGUAGE], `ran ${ran}, known failing ${knownFailing}`)
})
