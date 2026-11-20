// guarantee: bridge/large-and-deep-values
// A frame over the transport's message limit, carried as `chunk` frames and
// reassembled by the far end (#1361, #1363). A chunk is not a call: the method
// runs once, after the last chunk, and never for a transfer that does not
// finish.
import { expect, test } from 'bun:test'
import { createServer } from 'node:net'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Session, ENDPOINT_PROTOCOL, DEFAULT_MAX_TRANSFER } from '../src/session.ts'
import { decode, encode } from '../src/codec.ts'
import { frame } from '../src/channel.ts'
import { within } from './support.ts'

const data = (value: unknown) => ({ type: 'data', value })

function harness(dispatch: (...args: any[]) => unknown, options: Record<string, unknown> = {}, capabilities = ['chunked']) {
  const sent: string[] = []
  let fault: Error | undefined
  const peer = new Session({
    endpoint: { local: 'bun' },
    ...options,
    dispatch,
    send: (line: string) => sent.push(line),
    pump: () => { throw new Error('a synchronous call is not expected here') },
    abort: (error: Error) => { fault = error },
  } as any)
  peer.receive({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'node', capabilities })
  return { peer, sent, fault: () => fault }
}

const chunk = (transfer: string, seq: number, last: boolean, bytes: Uint8Array) => ({
  op: 'chunk', transfer, seq, last, data: Buffer.from(bytes).toString('base64'),
})

const parts = (text: string, size: number) => {
  const bytes = Buffer.from(text)
  const out: Buffer[] = []
  for (let at = 0; at < bytes.length; at += size) out.push(bytes.subarray(at, at + size))
  return out
}

test('a chunked call is dispatched once, after its last chunk', async () => {
  const calls: any[] = []
  const { peer, sent, fault } = harness((_target: string, _method: string, args: any[]) => { calls.push(args); return args[0] })
  const value = { text: 'x'.repeat(4096) }
  const request = { op: 'invoke', id: 'node:1', path: [], target: 'test', method: 'echo', args: data([value]) }
  const pieces = parts(encode(request), 1024)
  expect(pieces.length).toBeGreaterThan(1)
  pieces.slice(0, -1).forEach((part, seq) => peer.receive(chunk('node:1', seq, false, part)))
  expect(calls.length).toBe(0)
  peer.receive(chunk('node:1', pieces.length - 1, true, pieces[pieces.length - 1]))
  await Promise.resolve()
  expect(calls.length).toBe(1)
  expect(calls[0]).toEqual([value])
  const reply = decode(sent[sent.length - 1])
  expect(reply.op).toBe('return')
  expect(reply.value).toEqual(data(value))
  expect(fault()).toBeUndefined()
  peer.close()
})

test('a transfer the session never finishes is never dispatched', async () => {
  let calls = 0
  const { peer } = harness(() => { calls++ })
  const request = { op: 'invoke', id: 'node:1', path: [], target: 'test', method: 'm', args: data([]) }
  const pieces = parts(encode(request), 16)
  expect(pieces.length).toBeGreaterThan(2)
  pieces.slice(0, -1).forEach((part, seq) => peer.receive(chunk('node:1', seq, false, part)))
  peer.close(new Error('the far end disconnected midway'))
  await Promise.resolve()
  expect(calls).toBe(0)
})

test('a chunk out of order ends the session', () => {
  const { peer, fault } = harness(() => undefined)
  peer.receive(chunk('node:1', 1, false, Buffer.from('{')))
  expect(fault()?.message).toMatch(/^chunked transfer node:1: expected chunk 0, got 1/)
})

test('a second transfer before the first ended ends the session', () => {
  const { peer, fault } = harness(() => undefined)
  peer.receive(chunk('node:1', 0, false, Buffer.from('{"op"')))
  peer.receive(chunk('node:2', 0, false, Buffer.from('{')))
  expect(fault()?.message).toMatch(/^chunked transfer node:1: transfer node:2 started before it ended/)
})

