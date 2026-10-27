// The blocking watch's thread (./output.mjs `watchBlocking`): it sleeps
// until a row's call starts holding the runtime's thread, and says so once
// when the call still holds it after `limit` ms. The wait's timeout is the
// clock: the thread it watches may be too busy for any other.
//
// This file is the same in the Node runtime (node/rutis-runtime) and the Bun
// runtime (bun/rutis-bun).
import { workerData } from 'node:worker_threads'
import { writeSync } from 'node:fs'
import { blockingText } from './output.mjs'

const { state, text, limit } = workerData
for (;;) {
  const generation = Atomics.load(state, 0)
  // Even: no call holds the thread.
  if (generation % 2 === 0) { Atomics.wait(state, 0, generation); continue }
  if (Atomics.wait(state, 0, generation, limit) !== 'timed-out') continue
  const [runtime, row, method] = Buffer.from(text.slice(0, Atomics.load(state, 1))).toString().split('\0')
  try { writeSync(2, `${blockingText(runtime, row, method, limit / 1000)}\n`) } catch {}
  // Once for the call: the next is watched once it ends.
  Atomics.wait(state, 0, generation)
}
