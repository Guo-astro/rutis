// The testing tool against the unified events conformance cases
// (crates/rutis-bridge/src/runtime/testing/events.expected.json, #464 step 7),
// which crates/rutis-loader/tests/events_conformance.rs checks on the real
// runtimes. The tool runs every case without `tool: false`, with every
// participant at `X` (language `node-leaf`: the cases `only` / `except`
// exclude for it are skipped), on the leaf form of the plugin
// (node/rutis-runtime/test/fixtures/conformance-events-leaf.mjs; emitter
// rows `e` / `f` load conformance-events-leaf-e.mjs / -f.mjs, which declare
// only their own emitter).
//
// The tool's API for several rows on one bus, written here ahead of it
// (proposed; `load` stays as it is):
//
//   const h = host({ services: { 'events-log': log }, scopes: { t1: 'tenant', 't1/s1': 'session' } })
//   const row = await h.load(plugin, { key, config, scope: 't1/s1' })   // scope undefined: the root
//   row.service(name)          // as Loaded.service
//   await row.unload()
//   await h.close()
//
// `viaHost` and `reported` are about the host and the runtimes' stderr: the
// tool has neither, so those keys are skipped (with a diagnostic).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { host } from '../src/testing.mjs'
import * as rutis from '../src/index.mjs'
import listenerPlugin from '../../rutis-runtime/test/fixtures/conformance-events-leaf.mjs'
import emitterE from '../../rutis-runtime/test/fixtures/conformance-events-leaf-e.mjs'
import emitterF from '../../rutis-runtime/test/fixtures/conformance-events-leaf-f.mjs'

const expected = JSON.parse(readFileSync(new URL('../../../crates/rutis-bridge/src/runtime/testing/events.expected.json', import.meta.url), 'utf8'))
const LANGUAGE = 'node-leaf'
const PATIENCE = 5000
// The plugin a row loads: emitter rows the one declaring their emitter.
const emitterPlugins = { e: emitterE, f: emitterF }
const pluginOf = config => {
  if (!config.emitter) return listenerPlugin
  assert.ok(Object.hasOwn(emitterPlugins, config.label), `emitter rows are e or f, not ${config.label}`)
  return emitterPlugins[config.label]
}

// --- Values as the expectations write them ({"$<tag>": ...}).

// As the case sends them: SDK values for the tags JavaScript lacks, and
// `{"$callback": "upper"}` a function upper-casing its argument.
const writers = {
  $instant: text => new Date(text), $bytes: text => new Uint8Array(Buffer.from(text, 'base64')),
  $map: entries => new Map(entries.map(([key, value]) => [sent(key), sent(value)])),
  $set: elements => new Set(elements.map(sent)), $bigint: BigInt,
  $datetime: rutis.datetime, $date: rutis.date, $time: rutis.time, $decimal: rutis.decimal, $uuid: rutis.uuid,
  $callback: name => {
    assert.equal(name, 'upper', `unknown callback ${name}`)
    return text => text.toUpperCase()
  },
}
// As JavaScript receives them (conformance.test.mjs): an instant a Date to
// the millisecond, bytes a Uint8Array, a map a Map, a set a Set, a bigint a
// bigint, a datetime, date, time, decimal or uuid its text.
const readers = {
  $instant: text => new Date(text.replace(/(\.\d{3})\d+Z$/, '$1Z')),
  $bytes: text => new Uint8Array(Buffer.from(text, 'base64')),
  $map: entries => new Map(entries.map(([key, value]) => [received(key), received(value)])),
  $set: elements => new Set(elements.map(received)),
  $bigint: BigInt,
  $datetime: String, $date: String, $time: String, $decimal: String, $uuid: String,
}
const decoder = table => function decode(value) {
  if (Array.isArray(value)) return value.map(decode)
  if (value === null || typeof value !== 'object') return value
  const keys = Object.keys(value)
  if (keys.length === 1 && Object.hasOwn(table, keys[0])) return table[keys[0]](value[keys[0]])
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decode(item)]))
}
const sent = decoder(writers)
const received = decoder(readers)

// --- The host service `events-log`.

