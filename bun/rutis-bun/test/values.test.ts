// The tagged values (#308) in the Bun session: what the session sends for a Date, binary data,
// a Map, a Set, a bigint and the SDK constructors' values, what it makes of
// each tag it receives, and what fails only the call that carries a value
// (docs/design-value-types-2026-10-15.md; protocol design §6.1). The
// cross-language round trip is crates/rutis-bridge/tests/value_roundtrip.rs.
import { test } from 'bun:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { Session, CAPABILITIES } from '../src/session.ts'
import { base64Decode, base64Encode, checkText, instantFromText, instantText, VALUE_CAPABILITIES } from '../src/values.ts'

const PROTOCOL = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).rutisProtocol
const data = value => ({ type: 'data', value })
// What the SDK constructors make (@arcships/rutis), without the SDK.
const constructed = (tag, value) => Object.freeze({ [Symbol.for('rutis.value')]: tag, value })

// A compat session whose far end declared `capabilities`.
function harness(dispatch = () => {}, capabilities = VALUE_CAPABILITIES) {
  const sent = [], incoming = []
  let fault
  const peer = new Session({
    dispatch,
    send: line => sent.push(JSON.parse(line)),
    pump: () => { assert.ok(incoming.length, 'sync caller must not strand'); peer.receive(incoming.shift()) },
    abort: error => { fault = error },
  })
  peer.start()
  peer.receive({ op: 'hello', version: PROTOCOL, capabilities })
  return { peer, sent, incoming, fault: () => fault }
}
// The arguments a call on `peer` sends with `args`.
function sentArgs(args, capabilities) {
  const { peer, sent, incoming } = harness(undefined, capabilities)
  incoming.push({ op: 'return', id: 'node:1', value: data(null) })
  peer.invoke('test', 'm', args)
  return sent.find(frame => frame.op === 'invoke').args.value
}
// What `peer` makes of a result `value`.
function received(value) {
  const { peer, incoming, fault } = harness()
  incoming.push({ op: 'return', id: 'node:1', value })
  try {
    return peer.invoke('test', 'm', [])
  } finally {
    assert.equal(fault(), undefined)
  }
}

test('the session declares every tagged value in both formats', () => {
  for (const tag of ['instant', 'datetime', 'date', 'time', 'bytes', 'map', 'set', 'bigint', 'decimal', 'uuid']) {
    assert.ok(CAPABILITIES.includes(`value-${tag}`), tag)
  }
  const { sent } = harness()
  assert.deepEqual(sent[0].capabilities.filter(name => name.startsWith('value-')), VALUE_CAPABILITIES)
})

test('JavaScript values cross under their tags', () => {
  const [instant, bytes, buffer, view, map, set, big, datetime, date, time, decimal, uuid] = sentArgs([
    new Date('2026-10-15T03:22:15.120Z'), new Uint8Array([104, 105]), Buffer.from('hello'),
    new DataView(new Uint8Array([0, 255, 254, 0]).buffer, 1, 2),
    new Map([[1, 'a'], ['1', undefined]]), new Set([true, 2n]), -123456789012345678901n,
    constructed('datetime', '2026-10-15T14:30:05.25'), constructed('date', '2026-10-15'), constructed('time', '14:30:05'),
    constructed('decimal', '1.10'), constructed('uuid', '0f8fad5b-d9cb-469f-a165-70867728950e'),
  ])
  assert.deepEqual(instant, { type: 'instant', value: '2026-10-15T03:22:15.12Z' })
  assert.deepEqual(bytes, { type: 'bytes', value: 'aGk=' })
  assert.deepEqual(buffer, { type: 'bytes', value: 'aGVsbG8=' })
  assert.deepEqual(view, { type: 'bytes', value: '//4=' })
  assert.deepEqual(map, { type: 'map', value: [[data(1), data('a')], [data('1'), data(null)]] })
  assert.deepEqual(set, { type: 'set', value: [data(true), { type: 'bigint', value: '2' }] })
  assert.deepEqual(big, { type: 'bigint', value: '-123456789012345678901' })
  assert.deepEqual([datetime, date, time, decimal, uuid].map(wire => wire.type), ['datetime', 'date', 'time', 'decimal', 'uuid'])
  assert.equal(decimal.value, '1.10')
  // Inside objects and arrays: field by field, as with references.
  const [record] = sentArgs([{ at: new Date(0), list: [1n], plain: { a: 1 } }])
  assert.deepEqual(record, { type: 'record', value: {
    at: { type: 'instant', value: '1970-01-01T00:00:00Z' },
    list: { type: 'list', value: [{ type: 'bigint', value: '1' }] },
    plain: data({ a: 1 }),
  } })
})

