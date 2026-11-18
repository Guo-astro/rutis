import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createConnection, createServer } from 'node:net'
import { createServer as createHttpServer } from 'node:http'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createInterface } from 'node:readline'
import { encode, decode } from '../src/codec.mjs'
import { open, ConnectError } from '../src/channel/index.mjs'
import { frame, MAX_MESSAGE } from '../src/channel/unix.mjs'
import { LIMITS } from '../src/channel/websocket.mjs'

test('the codec adds no separator and escapes newlines inside strings', () => {
  const text = encode({ op: 'hello', note: 'a\nb', missing: undefined })
  assert.equal(text, '{"op":"hello","note":"a\\nb","missing":null}')
  assert.ok(!text.includes('\n'))
  assert.deepEqual(decode(text), { op: 'hello', note: 'a\nb', missing: null })
  assert.throws(() => encode({ n: Infinity }), TypeError)
})

test('a unix channel frames each message as one line, both ways', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'rutis-channel-'))
  const path = join(directory, 'peer.sock')
  const server = createServer()
  const accepted = new Promise(resolve => server.once('connection', resolve))
  await new Promise(resolve => server.listen(path, resolve))
  try {
    const received = []
    let closed
    const ended = new Promise(resolve => { closed = resolve })
    const channel = await open(`unix:${path}`, { message: text => received.push(text), closed })
    const remote = await accepted
    const lines = createInterface({ input: remote })[Symbol.asyncIterator]()
    channel.send(encode({ op: 'hello', note: 'x\ny' }))
    assert.deepEqual(decode((await lines.next()).value), { op: 'hello', note: 'x\ny' })
    remote.write('{"a":1}\n{"b":2}\n')
    remote.end()
    assert.equal(await ended, undefined)
    assert.deepEqual(received, ['{"a":1}', '{"b":2}'])
  } finally {
    server.close()
    await rm(directory, { recursive: true, force: true })
  }
})

test('failing to connect carries a category, not just text', async () => {
  const missing = join(tmpdir(), `rutis-missing-${process.pid}.sock`)
  await assert.rejects(open(missing, { message() {}, closed() {} }),
    error => error instanceof ConnectError && error.category === 'retryable')
  // An upgrade refused with 404 is incompatible, not retryable. The server
  // is this test's own: example.com answered this way until its network
  // stopped doing so (2026-11-18, #1080).
  const refusing = createHttpServer((_, response) => {
    response.writeHead(404)
    response.end('no channel here\n')
  })
  await new Promise(resolve => refusing.listen(0, '127.0.0.1', resolve))
  try {
    const { port } = refusing.address()
    await assert.rejects(open(`ws://127.0.0.1:${port}/rutis`, { message() {}, closed() {} }),
      error => error instanceof ConnectError && error.category === 'incompatible')
  } finally {
    refusing.close()
  }
})

test('an inherited socket (fd:3) is a channel, both ways', async () => {
  const { spawn } = await import('node:child_process')
  const echo = `
    import { open } from ${JSON.stringify(new URL('../src/channel/index.mjs', import.meta.url).href)}
    const channel = await open('fd:3', {
      message: text => channel.send(text.toUpperCase()),
      closed: () => process.exit(0),
    })
  `
  const child = spawn(process.execPath, ['--input-type=module', '-e', echo], { stdio: ['ignore', 'inherit', 'inherit', 'pipe'] })
  const socket = child.stdio[3]
  const lines = createInterface({ input: socket })[Symbol.asyncIterator]()
  socket.write('{"op":"hello"}\n')
  assert.equal((await lines.next()).value, '{"OP":"HELLO"}')
  socket.end()
  const code = await new Promise(resolve => child.once('exit', resolve))
  assert.equal(code, 0)
})

test('an invalid fd spec is incompatible, not retryable', async () => {
  await assert.rejects(open('fd:x', { message() {}, closed() {} }),
    error => error instanceof ConnectError && error.category === 'incompatible')
})

// Two connected sockets: ours framed as a channel with `options`, theirs raw.
async function framedPair(options) {
  const directory = await mkdtemp(join(tmpdir(), 'rutis-limit-'))
  const path = join(directory, 'peer.sock')
  const server = createServer()
  const accepted = new Promise(resolve => server.once('connection', resolve))
  await new Promise(resolve => server.listen(path, resolve))
  const ours = createConnection(path)
  await new Promise((resolve, reject) => { ours.once('connect', resolve); ours.once('error', reject) })
  const theirs = await accepted
  server.close()
  await rm(directory, { recursive: true, force: true })
  const received = []
  let closed
  const ended = new Promise(resolve => { closed = resolve })
  const channel = frame(ours, { message: text => received.push(text), closed }, options)
  return { channel, theirs, received, ended }
}

