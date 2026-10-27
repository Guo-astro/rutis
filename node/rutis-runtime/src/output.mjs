// What a runtime's plugins write, and a plugin holding the runtime (#678;
// docs/design-diagnostics-2026-10-26.en.md §6.4, §7).
//
// This file is the same in the Node runtime (node/rutis-runtime) and the Bun
// runtime (bun/rutis-bun).
//
// - Prefixes: with RUTIS_PREFIX_OUTPUT=1 (`rutis-host dev`, or
//   `--prefix-output`), each line the plugins write to stdout or stderr
//   starts with `[<row>] `, the row whose code is running (its async
//   context, the runner's); what no row's code writes (the runtime's code,
//   a callback the runtime cannot trace) with `[<runtime>] `. A child
//   process or a native module writing to the file descriptor itself is not
//   seen here: the host passes its lines on as they are.
// - Blocking: a call of a row's method that keeps this thread for 5 s is
//   said once, on stderr, by a thread of its own (this one is busy): which
//   row's which method, that the other rows wait meanwhile, and what to do.
//   Only said, never stopped; not under a debugger (RUTIS_DEBUG=1), which
//   stops the thread for as long as the person likes.
import { Worker } from 'node:worker_threads'
import { Console } from 'node:console'

// The runtime's own lines (`runtime node: …`) are never prefixed: they
// are written while `owning`.
let owning = false

/** Write `text` to stderr as the runtime's own, without a prefix. */
export function say(text) {
  owning = true
  try { return process.stderr.write(text) } finally { owning = false }
}

/** `text` (or bytes) with `prefix` at the start of each line: the first
 * when `state.atStart`, and after each newline that more text follows. */
export function prefixLines(chunk, prefix, state) {
  if (typeof chunk === 'string') {
    if (chunk === '') return chunk
    const out = (state.atStart ? prefix : '') + chunk.replace(/\n(?=[^])/g, () => `\n${prefix}`)
    state.atStart = chunk.endsWith('\n')
    return out
  }
  const bytes = Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength)
  if (bytes.length === 0) return chunk
  const head = Buffer.from(prefix)
  const parts = []
  let start = 0
  if (state.atStart) parts.push(head)
  for (let index = bytes.indexOf(10); index !== -1 && index < bytes.length - 1; index = bytes.indexOf(10, index + 1)) {
    parts.push(bytes.subarray(start, index + 1), head)
    start = index + 1
  }
  parts.push(bytes.subarray(start))
  state.atStart = bytes[bytes.length - 1] === 10
  return Buffer.concat(parts)
}

/** Prefix each line written to stdout and stderr with `[<name()>] `.
 * `name()`: the row running, or the runtime's name. */
export function prefixOutput(name) {
  for (const stream of [process.stdout, process.stderr]) {
    const write = stream.write
    const state = { atStart: true }
    stream.write = function (chunk, encoding, callback) {
      if (owning || (typeof chunk !== 'string' && !(chunk instanceof Uint8Array))) return Reflect.apply(write, this, arguments)
      if (typeof encoding === 'function') { callback = encoding; encoding = undefined }
      // A string in another encoding (hex, base64) as the bytes it stands for.
      if (typeof chunk === 'string' && encoding && !/^utf-?8$/i.test(encoding)) chunk = Buffer.from(chunk, encoding)
      let prefix
      try { prefix = `[${name()}] ` } catch { prefix = '' }
      const out = prefixLines(chunk, prefix, state)
      return callback ? write.call(this, out, callback) : write.call(this, out)
    }
  }
  // Bun's console writes to the descriptors itself; a console writing
  // through the streams takes its place.
  if (typeof Bun !== 'undefined') {
    const through = new Console({ stdout: process.stdout, stderr: process.stderr })
    for (const method of Object.keys(console)) {
      if (typeof through[method] === 'function') console[method] = through[method].bind(through)
    }
  }
}

/** How long a call may hold the thread before it is said: 5 s, shorter only
 * in this package's tests. */
export function blockingLimit() {
  const test = Number(process.env.RUTIS_TEST_BLOCKING_MS)
  return Number.isFinite(test) && test > 0 ? test : 5000
}

/** The watch of calls holding the thread: `held(row, method, run)` runs
 * `run()` as a call of row `row`'s `method` (`forecast.tomorrow`); one that
 * holds the thread for `limit` ms is said once, on stderr, by a worker
 * (started with the first call). `runtime()`: the runtime's name. Off
 * (`run()` alone) when `quiet`. */
export function watchBlocking({ runtime, limit = blockingLimit(), quiet = process.env.RUTIS_DEBUG === '1' }) {
  if (quiet) return { held: (_row, _method, run) => run() }
  // [generation (odd: a call holds the thread), length of the text]
  const state = new Int32Array(new SharedArrayBuffer(8))
  const text = new Uint8Array(new SharedArrayBuffer(1024))
  const encoder = new TextEncoder()
  let worker
  let depth = 0
  return {
    held(row, method, run) {
      if (!worker) {
        worker = new Worker(WATCH, { eval: true, workerData: { state, text, limit } })
        worker.unref()
        worker.on('error', error => say(`runtime ${runtime()}: the blocking watch failed: ${error?.message ?? error}\n`))
      }
      // The outermost call: one a synchronous call of it waits for is
      // part of it.
      if (depth++ === 0) {
        const { written } = encoder.encodeInto(`${runtime()}\0${row}\0${method}`, text)
        Atomics.store(state, 1, written)
        Atomics.add(state, 0, 1)
        Atomics.notify(state, 0)
      }
      try {
        return run()
      } finally {
        if (--depth === 0) {
          Atomics.add(state, 0, 1)
          Atomics.notify(state, 0)
        }
      }
    },
  }
}

// The watch's thread: it sleeps until a row's call starts holding the
// runtime's thread, and says so once when the call still holds it after
// `limit` ms. The wait's timeout is the clock: the thread it watches may be
// too busy for any other. Evaluated, as the runner's watchdog: a module
// would go through the loaders the runtime runs with (tsx), whose own
// thread and channels a runtime process should not grow mid-session.
const WATCH = `
const { workerData: { state, text, limit } } = require('node:worker_threads')
const { writeSync } = require('node:fs')
${blockingText.toString()}
for (;;) {
  const generation = Atomics.load(state, 0)
  // Even: no call holds the thread.
  if (generation % 2 === 0) { Atomics.wait(state, 0, generation); continue }
  if (Atomics.wait(state, 0, generation, limit) !== 'timed-out') continue
  // The call may have ended just now: then it is not said.
  if (Atomics.load(state, 0) !== generation) continue
  const [runtime, row, method] = Buffer.from(text.slice(0, Atomics.load(state, 1))).toString().split('\\0')
  try { writeSync(2, blockingText(runtime, row, method, limit / 1000) + '\\n') } catch {}
  // Once for the call: the next is watched once it ends.
  Atomics.wait(state, 0, generation)
}
`

/** What is said of row `row`'s `method` holding runtime `runtime` for
 * `seconds`. */
export function blockingText(runtime, row, method, seconds) {
  return `runtime ${runtime}: ${method} of row ${row} has held the runtime for ${seconds} s, and the other rows wait meanwhile: ` +
    `it runs synchronously; await asynchronous calls instead of blocking ones (readFileSync, execSync, a busy loop), ` +
    `or move the work to a worker thread, and it will not hold the runtime`
}