test('a value the far end did not declare fails only its call', () => {
  const { peer, sent, incoming, fault } = harness(undefined, [])
  for (const [value, capability] of [[new Date(0), 'value-instant'], [new Map(), 'value-map'], [[1n], 'value-bigint'], [{ b: new Uint8Array() }, 'value-bytes'], [constructed('uuid', '0f8fad5b-d9cb-469f-a165-70867728950e'), 'value-uuid']]) {
    assert.throws(() => peer.invoke('test', 'm', [value]), new RegExp(`did not declare the capability ${capability}`))
  }
  // Nothing was sent, and the session goes on.
  assert.equal(sent.filter(frame => frame.op === 'invoke').length, 0)
  incoming.push({ op: 'return', id: 'node:6', value: data('fine') })
  assert.equal(peer.invoke('test', 'm', ['plain']), 'fine')
  assert.equal(fault(), undefined)
  // A result it did not declare: the caller receives a throw.
  const served = harness(() => new Map([[1, 2]]), [])
  served.peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'm', args: data([]) })
  return Promise.resolve().then(() => {
    const reply = served.sent.at(-1)
    assert.equal(reply.op, 'throw')
    assert.match(reply.error.message, /value-map/)
    assert.equal(served.fault(), undefined)
  })
})

test('values this side cannot send fail before they are sent', () => {
  const { peer, fault } = harness()
  for (const [value, message] of [
    [new Date(NaN), /invalid Date/],
    [new Date(Date.UTC(10000, 0, 1)), /within years 0001–9999/],
    [new Int16Array(2), /Int16Array: binary data crosses as a Uint8Array/],
    [new Map([[new Date(0), 1]]), /map key cannot be an instant/],
    [new Set([null]), /set element cannot be null/],
    [new Map([[2 ** 53, 1]]), /beyond ±\(2\^53-1\): send it as a bigint/],
    [new Set([constructed('date', '2026-10-15'), constructed('date', '2026-10-15')]), /duplicate set element/],
    [new Set([constructed('time', '14:30:05.25'), constructed('time', '14:30:05.250')]), /duplicate set element/],
    [constructed('date', '2026-02-30'), /invalid date/],
    [10n ** 4300n, /at most 4300 digits/],
  ]) {
    assert.throws(() => peer.invoke('test', 'm', [value]), message)
  }
  assert.equal(fault(), undefined)
})

test('each tag arrives as its JavaScript value', () => {
  assert.deepEqual(received({ type: 'instant', value: '2026-10-15T03:22:15.123456789Z' }), new Date('2026-10-15T03:22:15.123Z'))
  // Truncated toward negative infinity (design V7).
  assert.equal(received({ type: 'instant', value: '1969-12-31T23:59:59.9999Z' }).getTime(), -1)
  assert.equal(received({ type: 'instant', value: '0001-01-01T00:00:00Z' }).getTime(), -62135596800000)
  assert.equal(received({ type: 'instant', value: '9999-12-31T23:59:59.999999999Z' }).getTime(), 253402300799999)
  for (const [tag, text] of [['datetime', '2026-10-15T14:30:05.250'], ['date', '2026-10-15'], ['time', '14:30:05'], ['decimal', '-12.3400'], ['uuid', '0f8fad5b-d9cb-469f-a165-70867728950e']]) {
    assert.equal(received({ type: tag, value: text }), text, tag)
  }
  assert.equal(received({ type: 'bigint', value: '-123456789012345678901' }), -123456789012345678901n)
  const bytes = received({ type: 'bytes', value: 'aGVsbG8=' })
  assert.equal(bytes.constructor, Uint8Array)
  assert.deepEqual(bytes, new TextEncoder().encode('hello'))
  const map = received({ type: 'map', value: [[data(2), data('b')], [{ type: 'bigint', value: '1' }, { type: 'undefined' }], [{ type: 'date', value: '2026-10-15' }, { type: 'set', value: [data('x')] }]] })
  assert.deepEqual([...map], [[2, 'b'], [1n, null], ['2026-10-15', new Set(['x'])]])
  assert.deepEqual(received({ type: 'list', value: [{ type: 'instant', value: '1970-01-01T00:00:00Z' }, data(null)] }), [new Date(0), null])
})

