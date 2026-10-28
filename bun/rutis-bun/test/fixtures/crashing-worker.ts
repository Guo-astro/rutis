// An I/O worker that greets, then crashes without saying so. It crashes when
// the session's first frame (its greeting) reaches it: after the session has
// started, with this worker's greeting already sent.
import { workerData } from 'node:worker_threads'

const { port, signal } = workerData
const send = (message: unknown) => {
  port.postMessage(message)
  Atomics.add(signal, 0, 1)
  Atomics.notify(signal, 0)
}
port.on('message', (message: any) => {
  if (message?.frame) throw new Error('the worker broke')
})
send({ ready: true })
send({ op: 'hello', version: 4, endpoint: 'main', capabilities: [] })
