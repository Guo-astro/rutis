// A remote Bun runtime: it listens for its controller (`listen:ws://…`,
// `listen:wss://…`), or dials a rutis access point and dials again
// (`dial:ws://…`, `dial:wss://…`, by the rules of `dialing.ts`).
//
// Each session runs in a child runtime of its own (this file's main.ts on a
// loopback channel, presenting a one-time token first), relayed message by
// message between the WebSocket and the child's socket: a session's lease
// (its rows, proxies and references) ends with that child, and the next
// connection is mounted afresh. A listening runtime serves one controller
// at a time: a newer connection takes over, the old one is closed as
// replaced, the old child cleans up and exits, and only then does a new
// child greet. A dialing one reads the access point's hello on the way
// through, to say it connected or why it refuses it; stopped (SIGTERM,
// Ctrl-C), its child unloads its rows while the access point still hears
// from it, then the connection closes, and nothing dials again. The same
// behaviour as the Node runtime's (its serve.mjs and dial.mjs), in this
// runtime's own code.
import { spawn, type ChildProcess } from 'node:child_process'
import { randomBytes, X509Certificate } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { createServer, type Socket } from 'node:net'
import { rootCertificates } from 'node:tls'
import { fileURLToPath } from 'node:url'
import { frame, TOKEN, type Channel } from './channel.ts'
import { keepDialing } from './dialing.ts'
import { ENDPOINT_PROTOCOL, greetingOf } from './session.ts'
import * as websocket from './websocket.ts'

const PROTOCOL = `rutis.${ENDPOINT_PROTOCOL}`

export interface Remote { id: string, peer?: string, project: string }

// A loopback listener for one child: its `tcp:` address, and the socket of
// the first connection that presents `token`.
export async function loopback(token: string) {
  const server = createServer()
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', () => resolve()) })
  const accepted = new Promise<Socket>(resolve => {
    server.on('connection', socket => {
      // A connection that fails before it authenticates is dropped alone.
      const onError = () => socket.destroy()
      socket.on('error', onError)
      // Read the token line by hand: anything after it is the session.
      let seen = Buffer.alloc(0)
      const onData = (chunk: Buffer) => {
        seen = Buffer.concat([seen, chunk])
        const end = seen.indexOf(10)
        if (end < 0) { if (seen.length > 256) socket.destroy(); return }
        socket.off('data', onData)
        if (seen.subarray(0, end).toString() !== token) { socket.destroy(); return }
        socket.pause()
        const rest = seen.subarray(end + 1)
        if (rest.length) socket.unshift(rest)
        socket.off('error', onError)
        server.close()
        resolve(socket)
      }
      socket.on('data', onData)
    })
  })
  const { port } = server.address() as { port: number }
  return { address: `tcp:127.0.0.1:${port}`, accepted, server }
}

// Start the runtime of one session (`args`: more flags for it); `exited`
// runs when it exits. Resolves to { child, socket }, the socket undefined
// when it exited before connecting.
export async function spawnSession({ id, peer, project }: Remote, args: string[], exited: () => void): Promise<{ child: ChildProcess, socket?: Socket }> {
  const token = randomBytes(32).toString('hex')
  const listener = await loopback(token)
  const main = fileURLToPath(new URL('./main.ts', import.meta.url))
  const child = spawn(process.execPath, [
    '--no-install', '--no-env-file', main, listener.address,
    '--id', id, ...(peer ? ['--peer', peer] : []), ...args, project,
  ], { stdio: ['ignore', 'inherit', 'inherit'], env: { ...process.env, [TOKEN]: token } })
  const gone = new Promise<undefined>(resolve => child.once('exit', () => {
    listener.server.close()
    exited()
    resolve(undefined)
  }))
  const socket = await Promise.race([listener.accepted, gone])
  return { child, socket }
}

// The channel on a child's socket, which the loopback listener paused.
function framed(socket: Socket, handlers: Parameters<typeof frame>[1]): Channel {
  const channel = frame(socket, handlers)
  socket.resume()
  return channel
}

const running = (child?: ChildProcess) => !!child && child.exitCode === null && child.signalCode === null

async function ended(child: ChildProcess, killAfter: number) {
  if (!running(child)) return
  const exited = new Promise(resolve => child.once('exit', resolve))
  const killed = setTimeout(() => child.kill('SIGKILL'), killAfter)
  await exited
  clearTimeout(killed)
}

// How long a child's cleanups may take: RUTIS_SHUTDOWN_TIMEOUT seconds, 10
// by default; in ms.
function shutdownDeadline(): number {
  const seconds = Number(process.env.RUTIS_SHUTDOWN_TIMEOUT)
  return Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : 10_000
}

// Messages from the far end that wait for the session's child.
function relay(first?: (text: string) => boolean) {
  const pending: string[] = []
  let target: Channel | undefined
  let closed: { reason?: string, code?: number } | undefined
  let done!: (ended: { reason?: string, code?: number }) => void
  const link = {
    ended: new Promise<{ reason?: string, code?: number }>(resolve => { done = resolve }),
    handlers: {
      message(text: string) {
        // `first` reads the far end's hello: false drops it.
        if (first) {
          const check = first
          first = undefined
          if (!check(text)) return
        }
        if (target) target.send(text); else pending.push(text)
      },
      closed(reason: string | undefined, code?: number) {
        closed = { reason, code }
        target?.end()
        done(closed)
      },
    } as websocket.Handlers,
    attach(channel: Channel) {
      target = channel
      for (const text of pending.splice(0)) channel.send(text)
      if (closed) channel.end()
    },
    get closed() { return closed !== undefined },
  }
  return link
}

