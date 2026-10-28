import { workerData } from 'node:worker_threads'
import { open } from './channel/index.mjs'
import { decode } from './codec.mjs'

const { channel: spec, port, signal, ended: endedFlag } = workerData
function send(message) {
  port.postMessage(message)
  Atomics.add(signal, 0, 1)
  Atomics.notify(signal, 0)
}
// The channel ended: said at once, also to a main thread that never looks
// at its messages again (a plugin keeps it busy), for a watchdog (#682).
function channelEnded() {
  if (endedFlag) { Atomics.store(endedFlag, 0, 1); Atomics.notify(endedFlag, 0) }
}

// Main-thread Worker error handlers cannot run during Atomics.wait(). Publish
// an uncaught worker failure before Node terminates the communication thread.
process.on('uncaughtExceptionMonitor', error => send({ closed: error.message }))

let channel
// What ends the channel when nothing failed: the far end went (client.mjs
// knows this text).
let failure = 'the host disconnected'
port.on('message', message => {
  if (message.abort) { channel?.close(); return }
  if (message.end) { channel?.end(); return }
  if (message.frame) channel?.send(message.frame)
})

try {
  let ended
  const done = new Promise(resolve => { ended = resolve })
  // Frames that arrive before the session started (the far end's hello can
  // come in the same read as the upgrade, before `open` resumes) wait: the
  // session sends its own hello on `ready`, and must not answer first.
  let early = []
  const forward = text => {
    try { send(decode(text)) } catch (error) { channel.close(error.message) }
  }
  const handlers = {
    message: text => { if (early) early.push(text); else forward(text) },
    closed: reason => { if (reason) failure = reason; channelEnded(); ended() },
  }
  channel = await open(spec, handlers)
  send({ ready: true })
  for (const text of early) forward(text)
  early = undefined
  await done
} catch (error) {
  failure = error.message
} finally {
  channelEnded()
  channel?.close()
  send({ closed: failure })
  port.close()
}
