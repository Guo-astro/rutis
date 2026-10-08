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
// child cleans up and exits, and only then does a new child greet.
//
// The child gets its socket as fd 3; on Windows, where an inherited pipe is
// not a socket, it dials a loopback address instead and presents a one-time
// token first (RUTIS_LOCAL_HANDOVER=loopback does the same elsewhere).
const LOOPBACK = process.platform === 'win32' || process.env.RUTIS_LOCAL_HANDOVER === 'loopback'
const TOKEN = 'RUTIS_CHANNEL_TOKEN'

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
export async function serve({ spec, id, peer, anchor }) {
  const runner = fileURLToPath(new URL('./runner.mjs', import.meta.url))
  let current
  let switching = Promise.resolve()
  let latest

  // Messages that arrive before the session's child is up wait for it.
  function relay() {
    const pending = []
    let target, ended = false
    return {
      handlers: {
        message: text => target ? target.send(text) : pending.push(text),
        closed: () => { ended = true; target?.end() },
      },
      attach(channel) {
        target = channel
        for (const text of pending.splice(0)) channel.send(text)
        if (ended) channel.end()
      },
      get ended() { return ended },
    }
  }

  async function stop(session) {
    session.childChannel?.close('replaced by a new connection')
    if (session.child.exitCode === null && session.child.signalCode === null) {
      const exited = new Promise(resolve => session.child.once('exit', resolve))
      const killed = setTimeout(() => session.child.kill('SIGKILL'), 5000)
      await exited
      clearTimeout(killed)
    }
  }

  async function start(ws, link) {
    const token = LOOPBACK ? randomBytes(32).toString('hex') : undefined
    const listener = LOOPBACK ? await loopback(token) : undefined
    const args = [...process.execArgv, runner, listener?.address ?? 'fd:3', '--id', id, '--format', 'endpoint']
    if (peer) args.push('--peer', peer)
    args.push(anchor)
    const child = spawn(process.execPath, args, {
      stdio: LOOPBACK ? ['ignore', 'inherit', 'inherit'] : ['ignore', 'inherit', 'inherit', 'pipe'],
      env: LOOPBACK ? { ...process.env, [TOKEN]: token } : process.env,
    })
    const session = { ws, child }
    child.once('exit', () => { listener?.server.close(); if (current === session) { current = undefined; ws.close('runtime session ended') } })
    const socket = LOOPBACK
      ? await Promise.race([listener.accepted, new Promise(resolve => child.once('exit', () => resolve(undefined)))])
      : child.stdio[3]
    if (!socket) {
      // The child ended before connecting: so does the session, now, since
      // no later exit will.
      ws.close('runtime session ended: its process exited before connecting')
      return undefined
    }
    session.childChannel = frame(socket, {
      message: text => ws.send(text),
      closed: () => { if (current === session) ws.close('runtime session ended') },
    })
    link.attach(session.childChannel)
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
