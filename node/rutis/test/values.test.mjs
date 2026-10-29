// The tagged values (#308): the SDK constructors, and the values crossing
// between the test and a plugin on a real host
// (docs/design-value-types-2026-10-15.md).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import * as rutis from '../src/advanced.mjs'
import { load } from '../src/testing.mjs'
import echoPlugin from './fixtures/echo.mjs'

test('the constructors check their text and mark their value', () => {
  const at = rutis.datetime('2026-10-15T14:30:05.25')
  assert.equal(at.value, '2026-10-15T14:30:05.25')
  assert.equal(at[rutis.VALUE], 'datetime')
  assert.equal(at[Symbol.for('rutis.value')], 'datetime')
  assert.equal(String(at), '2026-10-15T14:30:05.25')
  assert.equal(JSON.stringify({ at }), '{"at":"2026-10-15T14:30:05.25"}')
  assert.ok(Object.isFrozen(at))
  // Seconds may be left out; the wire always has them.
  assert.equal(rutis.datetime('2026-10-15T14:30').value, '2026-10-15T14:30:00')
  assert.equal(rutis.time('14:30').value, '14:30:00')
  assert.equal(rutis.time('23:59:59.999999999').value, '23:59:59.999999999')
  assert.equal(rutis.date('2000-02-29').value, '2000-02-29')
  // Trailing zeros are a different decimal.
  assert.equal(rutis.decimal('1.10').value, '1.10')
  assert.equal(rutis.decimal(-12n).value, '-12')
  assert.equal(rutis.uuid('0F8FAD5B-D9CB-469F-A165-70867728950E').value, '0f8fad5b-d9cb-469f-a165-70867728950e')
  for (const [make, text] of [
    [rutis.datetime, '2026-10-15T14:30:05Z'], [rutis.datetime, '2026-10-15 14:30:05'], [rutis.datetime, '0000-01-01T00:00:00'],
    [rutis.date, '2026-02-29'], [rutis.date, '2026-13-01'], [rutis.date, '10000-01-01'], [rutis.date, '2026-10-15T00:00:00'],
    [rutis.time, '24:00:00'], [rutis.time, '14:30:60'], [rutis.time, '14:30:05.1234567891'], [rutis.time, '14:30:05+01:00'],
    [rutis.decimal, '1e3'], [rutis.decimal, 'NaN'], [rutis.decimal, '.5'], [rutis.decimal, '01.5'], [rutis.decimal, '1'.repeat(4301)],
    [rutis.uuid, '0f8fad5bd9cb469fa16570867728950e'], [rutis.uuid, '{0f8fad5b-d9cb-469f-a165-70867728950e}'],
    [rutis.date, 20261015], [rutis.decimal, 1.5],
  ]) {
    assert.throws(() => make(text), TypeError, `${make.name}(${String(text).slice(0, 40)})`)
  }
})

// A service that echoes what it is given, in the real runtime.
let loaded
async function echo() {
  loaded ??= load(echoPlugin)
  return (await loaded).service('echo')
}

