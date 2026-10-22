// The WebSocket binding of a remote Bun runtime: it listens (Bun.serve) and
// the controlling rutis dials; or it dials a rutis access point (Bun's
// built-in `ws` client). One connection carries one channel of UTF-8 JSON
// text messages; the session protocol is the subprotocol; bearer tokens
// travel in the Authorization header, never in the URL; both sides ping
// every 10 s and drop a far end silent for 30 s; 16 MiB per message, 1009
// over it. The same binding as the Node runtime's (its
// `channel/websocket.mjs`), in this runtime's own code.
import { readFileSync } from 'node:fs'
import { timingSafeEqual } from 'node:crypto'
import { isIP } from 'node:net'
// Bun's own implementation of the `ws` client: unlike its WebSocket, it
// reports the status of a refused upgrade, which says what the refusal
// means.
import { WebSocket } from 'ws'
import { ConnectError } from './channel.ts'
import { MANIFEST } from './session.ts'

export const LIMITS = { maxMessage: 16 * 1024 * 1024, ping: 10_000, timeout: 30_000, handshake: 10_000 }
export type Limits = typeof LIMITS
export const GOING_AWAY = 1001, TOO_BIG = 1009, TLS_FAILED = 1015, REPLACED = 4002, REJECTED = 4003

// A channel over an open WebSocket. `closed(reason, code)` gets the close
// code received from the far end (1006 when none came).
export interface Handlers {
  message(text: string): void
  closed(reason: string | undefined, code?: number): void
}
export interface WebSocketChannel {
  send(text: string): void
  end(): void
  close(reason?: string): void
  replaced(): void
}

const truncate = (reason = '') => {
  const bytes = Buffer.from(reason)
  if (bytes.length <= 123) return reason
  let end = 123
  while (end > 0 && (bytes[end] & 0xc0) === 0x80) end--
  return bytes.subarray(0, end).toString()
}

const loopback = (host: string) => {
  const bare = host.replace(/^\[|\]$/g, '')
  return bare === 'localhost' || (isIP(bare) === 4 && bare.startsWith('127.')) || bare === '::1'
}

// Why a dialer that offered none of this endpoint's session protocol is
// refused, saying which end to upgrade (standard Q6.2.4) when the versions
// it offered (`rutis.<n>`) are all newer (this endpoint) or all older (the
// dialer) than this endpoint's. The wording is rutis-bridge's
// (`transport::websocket::listen::protocol_refusal`).
export function protocolRefusal(ours: string, offered: string[]): string {
  const version = (protocol: string) => /^rutis\.(\d+)$/.exec(protocol)?.[1]
  const theirs = offered.filter(version)
  const reason = `this endpoint speaks ${ours}`
  if (version(ours) === undefined || theirs.length === 0) return reason
  const n = Number(version(ours)), listed = `${reason}, the dialer offered ${theirs.join(', ')}`
  if (theirs.every(p => Number(version(p)) > n)) return `${listed}: upgrade this endpoint (npm ${MANIFEST.name})`
  if (theirs.every(p => Number(version(p)) < n)) return `${listed}: upgrade the dialer`
  return listed
}

// The host of `spec`, an access point to dial; throws unless it is a ws://
// or wss:// address, and a non-loopback one needs wss:// (the rule of
// rutis-bridge's dialer).
export function dialHost(spec: string): string {
  let url: URL
  try { url = new URL(spec) } catch { throw new Error(`${spec} is not a ws:// or wss:// address`) }
  if (!['ws:', 'wss:'].includes(url.protocol) || !url.hostname) throw new Error(`${spec} is not a ws:// or wss:// address`)
  if (url.protocol === 'ws:' && !loopback(url.hostname)) throw new Error(`${spec}: a non-loopback address needs wss://`)
  return url.hostname
}

export interface Options {
  protocol?: string
  token?: string
  ca?: string | string[]
  cert?: string
  key?: string
  limits?: Limits
}

// What this side presents and accepts, from the environment: the token it
// sends (dial) or accepts (listen), a listener's certificate and key.
// Tests shorten the heartbeat: RUTIS_HEARTBEAT=<ping ms>,<timeout ms>.
export function optionsFromEnvironment(env: Record<string, string | undefined> = process.env): Options {
  const read = (name: string) => env[name] ? readFileSync(env[name]!, 'utf8') : undefined
  const options: Options = { token: env.RUTIS_TOKEN, cert: read('RUTIS_CERT'), key: read('RUTIS_KEY') }
  if (env.RUTIS_HEARTBEAT) {
    const [ping, timeout] = env.RUTIS_HEARTBEAT.split(',').map(Number)
    options.limits = { ...LIMITS, ping, timeout }
  }
  return options
}

// The parts of a WebSocket the channel uses, on either side.
interface Socket {
  send(text: string): void
  close(code: number, reason: string): void
  ping(): void
  terminate(): void
  open(): boolean
}

