import { Worker, MessageChannel, receiveMessageOnPort } from 'node:worker_threads'
import { Session } from './session.mjs'
import { hostGone } from './errors.mjs'

export class Process {
  #port
  #signal = new Int32Array(new SharedArrayBuffer(4))
  #worker
  #exited
  #session

  // `ended`: an Int32Array on a SharedArrayBuffer whose first element the
  // I/O worker sets to 1 once the channel ends, whatever this thread does.
  constructor({ channel, dispatch, settled, endpoint, inline, holds, report, owner, within, ended } = {}) {
    const { port1, port2 } = new MessageChannel()
    this.#port = port1
    this.#session = new Session({
      send: frame => this.#port.postMessage({ frame }),
      abort: () => this.#port.postMessage({ abort: true }),
      dispatch: dispatch ?? (() => { throw new Error('application has no exported service target') }),
      settled,
      endpoint,
      inline,
      holds,
      report,
      owner,
      within,
      pump: done => {
        const sequence = Atomics.load(this.#signal, 0)
        let packet
        while ((packet = receiveMessageOnPort(this.#port))) this.#receive(packet.message)
        if (!done()) Atomics.wait(this.#signal, 0, sequence)
      },
    })
    this.#port.on('message', message => this.#receive(message))
    this.#worker = new Worker(new URL('./io-worker.mjs', import.meta.url), {
      workerData: { channel, port: port2, signal: this.#signal, ended }, transferList: [port2],
    })
    this.#worker.on('error', error => this.#session.close(error))
    this.#exited = new Promise(resolve => this.#worker.once('exit', code => {
      this.#session.close(hostGone(`the communication worker exited (${code})`))
      this.#port.close()
      resolve(code)
    }))
  }
  #receive(message) {
    if (message?.ready) { this.#session.start(); return }
    if (message?.closed) { this.#session.close(hostGone(message.closed)); return }
    this.#session.receive(message)
  }
  // `endpoint` ({ local, expected? }): this side's endpoint id and the far end's;
  // `options`: the session's `inline`, `holds`, `report`,
  // `owner` and `within`.
  static async connect(channel, dispatch, settled, endpoint, options = {}) {
    const process = new Process({ channel, dispatch, settled, endpoint, ...options })
    try { await process.#session.ready; return process }
    catch (error) { process.#port.postMessage({ abort: true }); await process.#exited; throw error }
  }
  get greeting() { return this.#session.greeting }
  // What this side found wrong with the session, if that ended it.
  get fault() { return this.#session.fault }
  call(target, method, args) { return this.#session.invoke(target, method, args) }
  callAsync(target, method, args) { return this.#session.invokeAsync(target, method, args) }
  callAnswered(target, method, args, answered) { this.#session.invokeAnswered(target, method, args, answered) }
  callFunctionAsync(fn, args) { return this.#session.callFunctionAsync(fn, args) }
  release(value) { this.#session.release(value) }
  references() { return this.#session.references() }
  drain() { return this.#session.drain() }
  closed() { return this.#exited }
  // End this side: the far end sees the channel end.
  end() { this.#port.postMessage({ end: true }) }
  // Whether the session keeps this process running (by default it does):
  // a test tool lets the process end while it waits for nothing.
  ref() { this.#worker.ref(); this.#port.ref() }
  unref() { this.#worker.unref(); this.#port.unref() }
}
