import { spawn } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { createServer } from 'node:net'
import { fileURLToPath } from 'node:url'
import * as websocket from './channel/websocket.mjs'
import { frame } from './channel/unix.mjs'
import { ENDPOINT_PROTOCOL } from './session.mjs'

// A runtime that stays up and listens for its controller. Each session runs
// in a child runner of its own, relayed message by message between the
// WebSocket and the child's inherited socket, so a session's lease (its
// rows, proxies, references, its Cordis Context) ends with that child. A
// newer connection takes over: the old one is closed as replaced, the old
// child cleans up and exits, and only then does a new child greet. The new
// connection's heartbeat starts at that hello, not at the accept, so a
// cleanup longer than the heartbeat timeout does not close it (#1182).
//
// The child gets its socket as fd 3; on Windows, where an inherited pipe is
// not a socket, it dials a loopback address instead and presents a one-time
// token first (RUTIS_LOCAL_HANDOVER=loopback does the same elsewhere).
const LOOPBACK = process.platform === 'win32' || process.env.RUTIS_LOCAL_HANDOVER === 'loopback'
const TOKEN = 'RUTIS_CHANNEL_TOKEN'

// How long a child's cleanups may take once its session ended:
// RUTIS_SHUTDOWN_TIMEOUT seconds, 10 by default; in ms.
export function shutdownDeadline() {
  const seconds = Number(process.env.RUTIS_SHUTDOWN_TIMEOUT)
  return Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : 10_000
}

// What a child still running past its deadline gets: rutis-host gives a
// runtime 2 s past the deadline at a restart, and this is the same
// handover, so a child that uses all of it is not cut off as it finishes.
const KILL_MARGIN = 2000

// Wait for `child` to exit, killing it once its cleanups had their
// deadline and that margin.
export async function ended(child) {
  if (child.exitCode !== null || child.signalCode !== null) return
  const exited = new Promise(resolve => child.once('exit', resolve))
  const killed = setTimeout(() => child.kill('SIGKILL'), shutdownDeadline() + KILL_MARGIN)
  await exited
  clearTimeout(killed)
}

// A loopback listener for one child: its `tcp:` address, and the socket of
// the first connection that presents `token`.
export async function loopback(token) {
  const server = createServer()
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve) })
  const accepted = new Promise(resolve => {
    server.on('connection', socket => {
      // A connection that fails before it authenticates is dropped alone:
      // its error must not end this process.
      const onError = () => socket.destroy()
      socket.on('error', onError)
      // Read the token line by hand: anything after it is the session.
      let seen = Buffer.alloc(0)
      const onData = chunk => {
        seen = Buffer.concat([seen, chunk])
        const end = seen.indexOf(10)
        if (end < 0) { if (seen.length > 256) socket.destroy(); return }
        socket.off('data', onData)
        if (seen.subarray(0, end).toString() !== token) return socket.destroy()
        socket.pause()
        const rest = seen.subarray(end + 1)
        if (rest.length) socket.unshift(rest)
        // From here the session's framing handles its errors.
        socket.off('error', onError)
        server.close()
        resolve(socket)
      }
      socket.on('data', onData)
    })
  })
  return { address: `tcp:127.0.0.1:${server.address().port}`, accepted, server }
}

// Start the runner of one session, as `id` expecting `peer` (`args`: more
// flags for it); `exited` runs when it exits. Resolves to { child, socket },
// the socket undefined when it exited before connecting.
export async function spawnSession({ id, peer, anchor, args: extra = [] }, exited) {
  const runner = fileURLToPath(new URL('./runner.mjs', import.meta.url))
  const token = LOOPBACK ? randomBytes(32).toString('hex') : undefined
  const listener = LOOPBACK ? await loopback(token) : undefined
  const args = [...process.execArgv, runner, listener?.address ?? 'fd:3', '--id', id, ...extra]
  if (peer) args.push('--peer', peer)
  args.push(anchor)
  const child = spawn(process.execPath, args, {
    stdio: LOOPBACK ? ['ignore', 'inherit', 'inherit'] : ['ignore', 'inherit', 'inherit', 'pipe'],
    env: LOOPBACK ? { ...process.env, [TOKEN]: token } : process.env,
  })
  child.once('exit', () => { listener?.server.close(); exited?.() })
  const socket = LOOPBACK
    ? await Promise.race([listener.accepted, new Promise(resolve => child.once('exit', () => resolve(undefined)))])
    : child.stdio[3]
  return { child, socket }
}

export async function serve({ spec, id, peer, anchor }) {
  let current
  let switching = Promise.resolve()
  let latest

  // Messages that arrive before the session's child is up wait for it.
  function relay() {
    const pending = []
    let target, closed = false
    return {
      handlers: {
        message: text => target ? target.send(text) : pending.push(text),
        closed: () => { closed = true; target?.end() },
      },
      attach(channel) {
        target = channel
        for (const text of pending.splice(0)) channel.send(text)
        if (closed) channel.end()
      },
      get ended() { return closed },
    }
  }

  async function stop(session) {
    session.childChannel?.close('replaced by a new connection')
    await ended(session.child)
  }

  async function start(ws, link) {
    const session = { ws }
    const { child, socket } = await spawnSession({ id, peer, anchor }, () => {
      if (current === session) { current = undefined; ws.close('the runtime ended') }
    })
    session.child = child
    if (!socket) {
      // The child ended before connecting: so does the session, now, since
      // no later exit will.
      ws.close('the runtime exited before connecting')
      return undefined
    }
    session.childChannel = frame(socket, {
      message: text => ws.send(text),
      closed: () => { if (current === session) ws.close('the runtime ended') },
    })
    link.attach(session.childChannel)
    // Served: from here silence counts, not from the accept, so a cleanup
    // longer than the heartbeat timeout does not close it before its hello
    // (#1182).
    ws.start()
    return session
  }

  const listener = await websocket.listen(spec, {
    handlers: () => { latest = relay(); return latest.handlers },
    accepted: ws => {
      const link = latest
      switching = switching.then(async () => {
        if (current) {
          const old = current
          current = undefined
          old.ws.replaced()
          await stop(old)
        }
        if (link.ended) return
        current = await start(ws, link)
      })
    },
  }, { protocol: `rutis.${ENDPOINT_PROTOCOL}`, ...websocket.optionsFromEnvironment() })
  process.stderr.write(`rutis: listening on ${listener.url}\n`)
  // Serves until killed.
  await new Promise(() => {})
}
