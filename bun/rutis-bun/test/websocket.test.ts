// The WebSocket binding of a remote Bun runtime: listening (Bun.serve) and
// dialing (Bun's `ws` client), and how a failed dial is classified. A
// listening runtime against rutis is crates/rutis-bridge/tests/session_matrix.rs
// (`bun_listening_on_a_websocket`); a dialing one, dial_conformance.rs.
import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { connect } from 'node:net'
import { rootCertificates } from 'node:tls'
import { ConnectError } from '../src/channel.ts'
import { ENDPOINT_PROTOCOL } from '../src/session.ts'
import { listen, open, LIMITS, type Handlers, type Options } from '../src/websocket.ts'
import { within } from './support.ts'

const PROTOCOL = `rutis.${ENDPOINT_PROTOCOL}`
const TLS = new URL('../../../python/rutis/tests/tls/', import.meta.url)
const pem = (name: string) => readFileSync(new URL(name, TLS), 'utf8')

function inbox() {
  const messages: string[] = []
  let wake: (() => void) | undefined
  let ended!: (closed: { reason?: string, code?: number }) => void
  const closed = new Promise<{ reason?: string, code?: number }>(resolve => { ended = resolve })
  const handlers: Handlers = {
    message: text => { messages.push(text); wake?.() },
    closed: (reason, code) => ended({ reason, code }),
  }
  return {
    handlers,
    next: () => within('the next message', messages.length ? Promise.resolve(messages.shift()) : new Promise<string | undefined>(resolve => { wake = () => { wake = undefined; resolve(messages.shift()) } })),
    get closedWith() { return within('the channel to close', closed) },
  }
}

async function pair(spec = 'ws://127.0.0.1:0/rutis', listening: Options = {}, dialing: Options = {}) {
  const server = inbox(), client = inbox()
  let accept!: (channel: any) => void
  const accepted = new Promise<any>(resolve => { accept = resolve })
  const listener = listen(spec, { handlers: () => server.handlers, accepted: channel => accept(channel) }, { protocol: PROTOCOL, token: 'secret', ...listening })
  const dialed = await open(listener.url.replace('://127.0.0.1', dialing.ca ? '://localhost' : '://127.0.0.1'), client.handlers, { protocol: PROTOCOL, token: 'secret', ...dialing })
  return { listener, dialed, accepted: await within('the accepted connection', accepted), server, client }
}

test('messages cross both ways and an orderly close is a normal end', async () => {
  const { listener, dialed, accepted, server, client } = await pair()
  dialed.send('{"a":1}')
  expect(await server.next()).toBe('{"a":1}')
  accepted.send('{"b":"x\\ny"}')
  expect(await client.next()).toBe('{"b":"x\\ny"}')
  accepted.close('bye')
  expect((await client.closedWith).reason).toBeUndefined()
  expect((await server.closedWith).reason).toBe('bye')
  listener.stop()
})

test('a takeover closes with 4002, read as replaced', async () => {
  const { listener, accepted, client } = await pair()
  accepted.replaced()
  expect(await client.closedWith).toEqual({ reason: 'replaced by a new connection', code: 4002 })
  listener.stop()
})

test('an access point rejecting the id closes with 4003, read as rejected, with its code', async () => {
  const server = Bun.serve({
    hostname: '127.0.0.1', port: 0,
    fetch(request, server) { return server.upgrade(request, { headers: { 'Sec-WebSocket-Protocol': PROTOCOL } }) ? undefined : new Response(null, { status: 400 }) },
    websocket: { open(ws) { ws.close(4003, 'pixel is not known') }, message() {} },
  })
  const client = inbox()
  await open(`ws://127.0.0.1:${server.port}/rutis`, client.handlers, { protocol: PROTOCOL })
  expect(await client.closedWith).toEqual({ reason: 'rejected: pixel is not known', code: 4003 })
  server.stop(true)
})