// Risk P2 (Q5.3.4, Q6.3.2): the line framing has a size limit, the
// WebSocket binding's by default; over it the channel closes with a reason.
test('a line at the limit arrives; one byte more closes the channel', async () => {
  assert.equal(MAX_MESSAGE, LIMITS.maxMessage)

  const exact = await framedPair()
  exact.theirs.write(Buffer.alloc(MAX_MESSAGE, 'a'))
  exact.theirs.end('\n')
  assert.equal(await exact.ended, undefined)
  assert.equal(exact.received.length, 1)
  assert.equal(exact.received[0].length, MAX_MESSAGE)

  const over = await framedPair()
  over.theirs.on('error', () => {})
  over.theirs.write(Buffer.alloc(MAX_MESSAGE + 1, 'a'))
  over.theirs.end('\n{}\n')
  assert.match(await over.ended, /over the limit/)
  assert.deepEqual(over.received, [])
})

test('bytes without a newline stop at the limit, whatever keeps coming', async () => {
  const { theirs, received, ended } = await framedPair({ maxMessage: 1024 })
  theirs.on('error', () => {})
  let writing = true
  ended.then(() => { writing = false })
  const chunk = Buffer.alloc(256, 'x')
  // Keep writing until the channel gives up: it must, without a newline.
  const write = () => { if (writing && !theirs.destroyed) theirs.write(chunk, write) }
  write()
  assert.match(await ended, /over the limit of 1024 bytes/)
  assert.deepEqual(received, [])
  theirs.destroy()
})

test('sending over the limit closes the channel instead', async () => {
  const { channel, theirs, ended } = await framedPair({ maxMessage: 4 })
  const lines = createInterface({ input: theirs })[Symbol.asyncIterator]()
  channel.send('1234')
  assert.equal((await lines.next()).value, '1234')
  channel.send('12345')
  assert.match(await ended, /exceeds the limit of 4/)
  assert.equal((await lines.next()).done, true)
})

// Q6.2.3: invalid UTF-8 is not a frame. Decoding must not replace it with
// U+FFFD, which inside a JSON string would parse with changed content.
test('a line that is not valid UTF-8 closes the channel', async () => {
  const { theirs, received, ended } = await framedPair()
  theirs.on('error', () => {})
  theirs.write(Buffer.concat([Buffer.from('{"v":"'), Buffer.from([0xff]), Buffer.from('"}\n{}\n')]))
  assert.match(await ended, /not valid UTF-8/)
  assert.deepEqual(received, [])
})

// The line is decoded whole: an invalid sequence split across two reads is
// refused, as one that arrives in a single read is.
test('invalid UTF-8 split across two reads closes the channel', async () => {
  const { theirs, received, ended } = await framedPair()
  theirs.on('error', () => {})
  // A lead byte of a 3-byte sequence, then a byte that cannot continue it.
  theirs.write(Buffer.concat([Buffer.from('{"v":"'), Buffer.from([0xe2])]), () => {
    theirs.write(Buffer.concat([Buffer.from([0x41]), Buffer.from('"}\n')]))
  })
  assert.match(await ended, /not valid UTF-8/)
  assert.deepEqual(received, [])
})

// Only \n separates messages: a \r before it is a byte of the message and
// counts toward the limit, as in Rust and Python.
test('a carriage return is part of the message', async () => {
  const fits = await framedPair({ maxMessage: 4 })
  fits.theirs.end('{}\r\nabc\r\n')
  assert.equal(await fits.ended, undefined)
  assert.deepEqual(fits.received, ['{}\r', 'abc\r'])
  const over = await framedPair({ maxMessage: 4 })
  over.theirs.on('error', () => {})
  over.theirs.end('abcd\r\n')
  assert.match(await over.ended, /over the limit/)
})

// A loopback child's socket comes paused, with what followed its token put
// back (serve.mjs): the framing must start it and read that first.
test('a loopback socket handed over after its token is framed from where the token ended', async () => {
  const { loopback } = await import('../src/serve.mjs')
  const listener = await loopback('secret')
  const port = Number(listener.address.split(':').at(-1))
  const dialed = createConnection({ host: '127.0.0.1', port })
  dialed.write('secret\n{"op":"hello"}\n')
  const socket = await listener.accepted
  const received = []
  let closed
  const ended = new Promise(resolve => { closed = resolve })
  frame(socket, { message: text => { received.push(text); if (received.length === 2) dialed.end() }, closed })
  dialed.write('{"op":"after"}\n')
  assert.equal(await ended, undefined)
  assert.deepEqual(received, ['{"op":"hello"}', '{"op":"after"}'])
})

test('a line split across reads, even inside a character; a truncated last line fails', async () => {
  const { theirs, received, ended } = await framedPair({ maxMessage: 10 })
  const bytes = Buffer.from('{"a":"é"}\n{"b"')
  // Two writes, split between the two bytes of é.
  theirs.write(bytes.subarray(0, 7))
  theirs.end(bytes.subarray(7))
  assert.match(await ended, /inside a message/)
  assert.deepEqual(received, ['{"a":"é"}'])
})
