// A real host and a real runtime for a test (`@arcships/rutis/testing`,
// platform cleanup §7.5): `rutis-host test` with no rows of its own, this
// runtime in a process of its own, on the two ends of one socket pair, and
// the driver (./rutis-test.mjs) as its first row. The test talks to the driver
// on a session of its own, through a relay thread here, so that either end
// can wait for the other synchronously.
//
//   const harness = await start()
//   await harness.control.callAsync('driver', 'load', [file, config])
//
// `rutis-host` is RUTIS_HOST, else the one on PATH.
import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { Worker } from 'node:worker_threads'
import { Process } from '../client.mjs'

const RUNNER = fileURLToPath(new URL('../runner.mjs', import.meta.url))
export const DRIVER = fileURLToPath(new URL('./rutis-test.mjs', import.meta.url))

// Accepts the test's and the driver's connections and joins them.
const RELAY = `
const { parentPort, workerData } = require('node:worker_threads')
const net = require('node:net')
const sockets = []
const server = net.createServer(socket => {
  sockets.push(socket)
  if (sockets.length < 2) return
  server.close()
  const [a, b] = sockets
  const end = () => { a.destroy(); b.destroy() }
  for (const socket of sockets) { socket.on('close', end); socket.on('error', end) }
  a.pipe(b); b.pipe(a)
})
server.listen(workerData.path, () => parentPort.postMessage('listening'))
`

// What this process was started with that the runtime needs too (a
// TypeScript loader such as `--import tsx`), without the test runner's own.
const execArgv = () => process.execArgv.filter(arg => !arg.startsWith('--test'))

/**
 * Start `rutis-host test` and this runtime. `dispatch(target, method,
 * args)` answers the driver's calls of the test (its services and
 * listeners). Resolves once the driver greeted, with `control`, the session
 * with it, and `stop()`.
 */
export async function start({ dispatch, id = 'node', command = [process.execPath, ...execArgv(), RUNNER] } = {}) {
  const host = process.env.RUTIS_HOST || 'rutis-host'
  const dir = mkdtempSync(join(tmpdir(), 'rutis-test-'))
  const address = join(dir, 'control.sock')
  const relay = new Worker(RELAY, { eval: true, workerData: { path: address } })
  relay.unref()
  await new Promise((resolve, reject) => { relay.once('message', resolve); relay.once('error', reject) })

  const env = { ...process.env, RUTIS_TEST_CONTROL: address }
  delete env.NODE_TEST_CONTEXT
  const [executable, ...args] = command
  // The runtime's channel is its standard input, a socket pair (`fd:0`);
  // the host has the other end as its standard input and output.
  const runtime = spawn(executable, [...args, 'fd:0', '--id', id, DRIVER], { env, stdio: ['pipe', 'pipe', 'pipe'] })
  const hostProcess = spawn(host, ['test', DRIVER], { env, stdio: [runtime.stdin, runtime.stdin, 'pipe'] })
  runtime.stdin.destroy()
  // What they write goes to this process's own output, whole chunks at a
  // time (a test runner may read this process's standard output).
  runtime.stdout.on('data', chunk => process.stdout.write(chunk))
  const said = []
  for (const stream of [runtime.stderr, hostProcess.stderr]) {
    stream.on('data', chunk => {
      said.push(String(chunk))
      process.stderr.write(chunk)
    })
  }
  const children = [runtime, hostProcess]
  const kill = () => { for (const child of children) child.kill() }
  process.once('exit', kill)
  const stop = () => {
    process.removeListener('exit', kill)
    kill()
    rmSync(dir, { recursive: true, force: true })
  }

  const failed = new Promise((_, reject) => {
    hostProcess.once('error', error => {
      reject(new Error(error.code === 'ENOENT'
        ? `${host} not found: install @arcships/rutis-host (a dev dependency) or set RUTIS_HOST to a rutis-host`
        : `${host}: ${error.message}`))
    })
    for (const [child, what] of [[hostProcess, host], [runtime, 'the runtime']]) {
      child.once('exit', code => reject(new Error(`${what} exited (${code}) before the test could start${said.length ? `:\n${said.join('').trimEnd()}` : ''}`)))
    }
  })
  failed.catch(() => {})
  let control
  try {
    control = await Promise.race([
      Process.connect(`unix:${address}`, dispatch ?? (() => { throw new Error('the test serves nothing') }), undefined, { local: 'test' }),
      failed,
    ])
  } catch (error) {
    stop()
    throw error
  }
  for (const child of children) {
    child.unref()
    for (const stream of [child.stdout, child.stderr]) stream?.unref?.()
  }
  return { control, stop }
}