function eventsLog() {
  const entries = []
  const seen = []
  const waiting = new Set()
  const wake = () => { for (const waiter of waiting) waiter() }
  const service = {
    add(entry) { entries.push(entry); wake() },
    filter(description) { seen.push(description); entries.push(`filter:${description.row}`); wake() },
    wait(entry) {
      return new Promise(resolve => {
        const waiter = () => {
          if (!entries.includes(entry)) return
          waiting.delete(waiter)
          resolve()
        }
        waiting.add(waiter)
        waiter()
      })
    },
  }
  return { entries, seen, service }
}

// --- Helpers.

const timeout = (ms, what) => new Promise((_, reject) => setTimeout(() => reject(new Error(`${what}: not within ${ms} ms`)), ms).unref())

// What a call did: its result or its error, and whether it ended in time.
async function outcome(make) {
  const done = (async () => {
    try {
      return { returns: await make() }
    } catch (thrown) {
      return { thrown }
    }
  })()
  const late = new Promise(resolve => setTimeout(() => resolve({ late: true }), PATIENCE).unref())
  const result = await Promise.race([done, late])
  return { ...result, settled: !result.late }
}

async function eventually(check) {
  const until = Date.now() + PATIENCE
  for (;;) {
    try {
      return check()
    } catch (error) {
      if (Date.now() > until) throw error
      await new Promise(resolve => setTimeout(resolve, 10))
    }
  }
}

// Whether `entries` holds `wanted` in this order, others between.
function contains(entries, wanted) {
  let i = 0
  for (const entry of entries) if (entry === wanted[i]) i++
  return i === wanted.length
}

// Whether a case runs for `node-leaf` at every location (`only` / `except`).
function excluded(c) {
  const all = entry => Object.entries(entry).filter(([key]) => key !== 'why').every(([, languages]) => languages.includes(LANGUAGE))
  const notOnly = (c.only ?? []).find(entry => !all(entry))
  if (notOnly) return `only: ${notOnly.why}`
  const except = (c.except ?? []).find(all)
  return except ? `except: ${except.why}` : undefined
}

// The emit specs of a case's steps (a listener's `does.emits` is not a step).
function* specsOf(c) {
  const steps = [...c.steps]
  while (steps.length) {
    const step = steps.shift()
    if (step.concurrent) steps.push(...step.concurrent)
    if (step.emit) yield step.emit
  }
}

// A participant as the runner resolves it: name, kind and log filled in.
function resolved(c, listener) {
  const specs = [...specsOf(c)]
  const sync = specs.length > 0 && specs.every(spec => spec.mode === 'bail' || spec.mode === 'waterfall-sync')
  const { at, scope, typed, ...config } = listener
  config.name ??= c.name
  config.kind ??= sync ? 'sync' : 'async'
  config.log ??= 'label'
  if (config.does?.emits) config.does = { ...config.does, emits: specOf(c, config.does.emits) }
  return config
}

// An emit spec as the plugin takes it.
function specOf(c, spec) {
  const { from, row, scope, args = [], ...rest } = spec
  return { ...rest, name: spec.name ?? c.name, args: sent(args) }
}

const scopeOf = path => path === undefined || path === 'root' ? undefined : path

// --- One case.

