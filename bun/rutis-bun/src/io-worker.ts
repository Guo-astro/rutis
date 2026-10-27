// The I/O side of a session: owns the channel, decodes frames and hands them
// to the main thread, which may be blocked in a synchronous call. Each frame
// handed over bumps the shared counter the main thread waits on.
import { workerData } from 'node:worker_threads'
import { open, type Channel } from './channel.ts'
import { decode } from './codec.ts'

const { channel: spec, token, port, signal, ended: endedFlag } = workerData
function send(message: unknown) {
  port.postMessage(message)
  Atomics.add(signal, 0, 1)
  Atomics.notify(signal, 0)
}
// The channel ended: said at once, also to a main thread that never looks
// at its messages again (a plugin keeps it busy), for a watchdog (#682).
function channelEnded() {
  if (endedFlag) { Atomics.store(endedFlag, 0, 1); Atomics.notify(endedFlag, 0) }
}

// The main thread cannot run a Worker 'error' handler while it waits in
// Atomics.wait: publish an uncaught failure here, before the worker ends.
process.on('uncaughtExceptionMonitor', (error: Error) => send({ closed: error.message }))

let channel: Channel | undefined
// What ends the channel when nothing failed (main.ts knows this text).
let failure = 'the host disconnected'
port.on('message', (message: any) => {
  if (message.abort) { channel?.close(message.reason); return }
  if (message.end) { channel?.end(); return }
  if (message.frame) channel?.send(message.frame)
})

// The worker lives until the channel ends. Its socket should keep it alive,
// but older Bun lets a worker whose only pending work is an inherited
// socket end (normally, mid-session); a timer keeps it.
const alive = setInterval(() => {}, 1 << 30)
try {
  let ended!: () => void
  const done = new Promise<void>(resolve => { ended = resolve })
  // Frames that arrive before the session started wait: the session sends
  // its own hello on `ready`, and must not answer first.
  let early: string[] | undefined = []
  const forward = (text: string) => {
    try { send(decode(text)) } catch (error: any) { channel!.close(error.message) }
  }
  channel = await open(spec, {
    message: text => { if (early) early.push(text); else forward(text) },
    closed: reason => { if (reason) failure = reason; channelEnded(); ended() },
  }, token)
  send({ ready: true })
  for (const text of early) forward(text)
  early = undefined
  await done
} catch (error: any) {
  failure = error.message
} finally {
  channelEnded()
  clearInterval(alive)
  channel?.close()
  send({ closed: failure })
  port.close()
}
