// The testing tool against the event expectations (#464): every case of
// crates/rutis-bridge/src/runtime/testing/events.expected.json that the
// tool runs (`tool` not false), with every participant in this one process
// (location X), using the conformance plugin the real Node runtime runs
// (node/rutis-runtime/test/fixtures/conformance-events.mjs). Format:
// docs/design-events-protocol-2026-10-18.en.md §13.
//
// guarantee: sdk/testing-matches-runtime (events), the testing tool's side.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { host } from '../src/testing.mjs'
import conformance from '../../rutis-runtime/test/fixtures/conformance-events.mjs'

const expected = JSON.parse(readFileSync(new URL('../../../crates/rutis-bridge/src/runtime/testing/events.expected.json', import.meta.url), 'utf8'))
const LANGUAGE = 'node'

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

// Why the tool does not run a case, if it does not.
function skipped(entry) {
  if (entry.tool === false) return 'tool: false'
  for (const [field, run] of [['only', langs => langs.includes(LANGUAGE)], ['except', langs => !langs.includes(LANGUAGE)]]) {
    for (const rule of entry[field] ?? []) {
      for (const [label, langs] of Object.entries(rule)) {
        if (label === 'why') continue
        if (!run(langs)) return `${field} ${label}: ${rule.why}`
      }
    }
  }
  const specs = entry.steps.flatMap(step => step.emit ? [step.emit] : [])
  if (specs.some(spec => spec.via === 'thisArg')) return 'Context.filter is a Cordis plugin\'s; the tool runs leaf plugins'
  return undefined
}

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

// Check `check()` until it passes, at most 5 s (the log may still grow).
async function settled(check) {
  const deadline = Date.now() + 5000
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

function checkOutcome(what, expect, done) {
  if ('returns' in expect) {
    assert.equal(done.thrown, undefined, `${what}: threw ${done.thrown?.stack ?? done.thrown}`)
    const value = done.returns === undefined ? null : done.returns
    assert.deepStrictEqual(value, received(expect.returns), `${what}: returns`)
  }
  if (expect.throws) {
    assert.ok(done.thrown !== undefined, `${what}: should throw, returned ${JSON.stringify(done.returns)}`)
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

function checkLog(what, expect, log, h) {
  if (expect.log) assert.deepStrictEqual(log, expect.log, `${what}: log`)
  if (expect.logUnordered) assert.deepStrictEqual([...log].sort(), [...expect.logUnordered].sort(), `${what}: logUnordered`)
  if (expect.logContains) assert.ok(subsequence(log, expect.logContains), `${what}: log ${JSON.stringify(log)} contains ${JSON.stringify(expect.logContains)}`)
  if (expect.logLacks) for (const entry of expect.logLacks) assert.ok(!log.includes(entry), `${what}: log lacks ${entry}`)
  if (expect.filterSees) assert.deepStrictEqual(log.filter(entry => entry.startsWith('filter:')).map(entry => entry.slice('filter:'.length)), expect.filterSees, `${what}: filterSees`)
  if (expect.reported) {
    assert.deepStrictEqual(h.reported.map(report => ({ label: report.row, name: report.error?.name })), expect.reported, `${what}: reported`)
  }
}

for (const entry of expected.cases) {
  const why = skipped(entry)
  test(`${entry.case} (${entry.rule})`, { skip: why }, async () => {
    const log = []
    const h = host({ services: { 'events-log': { add(item) { log.push(item) } } }, scopes: expected.scopes })
    const modes = entry.steps.flatMap(step => step.emit ? [step.emit.mode] : [])
    const sync = modes.length > 0 && modes.every(mode => mode === 'bail' || mode === 'waterfall-sync')
    const rows = new Map()
    const loadListener = async listener => {
      const participant = { ...listener, name: listener.name ?? entry.name, kind: listener.kind ?? (sync ? 'sync' : 'async') }
      rows.set(listener.label, await h.load(conformance, { row: listener.label, scope: listener.scope, config: participant }))
      participants.set(listener.label, participant)
    }
    const participants = new Map()
    let generation = 0
    for (const listener of entry.listeners) await loadListener(listener)
    const emitters = new Map()
    const emitter = async (label, scope) => {
      const key = !scope || scope === 'root' ? label : `${label}@${scope}`
      if (!emitters.has(key)) {
        const loaded = await h.load(conformance, { row: key, scope, config: { emitter: true, label: key } })
        emitters.set(key, loaded.service(`emitter-${key}`))
      }
      return emitters.get(key)
    }
    const emit = async spec => {
      const full = { ...spec, name: spec.name ?? entry.name }
      const service = await emitter(spec.row ?? 'e', spec.scope)
      return full.mode === 'bail' ? service.dispatch_sync(full) : service.dispatch(full)
    }
    let background
    try {
      for (const [index, step] of entry.steps.entries()) {
        const what = `${entry.case} step ${index}`
        if (step.emit) {
          if (step.background) { background = outcome(emit(step.emit)); continue }
          const done = await outcome(emit(step.emit))
          if (step.expect) {
            checkOutcome(what, step.expect, done)
            await settled(() => checkLog(what, step.expect, log, h))
          }
        } else if (step.update !== undefined) {
          // The row's config changes (a new generation): it registers again.
          await rows.get(step.update).update({ ...participants.get(step.update), generation: ++generation })
        } else if (step.load) {
          await loadListener(step.load)
        } else if (step.unload !== undefined) {
          if (step.when) await settled(() => assert.ok(log.includes(step.when), `${what}: ${step.when} started`))
          const started = Date.now()
          await rows.get(step.unload).unload()
          const done = background ? await background : undefined
          background = undefined
          if (step.expect) {
            if (step.expect.settles) assert.ok(Date.now() - started < 5000, `${what}: settles`)
            if (done) checkOutcome(what, step.expect, done)
            await settled(() => checkLog(what, step.expect, log, h))
          }
        } else {
          throw new Error(`${what}: the tool cannot run ${JSON.stringify(Object.keys(step))}`)
        }
      }
    } finally {
      await h.unload()
    }
  })
}