async function run(t, c) {
  const log = eventsLog()
  const h = host({ services: { 'events-log': log.service }, scopes: expected.scopes })
  // label -> { row, scope, config }
  const rows = new Map()
  const load = async (config, scope) => {
    const row = await h.load(pluginOf(config), { key: config.label, config, scope })
    rows.set(config.label, { row, scope, config })
    return row
  }
  const unload = async label => {
    const entry = rows.get(label)
    assert.ok(entry, `no row ${label} is loaded`)
    rows.delete(label)
    await entry.row.unload()
  }
  // The emitter row a spec names, at its scope: one row per label.
  const emitter = async spec => {
    const label = spec.row ?? 'e'
    const scope = scopeOf(spec.scope)
    const entry = rows.get(label)
    if (entry && entry.scope === scope) return entry.row
    if (entry) await unload(label)
    return await load({ label, emitter: true }, scope)
  }
  const emit = async spec => {
    const service = (await emitter(spec)).service(`emitter-${spec.row ?? 'e'}`)
    const plain = specOf(c, spec)
    return () => spec.mode === 'bail' ? service.dispatch_sync(plain) : service.dispatch(plain)
  }

  try {
    for (const listener of c.listeners) await load(resolved(c, listener), scopeOf(listener.scope))
    let background
    for (const [i, step] of c.steps.entries()) {
      const what = `steps[${i}]`
      let done
      if (step.emit) {
        const dispatching = outcome(await emit(step.emit))
        if (step.background) {
          background = dispatching
          continue
        }
        done = await dispatching
      } else if (step.load) {
        done = await outcome(() => load(resolved(c, step.load), scopeOf(step.load.scope)))
      } else if (step.unload) {
        if (step.when) await Promise.race([log.service.wait(step.when), timeout(PATIENCE, `${what}: ${step.when} logged`)])
        const unloading = await outcome(() => unload(step.unload))
        assert.equal(unloading.thrown, undefined, `${what}: unload ${step.unload}: ${unloading.thrown?.stack}`)
        // The expectations of the step are about the background dispatch.
        if (background) {
          done = await background
          done = { ...done, settled: done.settled && unloading.settled }
          background = undefined
        } else {
          done = unloading
        }
      } else if (step.update) {
        const { scope, config } = rows.get(step.update)
        await unload(step.update)
        done = await outcome(() => load(config, scope))
      } else {
        assert.fail(`${what}: the tool has no step ${Object.keys(step).join(', ')}`)
      }
      if (step.expect) await check(t, `${what}`, step.expect, done, log)
    }
    // A background dispatch no unload step waited for still ends (or times out).
    if (background) await background
  } finally {
    await h.close()
  }
}

async function check(t, what, expect, done, log) {
  for (const key of ['viaHost', 'reported']) {
    if (key in expect) t.diagnostic(`${what}: ${key} is not checked by the testing tool (no host)`)
  }
  if ('settles' in expect) assert.equal(done.settled, expect.settles, `${what}: settles within ${PATIENCE} ms`)
  if ('returns' in expect) {
    assert.ok(done.settled, `${what}: did not end within ${PATIENCE} ms`)
    assert.equal(done.thrown, undefined, `${what}: expected ${JSON.stringify(expect.returns)}, got ${done.thrown?.stack ?? done.thrown}`)
    if (expect.returns === null) assert.ok(done.returns == null, `${what}: no result expected, got ${String(done.returns)}`)
    else assert.deepStrictEqual(done.returns, received(expect.returns), `${what}: the result`)
  }
  if ('throws' in expect) {
    assert.ok(done.settled, `${what}: did not end within ${PATIENCE} ms`)
    assert.ok('thrown' in done, `${what}: expected an error, got ${String(done.returns)}`)
    thrown(what, done.thrown, expect.throws)
  }
  if (expect.log) await eventually(() => assert.deepStrictEqual(log.entries, expect.log, `${what}: the log`))
  if (expect.logUnordered) {
    const sorted = list => [...list].sort()
    await eventually(() => assert.deepStrictEqual(sorted(log.entries), sorted(expect.logUnordered), `${what}: the log, in any order (${JSON.stringify(log.entries)})`))
  }
  if (expect.logContains) {
    await eventually(() => assert.ok(contains(log.entries, expect.logContains), `${what}: the log ${JSON.stringify(log.entries)} holds ${JSON.stringify(expect.logContains)} in order`))
  }
  for (const entry of expect.logLacks ?? []) assert.ok(!log.entries.includes(entry), `${what}: the log has ${entry}`)
  if (expect.filterSees) assert.deepStrictEqual(log.seen.map(description => description.row), expect.filterSees, `${what}: the rows the filter saw`)
}

function thrown(what, error, { name, message, errors }) {
  if (name !== undefined) assert.equal(error?.name, name, `${what}: the error's name (${error?.stack ?? error})`)
  if (message !== undefined) assert.ok(String(error?.message).includes(message), `${what}: the error's message ${JSON.stringify(error?.message)} has ${JSON.stringify(message)}`)
  if (errors !== undefined) {
    assert.ok(Array.isArray(error?.errors), `${what}: an AggregateError's errors`)
    assert.equal(error.errors.length, errors.length, `${what}: the number of errors`)
    errors.forEach((child, i) => thrown(`${what} errors[${i}]`, error.errors[i], child))
  }
}

for (const c of expected.cases) {
  if (c.tool === false) continue
  const skip = excluded(c)
  test(`${c.case} (${c.rule})`, { skip }, t => run(t, c))
}
