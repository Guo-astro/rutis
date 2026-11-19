// guarantee: bridge/large-and-deep-values
// A frame over the transport's message limit, carried as `chunk` frames and
// reassembled by the far end (#1361). A chunk is not a call: the method runs
// once, after the last chunk, and never for a transfer that does not finish.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Session, ENDPOINT_PROTOCOL, DEFAULT_MAX_TRANSFER } from '../src/session.mjs'
import { decode, encode } from '../src/codec.mjs'

const data = value => ({ type: 'data', value })

function harness(dispatch, options = {}, capabilities = ['chunked']) {
  const sent = []
  let fault
  const peer = new Session({
    endpoint: { local: 'node' },
    ...options,
    dispatch,
    send: line => sent.push(line),
    pump: () => { throw new Error('a synchronous call is not expected here') },
    abort: error => { fault = error },
  })
  peer.receive({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'rust', capabilities })
  return { peer, sent, fault: () => fault }
}

const chunk = (transfer, seq, last, bytes) => ({
  op: 'chunk', transfer, seq, last, data: Buffer.from(bytes).toString('base64'),
})

const parts = (text, size) => {
  const bytes = Buffer.from(text)
  const out = []
  for (let at = 0; at < bytes.length; at += size) out.push(bytes.subarray(at, at + size))
  return out
}

test('a chunked call is dispatched once, after its last chunk', async () => {
  const calls = []
  const { peer, sent, fault } = harness((target, method, args) => { calls.push(args); return args[0] })
  const value = { text: 'x'.repeat(4096) }
  const frame = { op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'echo', args: data([value]) }
  const chunks = parts(encode(frame), 1024)
  assert.ok(chunks.length > 1, 'one frame, many chunks')
  chunks.slice(0, -1).forEach((part, seq) => peer.receive(chunk('rust:1', seq, false, part)))
  assert.equal(calls.length, 0, 'dispatched before the last chunk')
  peer.receive(chunk('rust:1', chunks.length - 1, true, chunks.at(-1)))
  await Promise.resolve()
  assert.equal(calls.length, 1, 'the call ran once')
  assert.deepEqual(calls[0], [value], 'the call ran with the whole value')
  const reply = decode(sent.at(-1))
  assert.equal(reply.op, 'return')
  assert.deepEqual(reply.value, data(value))
  assert.equal(fault(), undefined)
  peer.close()
})

test('a transfer the session never finishes is never dispatched', async () => {
  let calls = 0
  const { peer } = harness(() => { calls++ })
  const frame = { op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'm', args: data([]) }
  const chunks = parts(encode(frame), 16)
  assert.ok(chunks.length > 2)
  chunks.slice(0, -1).forEach((part, seq) => peer.receive(chunk('rust:1', seq, false, part)))
  peer.close(new Error('the far end disconnected midway'))
  await Promise.resolve()
  assert.equal(calls, 0, 'a call was dispatched from an incomplete transfer')
})

test('a chunk out of order ends the session', () => {
  const { peer, fault } = harness(() => undefined)
  peer.receive(chunk('rust:1', 1, false, Buffer.from('{')))
  assert.match(fault().message, /^chunked transfer rust:1: expected chunk 0, got 1/)
})

test('a second transfer before the first ended ends the session', () => {
  const { peer, fault } = harness(() => undefined)
  peer.receive(chunk('rust:1', 0, false, Buffer.from('{"op"')))
  peer.receive(chunk('rust:2', 0, false, Buffer.from('{')))
  assert.match(fault().message, /^chunked transfer rust:1: transfer rust:2 started before it ended/)
})

test('a chunk before the greeting ends the session', () => {
  const peer = new Session({
    endpoint: { local: 'node' },
    dispatch: () => undefined,
    send: () => {},
    pump: () => {},
    abort: () => {},
  })
  peer.ready.catch(() => {})
  peer.receive(chunk('rust:1', 0, true, Buffer.from('{}')))
  assert.match(peer.fault.message, /^request before protocol handshake/)
})

