// Dialing out (dial-out design §六): how failures are classified, the
// backoff, and the loop that dials again, against stand-ins for the dial
// and the session; what is checked before dialing. The rules against a
// rutis access point are crates/rutis-host/tests/dial_conformance.rs.
import { expect, test } from 'bun:test'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { AUTH_REJECTED, Backoff, INCOMPATIBLE, keepDialing, RETRY, retryFrom, RETRYABLE, seconds, sessionFailure, type Retry, type Session } from '../src/dialing.ts'
import { dialOptions } from '../src/remote.ts'
import { ENDPOINT_PROTOCOL, greetingOf } from '../src/session.ts'
import { within } from './support.ts'

// Short enough that the loop runs at once; the shape is what is checked.
const FAST: Retry = { ...RETRY, initial: 1, max: 4, rejected: 2, handshake: 1000 }

const refusalOf = (frame: object, endpoint: object) => {
  try { greetingOf(frame, endpoint) } catch (error: any) { return error }
  throw new Error('accepted')
}

test('the backoff doubles to its cap within jitter, and resets', () => {
  for (const random of [() => 0, () => 0.999, Math.random]) {
    const backoff = new Backoff(RETRY, random)
    const steps = Array.from({ length: 10 }, () => backoff.next())
    for (const [index, base] of [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000, 30000].entries()) {
      expect(steps[index]).toBeGreaterThanOrEqual(base * 0.8)
      expect(steps[index]).toBeLessThanOrEqual(base * 1.2)
    }
    backoff.reset()
    expect(backoff.next()).toBeLessThanOrEqual(600)
  }
})

test("the waits are rutis-bridge's, unless tests shorten them", () => {
  expect(retryFrom({})).toEqual({ initial: 500, max: 30_000, jitter: 0.2, stable: 60_000, rejected: 30_000, handshake: 10_000 })
  expect(retryFrom({ RUTIS_RETRY: '100,400,1000,300' })).toEqual({ ...RETRY, initial: 100, max: 400, stable: 1000, rejected: 300 })
})

test('waits read short', () => {
  expect([500, 1040, 30_000, 123].map(seconds)).toEqual(['0.5s', '1s', '30s', '0.1s'])
})

test('a session ends as an authentication failure, incompatible, or retryable', () => {
  expect(sessionFailure(4003)).toBe(AUTH_REJECTED)
  const mismatch = refusalOf({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'other' }, { local: 'pixel', expected: 'main' })
  expect(mismatch.message).toStartWith('endpoint mismatch')
  expect(sessionFailure(undefined, mismatch)).toBe(AUTH_REJECTED)
  const newer = refusalOf({ op: 'hello', version: 9, endpoint: 'main' }, { local: 'pixel' })
  expect(newer.message).toStartWith('incompatible session')
  expect(sessionFailure(undefined, newer)).toBe(INCOMPATIBLE)
  // Being replaced, and anything else, is retryable.
  expect(sessionFailure(4002)).toBe(RETRYABLE)
  expect(sessionFailure(1001, new Error('the access point did not greet in time'))).toBe(RETRYABLE)
  expect(greetingOf({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'main', capabilities: ['x'] }, { local: 'pixel', expected: 'main' }).capabilities).toEqual(['x'])
})

test('what retrying cannot fix is refused before dialing', () => {
  expect(() => dialOptions('ws://example.com/rutis', 'pixel', {})).toThrow('non-loopback address needs wss')
  expect(() => dialOptions('http://127.0.0.1/rutis', 'pixel', {})).toThrow('not a ws:// or wss:// address')
  expect(() => dialOptions('ws://127.0.0.1/rutis', 'Pixel 7', {})).toThrow('invalid endpoint id')
  const dir = mkdtempSync(join(tmpdir(), 'rutis-dial-'))
  writeFileSync(join(dir, 'empty.pem'), 'no certificate here\n')
  expect(() => dialOptions('wss://example.com/rutis', 'pixel', { RUTIS_CA: join(dir, 'empty.pem') })).toThrow(/^RUTIS_CA .*empty\.pem/)
  expect(() => dialOptions('wss://example.com/rutis', 'pixel', { RUTIS_CA: join(dir, 'missing.pem') })).toThrow('RUTIS_CA')
  const options = dialOptions('wss://example.com/rutis', 'pixel', { RUTIS_TOKEN: 'secret' })
  expect(options.token).toBe('secret')
  expect(options.ca).toBeUndefined()
})