// Listen for the controlling rutis; serves until killed.
export async function serve(spec: string, remote: Remote): Promise<never> {
  interface Live { ws: websocket.WebSocketChannel, child?: ChildProcess, childChannel?: Channel }
  let current: Live | undefined
  let switching = Promise.resolve()
  let latest: ReturnType<typeof relay>

  async function start(ws: websocket.WebSocketChannel, link: ReturnType<typeof relay>): Promise<Live | undefined> {
    const session: Live = { ws }
    const { child, socket } = await spawnSession(remote, [], () => {
      if (current === session) { current = undefined; ws.close('the runtime ended') }
    })
    session.child = child
    if (!socket) {
      ws.close('the runtime exited before connecting')
      return undefined
    }
    session.childChannel = framed(socket, {
      message: text => ws.send(text),
      closed: () => { if (current === session) ws.close('the runtime ended') },
    })
    link.attach(session.childChannel)
    return session
  }

  const options = { protocol: PROTOCOL, ...websocket.optionsFromEnvironment() }
  const listener = websocket.listen(spec, {
    handlers: () => { latest = relay(); return latest.handlers },
    accepted: ws => {
      const link = latest
      switching = switching.then(async () => {
        if (current) {
          const old = current
          current = undefined
          old.ws.replaced()
          old.childChannel?.close('replaced by a new connection')
          if (old.child) await ended(old.child, 5000)
        }
        if (link.closed) return
        current = await start(ws, link)
      })
    },
  }, options)
  process.stderr.write(`rutis: listening on ${listener.url}\n`)
  return new Promise<never>(() => {})
}

// What can be checked before dialing: what retrying cannot fix fails here
// (an address that is not ws:// or wss://, ws:// to another machine, an id
// no access point could admit, a RUTIS_CA that holds no certificate).
// Returns the options to dial with, or throws.
export function dialOptions(spec: string, id: string | undefined, env: Record<string, string | undefined> = process.env): websocket.Options {
  websocket.dialHost(spec)
  if (!/^[a-z0-9-]+$/.test(id ?? '')) throw new Error(`invalid endpoint id ${id}: use lower-case letters, digits and -`)
  const options = websocket.optionsFromEnvironment({ RUTIS_TOKEN: env.RUTIS_TOKEN, RUTIS_HEARTBEAT: env.RUTIS_HEARTBEAT })
  if (env.RUTIS_CA) {
    let ca: string
    try {
      ca = readFileSync(env.RUTIS_CA, 'utf8')
      // Throws for a file that holds no certificate.
      new X509Certificate(ca)
    } catch (error: any) {
      throw new Error(`RUTIS_CA ${env.RUTIS_CA}: ${error.message}`)
    }
    // Trusted besides the default roots: `ca` alone would replace them.
    options.ca = [...rootCertificates, ca]
  }
  return { protocol: PROTOCOL, ...options }
}

// Dial the access point at `spec` until stopped (0) or it is incompatible
// (1).
export async function dialOut(spec: string, remote: Remote): Promise<number> {
  let options: websocket.Options
  try { options = dialOptions(spec, remote.id) } catch (error: any) {
    process.stderr.write(`rutis: ${error.message}\n`)
    return 1
  }
  // Spent once read: what this process starts does not inherit it.
  delete process.env.RUTIS_TOKEN
  const endpoint = { local: remote.id, expected: remote.peer }
  let stop!: () => void
  const stopped = new Promise<void>(resolve => { stop = resolve })
  for (const signal of ['SIGINT', 'SIGTERM'] as const) process.on(signal, () => stop())

  type Connection = ReturnType<typeof relay> & { ws: websocket.WebSocketChannel, greeted: Promise<void>, close(reason: string): void }
  function dial(): Promise<Connection> {
    let settle!: { resolve(): void, reject(error: Error): void }
    const greeted = new Promise<void>((resolve, reject) => { settle = { resolve, reject } })
    greeted.catch(() => {})
    // The access point's hello, read on the way: one this side refuses is
    // not passed on; anything else is the session's to refuse.
    const link = relay(text => {
      let hello: any
      try { hello = JSON.parse(text) } catch {}
      if (hello?.op === 'hello') {
        try { greetingOf(hello, endpoint) } catch (refusal: any) { settle.reject(refusal); return false }
      }
      settle.resolve()
      return true
    })
    return websocket.open(spec, link.handlers, options)
      .then(ws => Object.assign(link, { ws, greeted, close: (reason: string) => ws.close(reason) }))
  }

  function start(link: Connection) {
    const { ws } = link
    // What the connection's close says once the child's session ended.
    let ending = 'the runtime ended'
    let child: ChildProcess | undefined
    let exit!: () => void
    const exited = new Promise<void>(resolve => { exit = resolve })
    const ready = spawnSession(remote, ['--supervisor', 'dial'], () => { exit(); ws.close(ending) })
      .then(started => {
        child = started.child
        if (!started.socket) { ws.close('the runtime exited before connecting'); return }
        link.attach(framed(started.socket, {
          message: text => ws.send(text),
          closed: () => ws.close(ending),
        }))
      })
    return {
      greeted: link.greeted,
      ended: link.ended,
      close: (reason: string) => ws.close(reason),
      // The connection ended: so does the child's session, and its rows go.
      async end() {
        const closed = await link.ended
        await ready
        await exited
        return closed
      },
      // Stopped: the child unloads its rows and ends its session (it heard
      // the signal too, from a terminal), then the connection closes.
      async stop() {
        ending = 'the runtime stopped'
        await ready
        if (running(child)) {
          child!.kill('SIGTERM')
          await ended(child!, shutdownDeadline() + 5000)
        }
        ws.close('the runtime stopped')
        await Promise.race([link.ended, new Promise(resolve => setTimeout(resolve, 2000))])
      },
    }
  }

  return keepDialing({ address: `${spec} as ${remote.id}`, dial, start, stopped })
}