test('a chunk from a far end that did not declare chunked ends the session', () => {
  const { peer, fault } = harness(() => undefined, {}, [])
  peer.receive(chunk('rust:1', 0, true, Buffer.from('{}')))
  assert.match(fault().message, /^chunk frame from a far end that did not declare chunked/)
})

test('a chunk with invalid base64 ends the session', () => {
  const { peer, fault } = harness(() => undefined)
  peer.receive({ op: 'chunk', transfer: 'rust:1', seq: 0, last: true, data: 'not base64!' })
  assert.match(fault().message, /^chunked transfer rust:1: invalid bytes \(11 characters\)/)
})

test('a transfer over the reassembly limit ends the session', () => {
  const { peer, fault } = harness(() => undefined, { maxTransfer: 1024 })
  assert.equal(DEFAULT_MAX_TRANSFER, 256 * 1024 * 1024)
  const bytes = Buffer.alloc(1024)
  peer.receive(chunk('rust:1', 0, false, bytes))
  peer.receive(chunk('rust:1', 1, true, Buffer.from('{')))
  assert.match(fault().message, /^chunked transfer rust:1: over the limit of 1024 bytes/)
})

test('a frame over the message limit is sent as chunks that fit it', async () => {
  const { peer, sent } = harness(() => undefined, { maxMessage: 4096 })
  const big = 'x'.repeat(8192)
  const promise = peer.invokeAsync('host:svc', 'take', [big])
  await Promise.resolve()
  assert.ok(sent.length > 1, 'one message for a frame over the limit')
  const frames = sent.map(decode)
  frames.forEach((frame, at) => {
    assert.equal(frame.op, 'chunk', `message ${at} is not a chunk: ${sent[at].slice(0, 80)}`)
    assert.ok(Buffer.byteLength(sent[at]) <= 4096, `chunk ${at} of ${Buffer.byteLength(sent[at])} bytes, over the limit`)
  })
  const transfer = frames[0].transfer
  frames.forEach((frame, seq) => {
    assert.equal(frame.transfer, transfer, `chunk ${seq} of another transfer`)
    assert.equal(frame.seq, seq)
    assert.equal(frame.last, seq === frames.length - 1)
  })
  const call = decode(Buffer.concat(frames.map(frame => Buffer.from(frame.data, 'base64'))).toString('utf8'))
  assert.equal(call.op, 'invoke')
  assert.deepEqual(call.args.value[0], data(big))
  peer.receive({ op: 'return', id: call.id, value: data('ok') })
  assert.equal(await promise, 'ok')
  peer.close()
})

test('a result over the message limit is sent as chunks too', async () => {
  const big = 'y'.repeat(8192)
  const { peer, sent } = harness(() => big, { maxMessage: 4096 })
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'big', args: data([]) })
  await Promise.resolve()
  const frames = sent.map(decode)
  assert.ok(frames.length > 1)
  assert.ok(frames.every(frame => frame.op === 'chunk'))
  const reply = decode(Buffer.concat(frames.map(frame => Buffer.from(frame.data, 'base64'))).toString('utf8'))
  assert.equal(reply.op, 'return')
  assert.deepEqual(reply.value, data(big))
  peer.close()
})

test('a far end without chunked still gets the over-limit error', () => {
  const { peer, sent } = harness(() => undefined, { maxMessage: 4096 }, [])
  assert.throws(() => peer.invoke('host:svc', 'take', ['x'.repeat(8192)]), {
    name: 'RangeError',
    message: /^svc\.take: argument 1 cannot be sent: the message would be \d+ bytes, over the limit of 4096 bytes$/,
  })
  assert.equal(sent.length, 0, 'nothing was sent')
  peer.close()
})

test('a frame over the reassembly limit fails only its call', () => {
  const { peer, sent } = harness(() => undefined, { maxMessage: 4096, maxTransfer: 2048 })
  assert.throws(() => peer.invoke('host:svc', 'take', ['x'.repeat(8192)]), {
    name: 'RangeError',
    message: /^svc\.take: argument 1 cannot be sent: the message would be \d+ bytes, over the limit of 2048 bytes$/,
  })
  assert.equal(sent.length, 0, 'nothing was sent')
  peer.close()
})