test('tagged values cross as the runtimes send and receive them', async () => {
  const { echo: back, async } = await echo()
  // A Date: copied, to the millisecond.
  const when = new Date('2026-10-15T03:22:15.123Z')
  const date = back(when)
  assert.notEqual(date, when)
  assert.deepEqual(date, when)
  assert.throws(() => back(new Date(NaN)), /invalid Date/)
  assert.throws(() => back(new Date(Date.UTC(10000, 0, 1))), /within years 0001–9999/)
  assert.throws(() => back(new Date(-62135596800001)), /within years 0001–9999/)
  assert.deepEqual(back(new Date(-62135596800000)), new Date('0001-01-01T00:00:00Z'))
  // Binary data: a Uint8Array, copied.
  const bytes = new Uint8Array([1, 2, 3])
  const copy = back(bytes)
  assert.notEqual(copy, bytes)
  assert.deepEqual(copy, bytes)
  assert.deepEqual(back(Buffer.from('hi')), new Uint8Array([104, 105]))
  assert.deepEqual(back(new Uint8Array([1, 2, 3, 4]).buffer), new Uint8Array([1, 2, 3, 4]))
  assert.deepEqual(back(new DataView(new Uint8Array([1, 2, 3, 4]).buffer, 1, 2)), new Uint8Array([2, 3]))
  assert.throws(() => back(new Int16Array([1])), /cannot send a Int16Array: binary data crosses as a Uint8Array/)
  // A bigint.
  assert.equal(back(-(10n ** 30n)), -(10n ** 30n))
  assert.throws(() => back(10n ** 4300n), /at most 4300 digits/)
  // The constructors' values arrive as their text.
  const texts = back([rutis.date('2026-10-15'), rutis.decimal('1.10')])
  assert.deepEqual(texts, ['2026-10-15', '1.10'])
  assert.ok(texts.every(text => typeof text === 'string'), 'plain strings, not objects')
  assert.equal(back(5n), 5n)
  // In plain objects and arrays, next to undefined, which is null inside a
  // value as on the runtimes (#687 §七).
  assert.deepEqual(back({ at: when, none: undefined, list: [1n] }), { at: when, none: null, list: [1n] })
  assert.deepEqual(await async(new Map([[1, 2n]])), new Map([[1, 2n]]))
})

test('maps and sets keep their order; keys follow design §5', async () => {
  const { echo: back } = await echo()
  const map = new Map([[1, 'a'], ['1', 'b'], [true, undefined], [2n, new Map([['x', [new Date(0)]]])]])
  const crossed = back(map)
  assert.notEqual(crossed, map)
  assert.deepEqual([...crossed], [[1, 'a'], ['1', 'b'], [true, null], [2n, new Map([['x', [new Date(0)]]])]])
  assert.deepEqual([...back(new Set(['b', 'a', 3]))], ['b', 'a', 3])
  assert.deepEqual(back(new Map([[rutis.date('2026-10-15'), 1], [rutis.uuid('0f8fad5b-d9cb-469f-a165-70867728950e'), 2]])),
    new Map([['2026-10-15', 1], ['0f8fad5b-d9cb-469f-a165-70867728950e', 2]]))
  // Not keys in every language.
  for (const key of [new Date(0), new Uint8Array(), null, undefined, [], {}, new Map(), new Set(), () => {}, NaN]) {
    assert.throws(() => back(new Map([[key, 1]])), /map key cannot be/, String(key))
    assert.throws(() => back(new Set([key])), /set element cannot be/, String(key))
  }
  // Integers beyond ±(2^53−1): a bigint instead.
  assert.throws(() => back(new Map([[2 ** 53, 1]])), /beyond ±\(2\^53-1\): send it as a bigint/)
  assert.throws(() => back(new Set([-(2 ** 53)])), /set element cannot be the integer/)
  back(new Set([2 ** 53 - 1, -(2 ** 53 - 1), 0.5]))
  // Equal values: they would be one key somewhere.
  assert.throws(() => back(new Set([rutis.date('2026-10-15'), rutis.date('2026-10-15')])), /duplicate set element/)
  assert.throws(() => back(new Set([rutis.time('14:30:05.25'), rutis.time('14:30:05.250')])), /duplicate set element/)
  back(new Set([rutis.decimal('1.1'), rutis.decimal('1.10'), 1, '1', 1n, true]))
  // Different values that one JavaScript key would merge once received.
  assert.throws(() => back(new Map([['2026-10-15', 1], [rutis.date('2026-10-15'), 2]])), /duplicate map key once received/)
  // Deep maps count levels as arrays do (#279).
  const nest = depth => Array.from({ length: depth }).reduce(inner => new Map([['k', inner]]), 'bottom')
  back(nest(64))
  assert.throws(() => back(nest(65)), /nested deeper than 64 levels/)
  const cyclic = new Map()
  cyclic.set('self', cyclic)
  assert.throws(() => back(cyclic), /nested deeper than 64 levels/)
})
