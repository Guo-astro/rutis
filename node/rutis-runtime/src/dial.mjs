import { readFileSync } from 'node:fs'
import { X509Certificate } from 'node:crypto'
import { rootCertificates } from 'node:tls'
import * as websocket from './channel/websocket.mjs'
import { frame } from './channel/unix.mjs'
import { keepDialing } from './dialing.mjs'
import { spawnSession } from './serve.mjs'
import { ENDPOINT_PROTOCOL, greetingOf } from './session.mjs'

// A runtime that dials a rutis access point and stays connected (`dialing`
// has the rules). Each connection is a new session, run, as when listening
// (serve.mjs), by a child runner of its own, relayed message by message
// between the WebSocket and the child's socket: a session's lease (its
// rows, proxies, references, its Cordis Context) ends with that child, and
// the next connection is mounted afresh. This process reads the access
// point's hello on the way through, to say it connected or why it refuses
// it. Stopped (SIGTERM, Ctrl-C), the child unloads its rows while the
// access point still hears from it, then the connection closes, and
// nothing dials again.

// What can be checked before dialing: what retrying cannot fix fails here
// (an address that is not ws:// or wss://, ws:// to another machine, an id
// no access point could admit, a RUTIS_CA that holds no certificate).
// Returns the options to dial with, or throws.
export function dialOptions(spec, id, env = process.env) {
  websocket.dialHost(spec)
  if (!/^[a-z0-9-]+$/.test(id ?? '')) throw new Error(`invalid endpoint id ${id}: use lower-case letters, digits and -`)
  const options = websocket.optionsFromEnvironment({ ...env, RUTIS_CA: undefined })
  if (env.RUTIS_CA) {
    let ca
    try {
      ca = readFileSync(env.RUTIS_CA, 'utf8')
      // Throws for a file that holds no certificate.
      new X509Certificate(ca)
    } catch (error) {
      throw new Error(`RUTIS_CA ${env.RUTIS_CA}: ${error.message}`)
    }
    // Trusted besides the roots Node trusts by default: `ca` alone would
    // replace them.
    options.ca = [...rootCertificates, ca]
  }
  return options
}

export async function dialOut({ spec, id, peer, anchor }) {
  let options
  try { options = dialOptions(spec, id) } catch (error) {
    process.stderr.write(`rutis: ${error.message}\n`)
    return 1
  }
  // Spent once read: what this process starts does not inherit it.
  delete process.env.RUTIS_TOKEN
  // A host older than 0.13 is refused at its hello, and not dialed again.
  const endpoint = { local: id, expected: peer, requires: ['events-v2'] }
  let stop
  const stopped = new Promise(resolve => { stop = resolve })
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => stop())

  // The connection: messages from the access point wait for the session's
  // child; the first one, its hello, is read here on the way.
  function dial() {
    const pending = []
    const link = { target: undefined, ended: undefined }
    let greet
    link.greeted = new Promise((resolve, reject) => { greet = { resolve, reject } })
    link.greeted.catch(() => {})
    link.closed = new Promise(resolve => {
      link.handlers = {
        message: text => {
          if (greet) {
            const settle = greet
            greet = undefined
            let hello
            try { hello = JSON.parse(text) } catch {}
            if (hello?.op === 'hello') {
              try { greetingOf(hello, endpoint) } catch (refusal) { settle.reject(refusal); return }
            }
            // Anything else is the session's to refuse.
            settle.resolve()
          }
          if (link.target) link.target.send(text); else pending.push(text)
        },
        closed: (reason, code) => {
          link.ended = { reason, code }
          link.target?.end()
          resolve(link.ended)
        },
      }
    })
    link.attach = channel => {
      link.target = channel
      for (const text of pending.splice(0)) channel.send(text)
      if (link.ended) channel.end()
    }
    return websocket.open(spec, link.handlers, { protocol: `rutis.${ENDPOINT_PROTOCOL}`, ...options })
      .then(ws => Object.assign(link, { ws, close: reason => ws.close(reason) }))
  }

  function start(link) {
    const { ws } = link
    // What the connection's close says once the child's session ended.
    let ending = 'runtime session ended'
    let child
    let exit
    const exited = new Promise(resolve => { exit = resolve })
    // The child: started now, its hello going out while the access point's
    // comes in.
    const ready = spawnSession({ id, peer, anchor, args: ['--supervisor', 'dial'] }, () => { exit(); ws.close(ending) })
      .then(started => {
        child = started.child
        if (!started.socket) { ws.close('runtime session ended: its process exited before connecting'); return }
        link.attach(frame(started.socket, {
          message: text => ws.send(text),
          closed: () => ws.close(ending),
        }))
      })
    return {
      greeted: link.greeted,
      ended: link.closed,
      close: reason => ws.close(reason),
      // The connection ended: so does the child's session, and its rows go.
      async end() {
        const ended = await link.closed
        await ready
        await exited
        return ended
      },
      // Stopped: the child unloads its rows and ends its session (it heard
      // the signal too, from a terminal), then the connection closes.
      async stop() {
        ending = 'the runtime stopped'
        await ready
        if (child && child.exitCode === null && child.signalCode === null) {
          // Windows has no SIGTERM to send: Ctrl-C reached the child already.
          if (process.platform !== 'win32') child.kill('SIGTERM')
          const killed = setTimeout(() => child.kill('SIGKILL'), shutdownDeadline() + 5000)
          await exited
          clearTimeout(killed)
        }
        ws.close('the runtime stopped')
        await Promise.race([link.closed, new Promise(resolve => setTimeout(resolve, 2000).unref())])
      },
    }
  }

  return keepDialing({ address: `${spec} as ${id}`, dial, start, stopped })
}

// How long the child's cleanups may take: RUTIS_SHUTDOWN_TIMEOUT seconds,
// 10 by default; in ms.
function shutdownDeadline() {
  const seconds = Number(process.env.RUTIS_SHUTDOWN_TIMEOUT)
  return Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : 10_000
}