type Play = (session: Fake) => void
type Fake = Session & { greet(): void, refuse(error: Error): void, lose(reason: string, code: number): void }

// A session playing `play(session)`: it greets, refuses, or ends.
function fakeSession(play: Play, log: string[]): Fake {
  let greet!: { resolve(): void, reject(error: Error): void }
  let close!: (ended: { reason?: string, code?: number }) => void
  const greeted = new Promise<void>((resolve, reject) => { greet = { resolve, reject } })
  greeted.catch(() => {})
  const ended = new Promise<{ reason?: string, code?: number }>(resolve => { close = resolve })
  const session: Fake = {
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
async function dialing(script: Array<Error | Play>, { stopOn, retry = FAST }: { stopOn?: string, retry?: Retry } = {}) {
  const lines: string[] = [], log: string[] = []
  let stop!: () => void
  const stopped = new Promise<void>(resolve => { stop = resolve })
  const steps = script[Symbol.iterator]()
  const say = (line: string) => { lines.push(line); if (stopOn && line.startsWith(stopOn)) stop() }
  const dial = async () => {
    const { value, done } = steps.next()
    if (done) { stop(); throw failed(RETRYABLE, 'the end of the script') }
    if (value instanceof Error) throw value
    return value as Play
  }
  const code = await within('the loop', keepDialing({ address: 'ws://main', dial, start: play => fakeSession(play, log), stopped, retry, say }))
  return { code, lines, log }
}

const failed = (category: string, message: string) => Object.assign(new Error(message), { category })

test('failures are waited out by kind until incompatible', async () => {
  const { code, lines } = await dialing([
    failed(RETRYABLE, 'refused'),
    failed(RETRYABLE, 'refused'),
    failed(AUTH_REJECTED, '403'),
    failed(RETRYABLE, 'refused'),
    failed(INCOMPATIBLE, '400'),
  ])
  expect(code).toBe(1)
  expect(lines).toEqual([
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
  expect(code).toBe(0)
  expect(log).toEqual(['end', 'end'])
  expect(lines[0]).toBe('connected to ws://main')
  expect(lines[1]).toStartWith('connection lost (replaced by a new connection); retrying in')
  expect(lines[2]).toBe('connected to ws://main')
  expect(lines[3]).toStartWith('refused: auth-rejected (rejected: not known); retrying in')
})

test('a greeting this side refuses ends the session by its kind', async () => {
  const mismatch = failed(AUTH_REJECTED, 'endpoint mismatch: the far end greeted as other')
  const newer = failed(INCOMPATIBLE, 'incompatible session: version 9')
  const { code, lines, log } = await dialing([session => session.refuse(mismatch), session => session.refuse(newer)])
  expect(code).toBe(1)
  expect(log).toEqual([`close: ${mismatch.message}`, 'end', `close: ${newer.message}`, 'end'])
  expect(lines).toEqual([
    `refused: auth-rejected (${mismatch.message}); retrying in 0s`,
    `refused: incompatible (${newer.message}); stopping`,
  ])
})

test('an access point that does not greet is given up on', async () => {
  const { lines } = await dialing([() => {}], { retry: { ...FAST, handshake: 10 } })
  expect(lines[0]).toStartWith('connection lost (the access point did not greet in time); retrying in')
})

test('stopping during a session stops it and does not dial again', async () => {
  const { code, lines, log } = await dialing([session => session.greet(), failed(RETRYABLE, 'dialed again')], { stopOn: 'connected' })
  expect(code).toBe(0)
  expect(log).toEqual(['stop'])
  expect(lines).toEqual(['connected to ws://main'])
})

test('stopping while waiting ends the wait', async () => {
  const { code, lines } = await dialing([failed(RETRYABLE, 'refused')], { stopOn: 'cannot connect', retry: { ...RETRY, initial: 60_000 } })
  expect(code).toBe(0)
  expect(lines.length).toBe(1)
})