test('authentication, subprotocol and address failures carry categories', async () => {
  const listener = listen('ws://127.0.0.1:0/rutis', { handlers: () => inbox().handlers, accepted: () => {} }, { protocol: PROTOCOL, token: 'secret' })
  const failure = async (url: string, options: Options) => {
    try { await open(url, inbox().handlers, options); return 'opened' }
    catch (error: any) { expect(error).toBeInstanceOf(ConnectError); return `${error.category}: ${error.message}` }
  }
  expect(await failure(listener.url, { protocol: PROTOCOL, token: 'guess' })).toMatch(/^auth-rejected: .*upgrade refused with 403: not accepted here$/)
  expect(await failure(listener.url, { protocol: PROTOCOL })).toMatch(/^auth-rejected: .*upgrade refused with 401/)
  // The refusal says which end to upgrade (standard Q6.2.4).
  expect(await failure(listener.url, { protocol: 'rutis.99', token: 'secret' })).toMatch(/^incompatible: .*400: this endpoint speaks rutis\.4, the dialer offered rutis\.99: upgrade this endpoint \(npm @arcships\/rutis-bun\)$/)
  expect(await failure(listener.url, { protocol: 'rutis.1', token: 'secret' })).toMatch(/upgrade the dialer$/)
  expect(await failure(listener.url.replace('/rutis', '/elsewhere'), { protocol: PROTOCOL, token: 'secret' })).toMatch(/^incompatible: .*404/)
  expect(await failure('ws://example.com/rutis', { protocol: PROTOCOL })).toMatch(/^incompatible: .*non-loopback address needs wss/)
  expect(await failure(listener.url.replace(/:\d+\//, ':1/'), { protocol: PROTOCOL, token: 'secret' })).toStartWith('retryable: ')
  listener.stop()
})

test('a listener without TLS is loopback only, and wss needs a certificate', () => {
  const accepting = { handlers: () => inbox().handlers, accepted: () => {} }
  expect(() => listen('ws://0.0.0.0:0/rutis', accepting, { protocol: PROTOCOL, token: 'secret' })).toThrow('only a loopback listener may go without TLS')
  expect(() => listen('wss://127.0.0.1:0/rutis', accepting, { protocol: PROTOCOL, token: 'secret' })).toThrow('wss needs a certificate and key')
})

test('wss: the CA given is trusted besides the default roots; a certificate refused is an authentication failure', async () => {
  const listening = { cert: pem('localhost.pem'), key: pem('localhost.key') }
  const { listener, dialed, server } = await pair('wss://127.0.0.1:0/rutis', listening, { ca: [...rootCertificates, pem('ca.pem')] })
  dialed.send('{"over":"tls"}')
  expect(await server.next()).toBe('{"over":"tls"}')
  const refused = await open(listener.url.replace('://127.0.0.1', '://localhost'), inbox().handlers, { protocol: PROTOCOL, token: 'secret' }).catch(error => error)
  expect(refused).toBeInstanceOf(ConnectError)
  expect(refused.category).toBe('auth-rejected')
  dialed.end()
  listener.stop()
})

test('heartbeats drop a far end that went silent', async () => {
  const quick = { ...LIMITS, ping: 50, timeout: 500 }
  const server = inbox()
  let accept!: () => void
  const accepted = new Promise<void>(resolve => { accept = resolve })
  const listener = listen('ws://127.0.0.1:0/rutis', { handlers: () => server.handlers, accepted: () => accept() }, { protocol: PROTOCOL, token: 'secret', limits: quick })
  // A far end that upgrades, then never answers a ping.
  const silent = connect(Number(new URL(listener.url).port), '127.0.0.1')
  silent.on('error', () => {})
  silent.write([
    'GET /rutis HTTP/1.1', 'Host: 127.0.0.1', 'Upgrade: websocket', 'Connection: Upgrade',
    'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==', 'Sec-WebSocket-Version: 13',
    `Sec-WebSocket-Protocol: ${PROTOCOL}`, 'Authorization: Bearer secret', '', '',
  ].join('\r\n'))
  await within('the silent connection to be accepted', accepted)
  expect((await server.closedWith).reason).toContain('heartbeat timeout')
  silent.destroy()
  listener.stop()
})
