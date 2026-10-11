// The local channels a runtime process starts on: one message per line.
//
//   `fd:<n>`             a socket inherited from the process that started this one
//   `unix:<path>`        a Unix socket to dial (also a bare path)
//   `tcp:<host>:<port>`  a loopback address to dial, presenting a one-time token
//                        first (how processes start on Windows; the starting
//                        process gives it in RUTIS_CHANNEL_TOKEN, see `takeToken`)
//
// open(spec, { message(text), closed(reason) }, token) → { send(text), end(), close(reason) }
import { connect, createConnection, type Socket } from 'node:net'

// Why a channel could not be established. The category decides what the
// starting side does next; the message is for diagnostics only.
export class ConnectError extends Error {
  category: 'retryable' | 'auth-rejected' | 'incompatible'
  constructor(category: ConnectError['category'], reason: string) {
    super(reason)
    this.name = 'ConnectError'
    this.category = category
  }
}

export const TOKEN = 'RUTIS_CHANNEL_TOKEN'
// The longest message either side may send, as on WebSocket channels; a
// longer one ends the channel rather than growing a buffer without bound.
export const MAX_MESSAGE = 16 * 1024 * 1024
// How long a channel that ended its side waits for the far end's end.
const END_GRACE = 1000
// Strict: invalid UTF-8 is not a frame, and decoding must not replace it
// quietly; a leading BOM is kept, so the frame fails as JSON, as elsewhere.
const UTF8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })

export interface Handlers {
  message(text: string): void
  closed(reason?: string): void
}
export interface Channel {
  send(text: string): void
  end(): void
  close(reason?: string): void
}

// A newline-framed channel on a connected stream: each message sent gets a
// trailing newline, each line received is one message. `closed(reason)` runs
// once, when the stream ends (no reason) or fails.
export function frame(stream: Socket, { message, closed }: Handlers, limit = MAX_MESSAGE): Channel {
  let failure: string | undefined
  let chunks: Buffer[] = []
  let length = 0
  stream.on('error', error => { failure ??= error.message })
  stream.on('data', (data: Buffer) => {
    let start = 0
    for (let at = data.indexOf(10); at !== -1; at = data.indexOf(10, start)) {
      const piece = data.subarray(start, at)
      if (length + piece.length > limit) return tooLong()
      const line = chunks.length ? Buffer.concat([...chunks, piece], length + piece.length) : piece
      chunks = []; length = 0
      start = at + 1
      let text: string
      try { text = UTF8.decode(line) } catch { return fail('received a message that is not valid UTF-8') }
      message(text)
      if (stream.destroyed) return
    }
    if (start < data.length) {
      const rest = data.subarray(start)
      if (length + rest.length > limit) return tooLong()
      chunks.push(Buffer.from(rest)); length += rest.length
    }
  })
  stream.once('end', () => { if (length) failure ??= 'stream ended inside a message' })
  stream.once('close', () => closed(failure))
  function tooLong() { fail(`a message is longer than ${limit} bytes`) }
  function fail(reason: string) {
    failure ??= reason
    chunks = []; length = 0
    stream.destroy()
  }
  return {
    send(text) {
      if (text.includes('\n')) throw new Error('a message contains a raw newline')
      if (Buffer.byteLength(text) > limit) throw new Error(`a message is longer than ${limit} bytes`)
      stream.write(text + '\n')
    },
    // Half-close, then wait for the far end's end, but not for ever: a
    // socket's end may not reach this side after it half-closed.
    end() { stream.end(); setTimeout(() => stream.destroy(), END_GRACE).unref() },
    close(reason) { failure ??= reason; stream.destroy() },
  }
}

function connected(stream: Socket): Promise<void> {
  return new Promise((resolve, reject) => {
    stream.once('connect', resolve)
    stream.once('error', reject)
  })
}

// The token of a loopback channel, taken out of the environment: spent once
// read, what this process starts must not inherit it.
export function takeToken(): string | undefined {
  const token = process.env[TOKEN]
  delete process.env[TOKEN]
  return token || undefined
}

export async function open(spec: string, handlers: Handlers, token?: string): Promise<Channel> {
  const scheme = /^([a-z][a-z0-9+.-]*):/.exec(spec)?.[1]
  if (scheme === 'fd') {
    const fd = Number(spec.slice('fd:'.length))
    if (!Number.isSafeInteger(fd) || fd < 0) throw new ConnectError('incompatible', `invalid channel ${spec}`)
    // `new net.Socket({ fd })` cannot read an existing descriptor in Bun:
    // `connect({ fd })` can.
    let stream: Socket
    try { stream = connect({ fd } as any) } catch (error: any) { throw new ConnectError('incompatible', `${spec}: ${error.message}`) }
    return frame(stream, handlers)
  }
  if (scheme === 'tcp') {
    const address = spec.slice('tcp:'.length)
    const at = address.lastIndexOf(':')
    const host = address.slice(0, at).replace(/^\[(.*)\]$/, '$1')
    const port = Number(address.slice(at + 1))
    if (at < 0 || !Number.isInteger(port)) throw new ConnectError('incompatible', `not a tcp address: ${spec}`)
    if (!token) throw new ConnectError('auth-rejected', `${TOKEN} is not set for ${spec}`)
    const stream = createConnection({ host, port })
    try { await connected(stream) } catch (error: any) {
      stream.destroy()
      throw new ConnectError('retryable', `${address}: ${error.message}`)
    }
    stream.setNoDelay(true)
    stream.write(token + '\n')
    return frame(stream, handlers)
  }
  if (scheme === undefined || scheme === 'unix') {
    const path = scheme === 'unix' ? spec.slice('unix:'.length) : spec
    const stream = createConnection(path)
    try { await connected(stream) } catch (error: any) {
      stream.destroy()
      throw new ConnectError(error.code === 'EACCES' ? 'auth-rejected' : 'retryable', `${path}: ${error.message}`)
    }
    return frame(stream, handlers)
  }
  if (scheme === 'ws' || scheme === 'wss') {
    // A remote runtime is reached by its controller, which dials and
    // reconnects; a runtime never dials one (remote plugins design §4.4).
    throw new ConnectError('incompatible', `${spec}: the Bun runtime does not dial; it listens (listen:ws://…)`)
  }
  throw new ConnectError('incompatible', `no channel for ${scheme}: addresses`)
}