test('a chunk before the greeting ends the session', () => {
  let fault: Error | undefined
  const peer = new Session({
    endpoint: { local: 'bun' },
    dispatch: () => undefined,
    send: () => {},
    pump: () => {},
    abort: (error: Error) => { fault = error },
  } as any)
  peer.ready.catch(() => {})
  peer.receive(chunk('node:1', 0, true, Buffer.from('{}')))
  expect(fault?.message).toMatch(/^request before protocol handshake/)
})

test('a chunk from a far end that did not declare chunked ends the session', () => {
  const { peer, fault } = harness(() => undefined, {}, [])
  peer.receive(chunk('node:1', 0, true, Buffer.from('{}')))
  expect(fault()?.message).toMatch(/^chunk frame from a far end that did not declare chunked/)
})

test('a chunk with invalid base64 ends the session', () => {
  const { peer, fault } = harness(() => undefined)
  peer.receive({ op: 'chunk', transfer: 'node:1', seq: 0, last: true, data: 'not base64!' })
  expect(fault()?.message).toMatch(/^chunked transfer node:1: invalid bytes \(11 characters\)/)
})

test('a transfer over the reassembly limit ends the session', () => {
  const { peer, fault } = harness(() => undefined, { maxTransfer: 1024 })
  expect(DEFAULT_MAX_TRANSFER).toBe(256 * 1024 * 1024)
  peer.receive(chunk('node:1', 0, false, Buffer.alloc(1024)))
  peer.receive(chunk('node:1', 1, true, Buffer.from('{')))
  expect(fault()?.message).toMatch(/^chunked transfer node:1: over the limit of 1024 bytes/)
})

test('a frame over the message limit is sent as chunks that fit it', async () => {
  const { peer, sent } = harness(() => undefined, { maxMessage: 4096 })
  const big = 'x'.repeat(8192)
  const promise = peer.invokeAsync('host:svc', 'take', [big])
  await Promise.resolve()
  expect(sent.length).toBeGreaterThan(1)
  const frames = sent.map(decode)
  frames.forEach((item: any, at: number) => {
    expect(item.op).toBe('chunk')
    expect(Buffer.byteLength(sent[at])).toBeLessThanOrEqual(4096)
  })
  const transfer = frames[0].transfer
  frames.forEach((item: any, seq: number) => {
    expect(item.transfer).toBe(transfer)
    expect(item.seq).toBe(seq)
    expect(item.last).toBe(seq === frames.length - 1)
  })
  const request = decode(Buffer.concat(frames.map((item: any) => Buffer.from(item.data, 'base64'))).toString('utf8'))
  expect(request.op).toBe('invoke')
  expect(request.args.value[0]).toEqual(data(big))
  peer.receive({ op: 'return', id: request.id, value: data('ok') })
  expect(await promise).toBe('ok')
  peer.close()
})

test('a result over the message limit is sent as chunks too', async () => {
  const big = 'y'.repeat(8192)
  const { peer, sent } = harness(() => big, { maxMessage: 4096 })
  peer.receive({ op: 'invoke', id: 'node:1', path: [], target: 'test', method: 'big', args: data([]) })
  await Promise.resolve()
  const frames = sent.map(decode)
  expect(frames.length).toBeGreaterThan(1)
  expect(frames.every((item: any) => item.op === 'chunk')).toBe(true)
  const reply = decode(Buffer.concat(frames.map((item: any) => Buffer.from(item.data, 'base64'))).toString('utf8'))
  expect(reply.op).toBe('return')
  expect(reply.value).toEqual(data(big))
  peer.close()
})

test('a far end without chunked still gets the over-limit error', () => {
  const { peer, sent } = harness(() => undefined, { maxMessage: 4096 }, [])
  expect(() => peer.invoke('host:svc', 'take', ['x'.repeat(8192)])).toThrow(/^svc\.take: argument 1 cannot be sent: the message would be \d+ bytes, over the limit of 4096 bytes$/)
  expect(sent.length).toBe(0)
  peer.close()
})

