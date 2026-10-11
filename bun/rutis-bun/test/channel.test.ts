import { expect, test } from 'bun:test'
import { createServer, type Socket } from 'node:net'
import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { frame, open, ConnectError, takeToken, TOKEN } from '../src/channel.ts'

// A connected pair of sockets over a Unix socket.
async function pair(): Promise<[Socket, Socket]> {
  const path = join(mkdtempSync(join(tmpdir(), 'rutis-bun-')), 'pair.sock')
  const server = createServer()
  const accepted = new Promise<Socket>(resolve => server.once('connection', resolve))
  await new Promise<void>(resolve => server.listen(path, resolve))
  const { createConnection } = await import('node:net')
  const client = createConnection(path)
  await new Promise(resolve => client.once('connect', resolve))
  const far = await accepted
  server.close()
  return [client, far]
}

function collect() {
  const messages: string[] = []
  let closed!: (reason?: string) => void
  const ended = new Promise<string | undefined>(resolve => { closed = resolve })
  return { messages, ended, handlers: { message: (text: string) => messages.push(text), closed } }
}

test('each line is one message, across reads and within one', async () => {
  const [a, b] = await pair()
  const got = collect()
  frame(b, got.handlers)
  a.write('one\ntw')
  await Bun.sleep(10)
  a.write('o\nthree\nfour\n')
  a.end()
  expect(await got.ended).toBeUndefined()
  expect(got.messages).toEqual(['one', 'two', 'three', 'four'])
})

test('a message longer than the limit ends the channel', async () => {
  const [a, b] = await pair()
  const got = collect()
  frame(b, got.handlers, 16)
  a.write('short\n' + 'x'.repeat(10))
  a.write('y'.repeat(10) + '\nnever\n')
  expect(await got.ended).toContain('longer than 16 bytes')
  expect(got.messages).toEqual(['short'])
  a.destroy()
})

test('invalid UTF-8 ends the channel, even split across two reads', async () => {
  for (const parts of [[Buffer.from('{"a":"\xff"}\n', 'latin1')], [Buffer.from([0x22, 0xe2, 0x82]), Buffer.from([0x22, 0x0a])]]) {
    const [a, b] = await pair()
    const got = collect()
    frame(b, got.handlers)
    a.write('ok\n')
    for (const part of parts) { a.write(part); await Bun.sleep(10) }
    a.write('never\n')
    expect(await got.ended).toBe('received a message that is not valid UTF-8')
    expect(got.messages).toEqual(['ok'])
    a.destroy()
  }
})

test('a leading BOM is kept, not stripped, and a valid split character is one', async () => {
  const [a, b] = await pair()
  const got = collect()
  frame(b, got.handlers)
  a.write(Buffer.from([0xef, 0xbb, 0xbf, 0x7b, 0x7d, 0x0a, 0xe2, 0x82]))
  await Bun.sleep(10)
  a.write(Buffer.from([0xac, 0x0a]))
  a.end()
  expect(await got.ended).toBeUndefined()
  expect(got.messages).toEqual(['\ufeff{}', '\u20ac'])
})

test('a stream that ends inside a message fails', async () => {
  const [a, b] = await pair()
  const got = collect()
  frame(b, got.handlers)
  a.end('partial')
  expect(await got.ended).toBe('stream ended inside a message')
})

test('sending refuses a raw newline and an oversized message', async () => {
  const [a, b] = await pair()
  const channel = frame(a, collect().handlers, 8)
  expect(() => channel.send('a\nb')).toThrow('raw newline')
  expect(() => channel.send('123456789')).toThrow('longer than 8 bytes')
  a.destroy(); b.destroy()
})

test('a runtime does not dial WebSocket endpoints', async () => {
  const error = await open('ws://127.0.0.1:1/rutis', collect().handlers).catch(error => error)
  expect(error).toBeInstanceOf(ConnectError)
  expect(error.category).toBe('incompatible')
  expect(error.message).toContain('listens')
})

test('the loopback token is taken out of the environment', () => {
  process.env[TOKEN] = 'secret'
  expect(takeToken()).toBe('secret')
  expect(process.env[TOKEN]).toBeUndefined()
  expect(takeToken()).toBeUndefined()
})

test('a tcp channel without a token is refused before dialing', async () => {
  const error = await open('tcp:127.0.0.1:1', collect().handlers).catch(error => error)
  expect(error.category).toBe('auth-rejected')
})

test('ending this side half-closes: the far end ends, what was sent arrives', async () => {
  const [a, b] = await pair()
  const near = frame(a, collect().handlers)
  const far = collect()
  frame(b, far.handlers)
  near.send('last')
  near.end()
  expect(await far.ended).toBeUndefined()
  expect(far.messages).toEqual(['last'])
})
