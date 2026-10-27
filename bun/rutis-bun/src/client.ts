// A session whose I/O runs in a worker, so that the main thread can block in
// a synchronous call (Atomics.wait) and still receive the frames that answer
// it, or that call back into it.
import { Worker, MessageChannel, receiveMessageOnPort, type MessagePort } from 'node:worker_threads'
import { Session } from './session.ts'
import { hostGone } from './errors.ts'

export interface Endpoint { local: string, expected?: string, verified?: string, declare?: string[] }
type Dispatch = (target: string, method: string, args: unknown) => unknown

export class Client {
  #port: MessagePort
  #signal = new Int32Array(new SharedArrayBuffer(4))
  #worker: Worker
  #exited: Promise<number>
  #session: Session

  // `worker`: the I/O worker's script; tests give one that misbehaves.
  // `inline` and `requires`: the session's (control methods run as their
  // frame arrives; capabilities the far end must declare).
  // `ended`: an Int32Array on a SharedArrayBuffer whose first element the
  // I/O worker sets to 1 once the channel ends, whatever this thread does.
  constructor(channel: string, { dispatch, settled, endpoint, token, inline, requires, ended, worker = new URL('./io-worker.ts', import.meta.url) }: { dispatch: Dispatch, settled?: () => void, endpoint?: Endpoint, token?: string, inline?: string[], requires?: string[], ended?: Int32Array, worker?: URL }) {
    const { port1, port2 } = new MessageChannel()
    this.#port = port1
    this.#session = new Session({
      send: (frame: string) => this.#port.postMessage({ frame }),
      // A session fault (an invalid frame, say) ends the channel, saying why.
      abort: (error: Error) => this.#port.postMessage({ abort: true, reason: `session fault: ${error?.message ?? error}` }),
      dispatch,
      settled,
      endpoint,
      inline,
      requires,
      pump: (done: () => boolean) => {
        const sequence = Atomics.load(this.#signal, 0)
        let packet
        while ((packet = receiveMessageOnPort(this.#port))) this.#receive(packet.message)
        if (!done()) Atomics.wait(this.#signal, 0, sequence)
      },
    })
    this.#port.on('message', message => this.#receive(message))
    this.#worker = new Worker(worker, {
      workerData: { channel, token, port: port2, signal: this.#signal, ended }, transferList: [port2],
    } as any)
    this.#worker.on('error', error => this.#session.close(error))
    this.#exited = new Promise(resolve => this.#worker.once('exit', code => {
      this.reason ??= `the communication worker exited (${code}) before its channel ended`
      this.#session.close(hostGone(`the communication worker exited (${code})`))
      this.#port.close()
      resolve(code)
    }))
  }
  // Why the channel ended, once it did.
  reason?: string
  #receive(message: any) {
    if (message?.ready) { this.#session.start(); return }
    if (message?.closed) { this.reason ??= message.closed; this.#session.close(hostGone(message.closed)); return }
    this.#session.receive(message)
  }
  // Connect on `channel` and greet; rejects when the channel or the
  // handshake fails.
  static async connect(channel: string, options: ConstructorParameters<typeof Client>[1]): Promise<Client> {
    const client = new Client(channel, options)
    try { await client.#session.ready; return client }
    catch (error) { client.#port.postMessage({ abort: true }); await client.#exited; throw error }
  }
  get session() { return this.#session }
  call(target: string, method: string, args: unknown) { return this.#session.invoke(target, method, args) }
  callAsync(target: string, method: string, args: unknown) { return this.#session.invokeAsync(target, method, args) }
  callAnswered(target: string, method: string, args: unknown, answered: (result: { ok: boolean, value: unknown }) => void) { this.#session.invokeAnswered(target, method, args, answered) }
  callFunctionAsync(fn: unknown, args: unknown[]) { return this.#session.callFunctionAsync(fn, args) }
  release(value: unknown) { this.#session.release(value) }
  drain() { return this.#session.drain() }
  closed() { return this.#exited }
  // End this side: the far end sees the channel end.
  end() { this.#port.postMessage({ end: true }) }
}
