// Dialing out (dial-out design §六): how failures are classified, the
// backoff, and the loop that dials again, against stand-ins for the dial
// and the session; what is checked before dialing. The rules against a
// rutis access point are crates/rutis-host/tests/dial_conformance.rs.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { AUTH_REJECTED, Backoff, INCOMPATIBLE, keepDialing, RETRY, retryFrom, RETRYABLE, seconds, sessionFailure } from '../src/dialing.mjs'
import { dialOptions } from '../src/dial.mjs'
import { greetingOf } from '../src/session.mjs'
import { within } from './support.mjs'

// Short enough that the loop runs at once; the shape is what is checked.
const FAST = { ...RETRY, initial: 1, max: 4, rejected: 2, handshake: 1000 }

test('the backoff doubles to its cap within jitter, and resets', () => {
  for (const random of [() => 0, () => 0.999, Math.random]) {
    const backoff = new Backoff(RETRY, random)
    const steps = Array.from({ length: 10 }, () => backoff.next())
    for (const [index, base] of [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000, 30000].entries()) {
      assert.ok(base * 0.8 <= steps[index] && steps[index] <= base * 1.2, `${steps[index]} for ${base}`)
    }
    backoff.reset()
    assert.ok(backoff.next() <= 600)
  }
})

test("the waits are rutis-bridge's, unless tests shorten them", () => {
  assert.deepEqual(retryFrom({}), { initial: 500, max: 30_000, jitter: 0.2, stable: 60_000, rejected: 30_000, handshake: 10_000 })
  assert.deepEqual(retryFrom({ RUTIS_RETRY: '100,400,1000,300' }), { ...RETRY, initial: 100, max: 400, stable: 1000, rejected: 300 })
})

test('waits read short', () => {
  assert.deepEqual([500, 1040, 30_000, 123].map(seconds), ['0.5s', '1s', '30s', '0.1s'])
})

test('a session ends as an authentication failure, incompatible, or retryable', () => {
  assert.equal(sessionFailure(4003), AUTH_REJECTED)
  const mismatch = (() => { try { greetingOf({ op: 'hello', version: 4, endpoint: 'other' }, { local: 'pixel', expected: 'main' }) } catch (error) { return error } })()
  assert.match(mismatch.message, /^endpoint mismatch/)
  assert.equal(sessionFailure(undefined, mismatch), AUTH_REJECTED)
  const newer = (() => { try { greetingOf({ op: 'hello', version: 9, endpoint: 'main' }, { local: 'pixel' }) } catch (error) { return error } })()
  assert.match(newer.message, /^incompatible session/)
  assert.equal(sessionFailure(undefined, newer), INCOMPATIBLE)
  // Being replaced, and anything else, is retryable.
  assert.equal(sessionFailure(4002), RETRYABLE)
  assert.equal(sessionFailure(1001, new Error('the access point did not greet in time')), RETRYABLE)
  assert.deepEqual(greetingOf({ op: 'hello', version: 4, endpoint: 'main', capabilities: ['x'] }, { local: 'pixel', expected: 'main' }).capabilities, ['x'])
})

test('what retrying cannot fix is refused before dialing', () => {
  assert.throws(() => dialOptions('ws://example.com/rutis', 'pixel', {}), /non-loopback address needs wss/)
  assert.throws(() => dialOptions('http://127.0.0.1/rutis', 'pixel', {}), /not a ws:\/\/ or wss:\/\/ address/)
  assert.throws(() => dialOptions('ws://127.0.0.1/rutis', 'Pixel 7', {}), /invalid endpoint id/)
  const dir = mkdtempSync(join(tmpdir(), 'rutis-dial-'))
  writeFileSync(join(dir, 'empty.pem'), 'no certificate here\n')
  assert.throws(() => dialOptions('wss://example.com/rutis', 'pixel', { RUTIS_CA: join(dir, 'empty.pem') }), /^Error: RUTIS_CA .*empty\.pem/)
  assert.throws(() => dialOptions('wss://example.com/rutis', 'pixel', { RUTIS_CA: join(dir, 'missing.pem') }), /RUTIS_CA/)
  const options = dialOptions('wss://example.com/rutis', 'pixel', { RUTIS_TOKEN: 'secret' })
  assert.equal(options.token, 'secret')
  assert.equal(options.ca, undefined)
})

