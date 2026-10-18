// The testing tool against the real runtime: the same plugin, the same calls,
// the same expected results (crates/rutis-bridge/src/runtime/testing/
// weather.expected.json), which rutis_bridge::runtime::testing::runtime
// checks on the real Node runtime.
//
// guarantee: sdk/testing-matches-runtime (risk A1, Q6.7.2), the testing tool's
// side. Entries marked `allowed` are the written-down differences
// (docs/guide/plugin-api.md); the test says which it skips and why.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { load } from '../src/testing.mjs'
import weather from '../../rutis-runtime/test/fixtures/conformance-weather.mjs'

const expected = JSON.parse(readFileSync(new URL('../../../crates/rutis-bridge/src/runtime/testing/weather.expected.json', import.meta.url), 'utf8'))

// The clock the plugin injects: 0, 1, 2, ...
function clock() {
  let next = 0
  return { now: () => next++ }
}

// An argument as the expectations write it: data, or a callback.
function argument(arg) {
  const callback = arg?.$callback
  if (callback === undefined) return arg
  if (callback === 'upper') return day => day.toUpperCase()
  const { name, message, fields } = callback.throws
  return () => { throw Object.assign(new Error(message), { name }, fields) }
}

// A value as the expectations write it ({"$<tag>": ...} for a tagged value,
// `about` in the file), as JavaScript receives it: an instant as a Date
// to the millisecond, bytes as a Uint8Array, a map as a Map, a set as a
// Set, a bigint as a bigint, a datetime, date, time, decimal or uuid as its
// text.
const readers = {
  $instant: text => new Date(text.replace(/(\.\d{3})\d+Z$/, '$1Z')),
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

// What the call did, as the expectations write it.
async function outcome(make) {
  try {
    return { returns: await make() }
  } catch (thrown) {
    return { thrown }
  }
}

function check(what, entry, { returns, thrown }, args = []) {
  const why = `${what} (${entry.why})`
  if ('returns' in entry) {
    assert.equal(thrown, undefined, `${why}: expected ${JSON.stringify(entry.returns)}, got ${thrown?.stack ?? thrown}`)
    let value = returns
    if (entry.ignore && value !== null && typeof value === 'object') {
      value = { ...value }
      for (const key of entry.ignore) delete value[key]
    }
    assert.deepStrictEqual(value, received(entry.returns), why)
    if (entry.copied) assert.ok(args.every(arg => arg !== returns), `${why}: a copy, not the object passed in`)
  } else {
    assert.ok(thrown !== undefined, `${why}: expected an error, got ${JSON.stringify(returns)}`)
    const { name, message } = entry.throws
    if (name !== undefined) assert.equal(thrown.name, name, `${why}: the error's name`)
    if (message !== undefined) assert.equal(thrown.message, message, `${why}: the error's message`)
    // What crosses is the name and message, not the object thrown.
    for (const arg of entry.args ?? []) {
      for (const field of Object.keys(arg?.$callback?.throws?.fields ?? {})) {
        assert.equal(thrown[field], undefined, `${why}: ${field} does not cross`)
      }
    }
  }
}

test('the testing tool returns what the real runtime does (weather.expected.json)', async t => {
  const plugin = await load(weather, { services: { clock: clock() }, emits: expected.emits })
  const service = plugin.service(expected.service)
  for (const [method, kind] of Object.entries(expected.methods)) {
    assert.equal(weather.provides[expected.service][method], kind, `the plugin declares ${method} as ${kind}`)
  }
  for (const [i, entry] of expected.calls.entries()) {
    const what = `calls[${i}] ${entry.call}`
    if (entry.allowed) {
      t.diagnostic(`${what}: not made here, an allowed difference: ${entry.allowed}`)
      continue
    }
    const args = entry.args.map(argument)
    const done = await outcome(() => service[entry.call](...args))
    await t.test(what, () => check(what, entry, done, args))
  }
  const names = service.samples()
  assert.ok(names.length > 0, 'the Node plugin has samples')
  for (const name of names) {
    const entry = expected.samples.find(entry => entry.sample === name)
    assert.ok(entry, `the expectations list sample ${name}`)
    if (entry.allowed) {
      t.diagnostic(`sample ${name}: an allowed difference: ${entry.allowed}`)
      continue
    }
    const done = await outcome(() => service.sample(name))
    await t.test(`sample ${name}`, () => check(`sample ${name}`, entry, done))
  }
  for (const [i, entry] of expected.events.entries()) {
    const what = `events[${i}] ${entry.emit}`
    const emitted = await outcome(() => plugin.emit(entry.emit, ...entry.args))
    if (entry.then) {
      await t.test(`${what} emitted`, () => assert.equal(emitted.thrown, undefined, `${what}: ${emitted.thrown?.stack}`))
      const done = await outcome(() => service[entry.then.call](...entry.then.args.map(argument)))
      await t.test(what, () => check(what, { ...entry.then, why: entry.why }, done))
    } else {
      await t.test(what, () => check(what, entry, emitted))
    }
  }
  // A service the plugin does not declare: its methods inferred, all async.
  const { undeclared } = expected
  assert.equal(weather.provides[undeclared.service], undefined, `the plugin does not declare ${undeclared.service}`)
  assert.deepStrictEqual(plugin.methods(undeclared.service), undeclared.methods, 'the methods rutis sees')
  const forecast = plugin.service(undeclared.service)
  for (const [i, entry] of undeclared.calls.entries()) {
    const what = `undeclared.calls[${i}] ${entry.call}`
    const call = forecast[entry.call](...entry.args.map(argument))
    assert.ok(call instanceof Promise, `${what}: async`)
    await t.test(what, async () => check(what, entry, await outcome(() => call)))
  }
  await plugin.unload()
})
