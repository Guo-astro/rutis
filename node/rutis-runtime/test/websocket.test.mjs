import { test } from 'node:test'
import assert from 'node:assert/strict'
import { open, listen } from '../src/channel/websocket.mjs'
import { ConnectError } from '../src/channel/errors.mjs'
import { HANG_GUARD_MS, within, eventually } from './support.mjs'

const PROTOCOL = 'rutis.2'
// Ping and timeout ten times apart, so that a stalled machine does not end a
// connection the test expects to live (docs/design-test-clocks-2026-10-11.md
// §3.3); the handshake bound only guards against a hang.
const quick = { maxMessage: 1024, ping: 100, timeout: 1000, handshake: HANG_GUARD_MS }

function inbox() {
  const messages = []
  let wake, ended
  const closed = new Promise(resolve => { ended = resolve })
  return {
    handlers: { message: text => { messages.push(text); wake?.() }, closed: reason => ended(reason) },
    next: () => within('the next message', messages.length ? Promise.resolve(messages.shift()) : new Promise(resolve => { wake = () => { wake = undefined; resolve(messages.shift()) } })),
    get closedWith() { return within('the channel to close', closed) },
  }
}

async function pair(limits) {
  const server = inbox(), client = inbox()
  let accept
  const accepted = new Promise(resolve => { accept = resolve })
  const listener = await listen('ws://127.0.0.1:0/rutis', { handlers: () => server.handlers, accepted: channel => accept(channel) }, { protocol: PROTOCOL, token: 'secret', limits })
  const dialed = await open(listener.url, client.handlers, { protocol: PROTOCOL, token: 'secret', limits })
  return { listener, dialed, accepted: await within('the accepted connection', accepted), server, client }
}

test('messages cross both ways and an orderly close is a normal end', async () => {
  const { listener, dialed, accepted, server, client } = await pair()
  dialed.send('{"a":1}')
  assert.equal(await server.next(), '{"a":1}')
  accepted.send('{"b":"x\\ny"}')
  assert.equal(await client.next(), '{"b":"x\\ny"}')
  accepted.close('bye')
  assert.equal(await client.closedWith, undefined)
  listener.close()
})

test('a takeover closes with 4002, read as replaced', async () => {
  const { listener, accepted, client } = await pair()
  accepted.replaced()
  assert.equal(await client.closedWith, 'replaced by a new connection')
  listener.close()
})

test('authentication, subprotocol and address failures carry categories', async () => {
  const listener = await listen('ws://127.0.0.1:0/rutis', { handlers: () => inbox().handlers, accepted: () => {} }, { protocol: PROTOCOL, token: 'secret' })
  const category = async options => {
    try { await open(listener.url, inbox().handlers, options); return 'opened' }
    catch (error) { assert.ok(error instanceof ConnectError, error); return error.category }
  }
  assert.equal(await category({ protocol: PROTOCOL, token: 'guess' }), 'auth-rejected')
  assert.equal(await category({ protocol: PROTOCOL }), 'auth-rejected')
  assert.equal(await category({ protocol: 'rutis.99', token: 'secret' }), 'incompatible')
  // The refusal says which end to upgrade (standard Q6.2.4).
  // guarantee: bridge/version-refusal
  const refusal = async protocol => {
    try { await open(listener.url, inbox().handlers, { protocol, token: 'secret' }); return 'opened' }
    catch (error) { return error.message }
  }
  assert.match(await refusal('rutis.99'), /: this endpoint speaks rutis\.2, the dialer offered rutis\.99: upgrade this endpoint \(npm @arcships\/rutis-runtime\)$/)
  assert.match(await refusal('rutis.1'), /: this endpoint speaks rutis\.2, the dialer offered rutis\.1: upgrade the dialer$/)
  await assert.rejects(open('ws://example.com/rutis', inbox().handlers, { protocol: PROTOCOL }), error => error.category === 'incompatible')
  const free = listener.url.replace(/:\d+\//, ':1/')
  await assert.rejects(open(free, inbox().handlers, { protocol: PROTOCOL, token: 'secret' }), error => error.category === 'retryable')
  listener.close()
})

test('a message over the limit closes the channel', async () => {
  const { listener, dialed, client, server } = await pair(quick)
  dialed.send('x'.repeat(2048))
  assert.match(await client.closedWith, /1009/)
  await server.closedWith
  listener.close()
})

// A TCP relay that can stop carrying bytes without closing: a half-open
// connection, as a vanished network leaves it. It counts what it carried
// (each read it passed on).
async function freezableRelay(target) {
  const { createServer, connect } = await import('node:net')
  const state = { frozen: false, carried: 0 }
  const sockets = []
  const relay = createServer(client => {
    const upstream = connect(Number(target.port), target.hostname)
    sockets.push(client, upstream)
    for (const [from, to] of [[client, upstream], [upstream, client]]) {
      from.on('data', data => { if (!state.frozen) { to.write(data); state.carried++ } })
      from.on('error', () => {})
    }
  })
  await new Promise(resolve => relay.listen(0, '127.0.0.1', resolve))
  // Closing drops the connections too: a failed test does not stay alive.
  return { port: relay.address().port, state, close: () => { relay.close(); for (const socket of sockets) socket.destroy() } }
}

// Timing is what this tests, across a socket: by tolerance
// (docs/design-test-clocks-2026-10-11.md §3.3), ping and timeout ten times
// apart, "still connected" after a count of pings rather than a time, no
// bound on how fast the timeout is found (only a hang guard).
test('heartbeats find a half-open connection on both sides', async () => {
  const server = inbox(), client = inbox()
  let accept
  const accepted = new Promise(resolve => { accept = resolve })
  const listener = await listen('ws://127.0.0.1:0/rutis', { handlers: () => server.handlers, accepted: channel => accept(channel) }, { protocol: PROTOCOL, token: 'secret', limits: quick })
  const relay = await freezableRelay(new URL(listener.url))
  try {
    const dialed = await open(`ws://127.0.0.1:${relay.port}/rutis`, client.handlers, { protocol: PROTOCOL, token: 'secret', limits: quick })
    await within('the accepted connection', accepted)
    // Alive while pings flow: through more pings and pongs than fit in one
    // timeout (at most 40: each side pings, and is answered, every 100 ms).
    const before = relay.state.carried
    await eventually('pings through the relay', () => relay.state.carried >= before + 60, () => `${relay.state.carried - before} carried`)
    dialed.send('{"still":"here"}')
    assert.equal(await server.next(), '{"still":"here"}')
    relay.state.frozen = true
    assert.match(await client.closedWith, /heartbeat timeout/)
    assert.match(await server.closedWith, /heartbeat timeout/)
  } finally {
    relay.close()
    listener.close()
  }
})