// The channel over `socket`; `events` is what the socket's handlers call.
function wrap(socket: Socket, { message, closed }: Handlers, limits: Limits) {
  let ended = false
  let failure: string | undefined
  let lastSeen = Date.now()
  const heartbeat = setInterval(() => {
    if (Date.now() - lastSeen > limits.timeout) {
      failure = `no message from the far end for ${limits.timeout / 1000} s: heartbeat timeout`
      socket.terminate()
      return
    }
    if (socket.open()) socket.ping()
  }, limits.ping)
  const events = {
    seen() { lastSeen = Date.now() },
    message(data: string | Buffer, binary: boolean) {
      lastSeen = Date.now()
      if (binary) {
        failure = 'binary messages are reserved for a binary encoding'
        socket.close(1003, failure)
        return
      }
      message(typeof data === 'string' ? data : data.toString('utf8'))
    },
    error(reason: string) { failure ??= reason },
    close(code: number, reason: string) {
      if (ended) return
      ended = true
      clearInterval(heartbeat)
      if (failure === undefined) {
        if (code === REPLACED) failure = 'replaced by a new connection'
        else if (code === REJECTED) failure = `rejected: ${reason}`
        else if (code === TOO_BIG) failure = `received a message over the limit (${TOO_BIG})`
        else if (code !== 1000 && code !== GOING_AWAY && code !== 1005) failure = `closed by the far end (${code}): ${reason}`
      }
      closed(failure, code)
    },
  }
  const channel: WebSocketChannel = {
    send(text) {
      if (ended) return
      if (Buffer.byteLength(text) > limits.maxMessage) {
        failure = `message over the limit (${TOO_BIG})`
        socket.close(TOO_BIG, failure)
        return
      }
      socket.send(text)
    },
    end() { socket.close(GOING_AWAY, '') },
    close(reason) { failure ??= reason; socket.close(GOING_AWAY, truncate(reason)) },
    replaced() { failure ??= 'replaced by a new connection'; socket.close(REPLACED, 'replaced by a new connection') },
  }
  return { channel, events }
}

// Dial `spec` (ws:// or wss://) once. A failure rejects with a
// ConnectError, classified from its status or close code, never from its
// text, as rutis-bridge's dialer does: 401 / 403 at the upgrade and a TLS
// handshake this side refuses (an error whose code names the certificate,
// as Node's do, since Bun 1.4.4; close code 1015 before) are an
// authentication failure; 400, 404, 405, 426 and a session protocol
// refusal are incompatible; the rest is retryable.
export function open(spec: string, handlers: Handlers, { protocol, token, ca, limits = LIMITS }: Options = {}): Promise<WebSocketChannel> {
  try { dialHost(spec) } catch (error: any) { return Promise.reject(new ConnectError('incompatible', error.message)) }
  const where = new URL(spec).host
  return new Promise((resolve, reject) => {
    let category: ConnectError['category'] | undefined
    let refused: string | undefined
    let opened = false
    let settled = false
    const fail = () => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      reject(new ConnectError(category ?? 'retryable', refused ?? `${where}: closed before the upgrade`))
    }
    let wrapped: ReturnType<typeof wrap> | undefined
    const socket: any = new WebSocket(spec, protocol ? [protocol] : [], {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
      ...(ca ? { tls: { ca } } : {}),
    } as any)
    const timer = setTimeout(() => {
      refused ??= `${where}: the upgrade took longer than ${limits.handshake / 1000} s`
      socket.terminate()
    }, limits.handshake)
    socket.once('unexpected-response', (_request: unknown, response: any) => {
      const status = response.statusCode
      category = status === 401 || status === 403 ? 'auth-rejected'
        : [400, 404, 405, 426].includes(status) ? 'incompatible' : 'retryable'
      // The access point's own words (for a session protocol refusal,
      // which end to upgrade): the first line, bounded, for the reader only.
      refused = `${where}: upgrade refused with ${status}`
      // Settled once the body is read, not on the socket's close: with a
      // listener here, the client leaves the request to it, as the `ws`
      // package does (Bun 1.4.4 does too: no close follows), so it ends
      // the socket itself, as the Node runtime does.
      const refuse = () => { socket.terminate(); fail() }
      if (typeof response.once !== 'function') { refuse(); return }
      let body = ''
      response.setEncoding?.('utf8')
      response.on('data', (chunk: string) => { if (body.length < 1024) body += chunk })
      response.once('end', () => {
        const said = (body.split('\n')[0] ?? '').trim().slice(0, 200).replace(/[\u0000-\u001f\u007f]/g, '')
        if (said) refused = `${where}: upgrade refused with ${status}: ${said}`
        refuse()
      })
      response.once('error', refuse)
      response.once('close', refuse)
    })
    // Bun's client may report one failure more than once: every report is
    // handled, none is thrown.
    socket.on('error', (error: Error & { code?: unknown }) => {
      if (wrapped) { wrapped.events.error(error.message); return }
      refused ??= `${where}: ${error.message}`
      // A certificate this side refuses (Bun 1.4.4 gives Node's codes).
      if (typeof error.code === 'string' && /CERT|SIGNATURE|ALTNAME|ISSUER/.test(error.code)) category ??= 'auth-rejected'
    })
    socket.once('open', () => {
      clearTimeout(timer)
      if (protocol && socket.protocol !== protocol) {
        category = 'incompatible'
        refused = `${where}: session protocol: the access point chose ${socket.protocol || 'none'}`
        socket.close(1002, 'no session protocol')
        return
      }
      opened = true
      wrapped = wrap({
        send: text => socket.send(text),
        close: (code, reason) => socket.close(code, reason),
        ping: () => socket.ping(),
        terminate: () => socket.terminate(),
        open: () => socket.readyState === WebSocket.OPEN,
      }, handlers, limits)
      socket.on('message', (data: Buffer | string, binary: boolean) => wrapped!.events.message(data, binary))
      socket.on('ping', () => wrapped!.events.seen())
      socket.on('pong', () => wrapped!.events.seen())
      resolve(wrapped.channel)
    })
    socket.on('close', (code: number, reason: Buffer | string) => {
      clearTimeout(timer)
      if (opened) { wrapped!.events.close(code, String(reason ?? '')); return }
      // The body of a refusal may still be coming: settle once it is read.
      setTimeout(() => {
        if (category === undefined && code === TLS_FAILED) category = 'auth-rejected'
        refused ??= `${where}: closed (${code}) before the upgrade`
        fail()
      }, 0)
    })
  })
}