test('a frame over the reassembly limit fails only its call', () => {
  const { peer, sent } = harness(() => undefined, { maxMessage: 4096, maxTransfer: 2048 })
  expect(() => peer.invoke('host:svc', 'take', ['x'.repeat(8192)])).toThrow(/^svc\.take: argument 1 cannot be sent: the message would be \d+ bytes, over the limit of 2048 bytes$/)
  expect(sent.length).toBe(0)
  peer.close()
})

test('RUTIS_MAX_TRANSFER must be a positive number of bytes', () => {
  const previous = process.env.RUTIS_MAX_TRANSFER
  try {
    process.env.RUTIS_MAX_TRANSFER = 'abc'
    expect(() => harness(() => undefined)).toThrow('RUTIS_MAX_TRANSFER "abc": expected a positive number of bytes')
    process.env.RUTIS_MAX_TRANSFER = '1024'
    const { peer } = harness(() => undefined)
    peer.close()
  } finally {
    if (previous === undefined) delete process.env.RUTIS_MAX_TRANSFER
    else process.env.RUTIS_MAX_TRANSFER = previous
  }
})

// A real Node process on the other end of a Unix socket, running the Node
// runtime's own Session: the argument and the result cross whole and each
// call runs once (#1363).
test('a real Node runtime carries values over 16 MiB whole', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-chunked-'))
  const socket = join(dir, 'session.sock')
  const fixture = new URL('./fixtures/node-chunked-session.mjs', import.meta.url).pathname
  let session: Session | undefined
  let channel: ReturnType<typeof frame> | undefined
  let failed: Error | undefined
  let greet!: () => void
  let refused!: (error: Error) => void
  const ready = new Promise<void>((resolve, reject) => { greet = resolve; refused = reject })
  const server = createServer(connection => {
    connection.on('error', () => {})
    channel = frame(connection, {
      message: text => session!.receive(decode(text)),
      closed: reason => { if (reason && !failed) refused(new Error(reason)) },
    })
    session = new Session({
      endpoint: { local: 'main', expected: 'node' },
      dispatch: () => { throw new Error('the host serves nothing') },
      send: line => channel!.send(line),
      pump: () => { throw new Error('a synchronous call is not expected here') },
      abort: (error: Error) => { failed = error; refused(error) },
    } as any)
    session.start()
    session.ready.then(greet, refused)
  })
  await new Promise<void>(resolve => server.listen(socket, resolve))
  const child = Bun.spawn(['node', fixture, `unix:${socket}`], { stdin: 'ignore', stdout: 'ignore', stderr: 'pipe' })
  try {
    await within('the Node runtime to greet', ready)
    expect(session!.greeting.endpoint).toBe('node')
    expect(session!.supports('chunked')).toBe(true)
    const big = 'x'.repeat(17 * 1024 * 1024)
    const first = await within('a 17 MiB argument', session!.invokeAsync('conformance', 'tally', [big]))
    expect(first).toEqual({ calls: 1, size: big.length })
    const second = await within('a second 17 MiB argument', session!.invokeAsync('conformance', 'tally', [big]))
    expect(second).toEqual({ calls: 2, size: big.length })
    const back = await within('a 17 MiB result', session!.invokeAsync('conformance', 'big', [big.length]))
    expect(back.length).toBe(big.length)
    expect(await within('a call after them', session!.invokeAsync('conformance', 'echo', ['after']))).toBe('after')
    expect(failed).toBeUndefined()
  } finally {
    try {
      try { channel?.end() } catch {}
      session?.close()
      const code = await within('the Node runtime to stop', child.exited)
      if (code !== 0) {
        const stderr = await new Response(child.stderr).text()
        throw new Error(`the Node runtime exited (${code}): ${stderr}`)
      }
    } finally {
      child.kill()
      server.close()
      rmSync(dir, { recursive: true, force: true })
    }
  }
}, 60_000)
