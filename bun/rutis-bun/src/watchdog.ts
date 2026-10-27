// The watchdog of a runtime process, on a thread of its own (#682): once
// the channel ended (the I/O worker sets `ended`), the process unloads its
// rows and exits within the shutdown deadline. A plugin keeping the main
// thread busy keeps it from both, and from even seeing that the session
// ended: `after` that, counted from when the I/O worker saw the end, the
// watchdog ends the process, so it never outlives its host. It sleeps
// until then (the wait's timeout is the clock); process.exit ends it as
// any other thread.
import { workerData } from 'node:worker_threads'
import { writeSync } from 'node:fs'

const { ended, after } = workerData as { ended: Int32Array, after: number }
Atomics.wait(ended, 0, 0)
Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, after)
try {
  writeSync(2, `runtime bun: the host disconnected ${after / 1000} s ago and this process has not exited (a plugin keeps it busy); ending it\n`)
} catch {}
process.kill(process.pid, 'SIGKILL')