const sameSecret = (a: string, b: string) => {
  const x = Buffer.from(a), y = Buffer.from(b)
  return x.length === y.length && timingSafeEqual(x, y)
}

export interface Listener {
  url: string
  // Stop accepting; established channels stay.
  stop(): void
}

// Listen on `spec` (ws:// or wss://, loopback only without TLS) and accept
// connections that present `token` and speak `protocol`; `accepted(channel)`
// runs for each, with the channel's handlers from `handlers()`.
export function listen(spec: string, { handlers, accepted }: { handlers(): Handlers, accepted(channel: WebSocketChannel): void }, { protocol, token, cert, key, limits = LIMITS }: Options = {}): Listener {
  const url = new URL(spec)
  const secure = url.protocol === 'wss:'
  if (!['ws:', 'wss:'].includes(url.protocol)) throw new ConnectError('incompatible', `${spec} is not a ws:// or wss:// address`)
  if (!secure && !loopback(url.hostname)) throw new ConnectError('incompatible', `${spec}: only a loopback listener may go without TLS`)
  if (secure && !(cert && key)) throw new ConnectError('incompatible', `${spec}: wss needs a certificate and key`)
  const path = url.pathname || '/'
  // URL drops an explicit port 0 (pick one); read the authority itself.
  const explicit = /^[a-z]+:\/\/(?:\[[^\]]*\]|[^/:]*):(\d+)/.exec(spec)?.[1]
  const port = explicit !== undefined ? Number(explicit) : secure ? 443 : 80
  type Data = { wrapped?: ReturnType<typeof wrap> }
  const refuse = (status: number, text: string) => new Response(`${text}\n`, { status })
  let server: ReturnType<typeof Bun.serve>
  try {
    server = Bun.serve<Data, never>({
      hostname: url.hostname.replace(/^\[|\]$/g, ''),
      port,
      ...(secure ? { tls: { cert, key } } : {}),
      fetch(request, server) {
        if (new URL(request.url).pathname !== path) return refuse(404, 'no rutis endpoint here')
        const header = request.headers.get('authorization') ?? ''
        if (!header.startsWith('Bearer ')) return refuse(401, 'credentials required')
        if (!token || !sameSecret(header.slice('Bearer '.length), token)) return refuse(403, 'not accepted here')
        const offered = (request.headers.get('sec-websocket-protocol') ?? '').split(',').map(p => p.trim()).filter(Boolean)
        if (!protocol || !offered.includes(protocol)) return refuse(400, protocolRefusal(protocol ?? '', offered))
        if (server.upgrade(request, { headers: { 'Sec-WebSocket-Protocol': protocol }, data: {} })) return undefined
        return refuse(400, 'not a WebSocket upgrade')
      },
      websocket: {
        maxPayloadLength: limits.maxMessage,
        // The heartbeat here decides when a silent far end is dropped.
        idleTimeout: 0,
        sendPings: false,
        open(ws) {
          const wrapped = wrap({
            send: text => { ws.send(text) },
            close: (code, reason) => ws.close(code, reason),
            ping: () => { ws.ping() },
            terminate: () => ws.terminate(),
            open: () => ws.readyState === 1,
          }, handlers(), limits)
          ws.data.wrapped = wrapped
          accepted(wrapped.channel)
        },
        message(ws, data) { ws.data.wrapped?.events.message(data as string | Buffer, typeof data !== 'string') },
        ping(ws) { ws.data.wrapped?.events.seen() },
        pong(ws) { ws.data.wrapped?.events.seen() },
        close(ws, code, reason) { ws.data.wrapped?.events.close(code, reason) },
      },
    })
  } catch (error: any) {
    throw new ConnectError('retryable', `${spec}: ${error.message}`)
  }
  const bound = new URL(spec)
  bound.port = String(server.port)
  return { url: bound.href, stop: () => server.stop(false) }
}