test('a value not in its format, or keys that collide, fail only the call', () => {
  const refused = [
    [{ type: 'instant', value: '2026-10-15T03:22:15' }, /invalid instant/],
    [{ type: 'instant', value: '2026-10-15T03:22:15+00:00' }, /invalid instant/],
    [{ type: 'datetime', value: '2026-10-15T14:30:05Z' }, /invalid datetime/],
    [{ type: 'date', value: '1900-02-29' }, /invalid date/],
    [{ type: 'time', value: '24:00:00' }, /invalid time/],
    [{ type: 'bytes', value: 'Zh==' }, /invalid bytes/],
    [{ type: 'bigint', value: '-0' }, /invalid bigint/],
    [{ type: 'decimal', value: '1e3' }, /invalid decimal/],
    [{ type: 'uuid', value: '0F8FAD5B-D9CB-469F-A165-70867728950E' }, /invalid uuid/],
    // Keys that are one key somewhere: 1 and 1.0 (equal numbers), or two
    // values one JavaScript key once received.
    [{ type: 'map', value: [[data(1), data('a')], [data(1.0), data('b')]] }, /duplicate map key 1/],
    [{ type: 'set', value: [data('2026-10-15'), { type: 'date', value: '2026-10-15' }] }, /duplicate set element once received/],
    [{ type: 'set', value: [{ type: 'time', value: '14:30:05.25' }, { type: 'time', value: '14:30:05.250' }] }, /duplicate set element/],
    [{ type: 'map', value: [[{ type: 'instant', value: '1970-01-01T00:00:00Z' }, data(1)]] }, /map key cannot be an instant/],
    [{ type: 'map', value: [[data(null), data(1)]] }, /map key cannot be null/],
    [{ type: 'set', value: [data([1])] }, /set element cannot be an array/],
    [{ type: 'set', value: [data(9007199254740992)] }, /beyond ±\(2\^53-1\)/],
    // The first refused value is named, wherever it is.
    [{ type: 'list', value: [data(1), { type: 'map', value: [[data('k'), { type: 'bytes', value: 'Zg' }]] }] }, /invalid bytes/],
  ]
  for (const [value, message] of refused) assert.throws(() => received(value), message, JSON.stringify(value))
})

test('a refused call is answered with a throw and releases every reference it carried', async () => {
  let calls = 0
  const { peer, sent, fault } = harness(() => ++calls)
  const callback = id => ({ type: 'reference', value: { id, kind: 'function', home: false, origin: [] } })
  // A callback granted twice and one granted once, around a value refused.
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'm', args: { type: 'list', value: [callback(7), { type: 'bytes', value: 'not base64' }, callback(7), { type: 'map', value: [[data('k'), callback(8)]] }] } })
  const reply = sent.find(frame => frame.id === 'rust:1')
  assert.equal(reply.op, 'throw')
  assert.match(reply.error.message, /invalid bytes/)
  assert.deepEqual(sent.filter(frame => frame.op === 'release').sort((a, b) => a.reference - b.reference), [
    { op: 'release', reference: 7, count: 2 },
    { op: 'release', reference: 8, count: 1 },
  ])
  assert.deepEqual(peer.references(), { exports: 0, imports: 0 })
  assert.equal(calls, 0)
  // A reference this side already holds keeps the grants it had.
  const { peer: holder, sent: holderSent, incoming } = harness()
  incoming.push({ op: 'return', id: 'node:1', value: callback(9) })
  const kept = holder.invoke('test', 'm', [])
  holder.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'm', args: { type: 'list', value: [callback(9), { type: 'uuid', value: 'x' }] } })
  assert.deepEqual(holderSent.filter(frame => frame.op === 'release'), [{ op: 'release', reference: 9, count: 1 }])
  assert.deepEqual(holder.references(), { exports: 0, imports: 1 })
  holder.release(kept)
  assert.deepEqual(holderSent.at(-1), { op: 'release', reference: 9, count: 1 })
  // The session goes on.
  peer.receive({ op: 'invoke', id: 'rust:2', path: [], target: 'test', method: 'm', args: data([]) })
  await Promise.resolve()
  assert.deepEqual(sent.at(-1), { op: 'return', id: 'rust:2', value: data(1) })
  assert.equal(fault(), undefined)
  // A refused result releases its references too, and fails the call.
  const { peer: caller, sent: callerSent, incoming: replies, fault: callerFault } = harness()
  replies.push({ op: 'return', id: 'node:1', value: { type: 'list', value: [callback(3), { type: 'date', value: '2026-13-01' }] } })
  assert.throws(() => caller.invoke('test', 'm', []), /invalid date/)
  assert.deepEqual(callerSent.filter(frame => frame.op === 'release'), [{ op: 'release', reference: 3, count: 1 }])
  assert.equal(callerFault(), undefined)
})