// A session playing `play(session)`: it greets, refuses, or ends.
function fakeSession(play, log) {
  let greet, close
  const greeted = new Promise((resolve, reject) => { greet = { resolve, reject } })
  greeted.catch(() => {})
  const ended = new Promise(resolve => { close = resolve })
  const session = {
    greeted, ended,
    greet: () => greet.resolve(),
    refuse: error => greet.reject(error),
    lose: (reason, code) => close({ reason, code }),
    close: reason => { log.push(`close: ${reason}`); close({ reason, code: 1001 }) },
    async end() { log.push('end'); return ended },
    async stop() { log.push('stop') },
  }
  setImmediate(() => play(session))
  return session
}

// Run the loop through `script`: an error to fail the dial with, or a
// function that plays the session. It stops once out of script, or once it
// says a line starting with `stopOn`.
async function dialing(script, { stopOn, retry = FAST } = {}) {
  const lines = [], log = []
  let stop
  const stopped = new Promise(resolve => { stop = resolve })
  const steps = script[Symbol.iterator]()
  const say = line => { lines.push(line); if (stopOn && line.startsWith(stopOn)) stop() }
  const dial = async () => {
    const { value, done } = steps.next()
    if (done) { stop(); throw Object.assign(new Error('the end of the script'), { category: RETRYABLE }) }
    if (value instanceof Error) throw value
    return value
  }
  const code = await within('the loop', keepDialing({ address: 'ws://main', dial, start: play => fakeSession(play, log), stopped, retry, say }))
  return { code, lines, log }
}

const failed = (category, message) => Object.assign(new Error(message), { category })

test('failures are waited out by kind until incompatible', async () => {
  const { code, lines } = await dialing([
    failed(RETRYABLE, 'refused'),
    failed(RETRYABLE, 'refused'),
    failed(AUTH_REJECTED, '403'),
    failed(RETRYABLE, 'refused'),
    failed(INCOMPATIBLE, '400'),
  ])
  assert.equal(code, 1)
  assert.deepEqual(lines, [
    'cannot connect (refused); retrying in 0s',
    'cannot connect (refused); retrying in 0s',
    'refused: auth-rejected (403); retrying in 0s',
    'cannot connect (refused); retrying in 0s',
    'refused: incompatible (400); stopping',
  ])
})

test('each session ends before the next dial and says why', async () => {
  const { code, lines, log } = await dialing([
    session => { session.greet(); session.lose('replaced by a new connection', 4002) },
    session => { session.greet(); session.lose('rejected: not known', 4003) },
  ])
  assert.equal(code, 0)
  assert.deepEqual(log, ['end', 'end'])
  assert.equal(lines[0], 'connected to ws://main')
  assert.match(lines[1], /^connection lost \(replaced by a new connection\); retrying in/)
  assert.equal(lines[2], 'connected to ws://main')
  assert.match(lines[3], /^refused: auth-rejected \(rejected: not known\); retrying in/)
})

test('a greeting this side refuses ends the session by its kind', async () => {
  const mismatch = failed(AUTH_REJECTED, 'endpoint mismatch: the far end greeted as other')
  const newer = failed(INCOMPATIBLE, 'incompatible session: version 9')
  const { code, lines, log } = await dialing([session => session.refuse(mismatch), session => session.refuse(newer)])
  assert.equal(code, 1)
  assert.deepEqual(log, [`close: ${mismatch.message}`, 'end', `close: ${newer.message}`, 'end'])
  assert.deepEqual(lines, [
    `refused: auth-rejected (${mismatch.message}); retrying in 0s`,
    `refused: incompatible (${newer.message}); stopping`,
  ])
})

test('an access point that does not greet is given up on', async () => {
  const { lines } = await dialing([() => {}], { retry: { ...FAST, handshake: 10 } })
  assert.match(lines[0], /^connection lost \(the access point did not greet in time\); retrying in/)
})

test('stopping during a session stops it and does not dial again', async () => {
  const { code, lines, log } = await dialing([session => session.greet(), failed(RETRYABLE, 'dialed again')], { stopOn: 'connected' })
  assert.equal(code, 0)
  assert.deepEqual(log, ['stop'])
  assert.deepEqual(lines, ['connected to ws://main'])
})

test('stopping while waiting ends the wait', async () => {
  const { code, lines } = await dialing([failed(RETRYABLE, 'refused')], { stopOn: 'cannot connect', retry: { ...RETRY, initial: 60_000 } })
  assert.equal(code, 0)
  assert.equal(lines.length, 1)
})