test('a tagged value not in the protocol\'s shape ends the session', () => {
  for (const value of [
    { type: 'instant', value: 0 },
    { type: 'bytes' },
    { type: 'map', value: [[data(1)]] },
    { type: 'map', value: {} },
    { type: 'set', value: 'a' },
  ]) {
    const { peer, incoming, fault } = harness()
    incoming.push({ op: 'return', id: 'node:1', value })
    assert.throws(() => peer.invoke('test', 'm', []), JSON.stringify(value))
    assert.ok(fault(), JSON.stringify(value))
  }
})

test('the text formats are those of the protocol, strictly', () => {
  for (const text of ['2026-10-15T03:22:15', '2026-10-15T03:22:15z', '2026-10-15t03:22:15Z', '2026-10-15 03:22:15Z', '2026-10-15T03:22:15.Z', '2026-10-15T03:22:15.1234567891Z', '2026-10-15T03:22:60Z', '0000-12-31T23:59:59Z', '10000-01-01T00:00:00Z', '+2026-10-15T03:22:15Z', '2026-1-15T03:22:15Z', '２０２６-10-15T03:22:15Z', '']) {
    assert.throws(() => instantFromText(text), /invalid instant/, text)
  }
  for (const [date, text] of [[new Date(0), '1970-01-01T00:00:00Z'], [new Date(-1), '1969-12-31T23:59:59.999Z'], [new Date('0001-01-01T00:00:00.5Z'), '0001-01-01T00:00:00.5Z']]) {
    assert.equal(instantText(date), text)
    assert.equal(instantFromText(text).getTime(), date.getTime())
  }
  for (const text of ['0', '-1', '9'.repeat(4300)]) assert.equal(checkText('bigint', text), text)
  for (const text of ['', '-', '-0', '01', '+1', '1.0', '1e3', ' 1', '0x10', '١', '9'.repeat(4301)]) assert.throws(() => checkText('bigint', text), /invalid bigint/, text)
  for (const text of ['0', '-0', '-0.0', '1.10', '0.000001']) assert.equal(checkText('decimal', text), text)
  for (const text of ['', '.5', '5.', '1E-3', 'NaN', 'Infinity', '00.1', '+1', '1.2.3', '1,5', `${'1'.repeat(4290)}.${'0'.repeat(11)}`]) assert.throws(() => checkText('decimal', text), /invalid decimal/, text)
  for (const [bytes, text] of [[[], ''], [[102], 'Zg=='], [[102, 111], 'Zm8='], [[102, 111, 111], 'Zm9v'], [[0xff, 0xfe, 0], '//4A']]) {
    assert.equal(base64Encode(new Uint8Array(bytes)), text)
    assert.deepEqual(base64Decode(text), new Uint8Array(bytes))
  }
  for (const text of ['Zg', 'Zg=', 'Zh==', 'Zg==Zg==', 'Z===', 'Zm9v\n', 'Zm-v', 'Zm_v', '====', 'Zm9=']) assert.throws(() => base64Decode(text), /invalid bytes/, text)
})
