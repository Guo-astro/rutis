// @ts-nocheck: the same as the Node runtime's (node/rutis-runtime/src/testing/start.mjs).
// A real host and a real runtime for a test (`@arcships/rutis/testing`,
// platform cleanup §7.5): `rutis-host test`, with no rows of its own, starts
// this runtime as a local runtime and loads the driver (./rutis-test.ts) first. The test talks to the driver
// on a session of its own, through a relay thread here, so that either end
// can wait for the other synchronously.
//
//   const harness = await start()
//   await harness.control.callAsync('driver', 'load', [file, config])
//
// `rutis-host` is RUTIS_HOST, else the project's (`@arcships/rutis-host` in
// its node_modules: `bun test` does not put node_modules/.bin on PATH),
// else the one on PATH.
import { spawn } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { Worker } from 'node:worker_threads'
import { Client } from '../client.ts'

const MAIN = fileURLToPath(new URL('../main.ts', import.meta.url))
export const DRIVER = fileURLToPath(new URL('./rutis-test.ts', import.meta.url))

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


// The project's rutis-host, when it has `@arcships/rutis-host`.
function projectHost() {
  const file = join(process.cwd(), 'node_modules', '.bin', process.platform === 'win32' ? 'rutis-host.cmd' : 'rutis-host')
  return existsSync(file) ? file : undefined
}

/**
 * Start `rutis-host test`, which starts this runtime (`command`, then
 * `project`). `dispatch(target, method, args)` answers the driver's calls
 * of the test (its services and listeners). Resolves once the driver
 * greeted, with `control`, the session with it, and `stop()`.
 */
export async function start({ dispatch, command = [process.execPath, '--no-install', '--no-env-file', MAIN], project = process.cwd() }: { dispatch?: (target: string, method: string, args: unknown) => unknown, command?: string[], project?: string } = {}) {
  const host = process.env.RUTIS_HOST || projectHost() || 'rutis-host'
  const dir = mkdtempSync(join(tmpdir(), 'rutis-test-'))
  // A Unix socket, or on Windows a named pipe.
  const address = process.platform === 'win32' ? `\\\\.\\pipe\\rutis-test-${process.pid}-${Date.now()}` : join(dir, 'control.sock')
  const relay = new Worker(RELAY, { eval: true, workerData: { path: address } })
  relay.unref()
  await new Promise((resolve, reject) => { relay.once('message', resolve); relay.once('error', reject) })

  const env = { ...process.env, RUTIS_TEST_CONTROL: address }
  delete env.NODE_TEST_CONTEXT
  const hostProcess = spawn(host, ['test', DRIVER, '--', ...command, project], { env, stdio: ['ignore', 'pipe', 'pipe'] })
  // What the host and the runtime write goes to this process's own output,
  // whole chunks at a time (a test runner may read this process's standard
  // output).
  hostProcess.stdout.on('data', chunk => process.stdout.write(chunk))
  // What it said before the test could start, for the error if it ends.
  let said = []
  hostProcess.stderr.on('data', chunk => {
    said?.push(String(chunk))
    process.stderr.write(chunk)
  })
  const kill = () => { hostProcess.kill() }
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
    hostProcess.once('exit', code => reject(new Error(`${host} exited (${code}) before the test could start${said.length ? `:\n${said.join('').trimEnd()}` : ''}`)))
  })
  failed.catch(() => {})
  let control
  try {
    control = await Promise.race([
      Client.connect(`unix:${address}`, { dispatch: dispatch ?? (() => { throw new Error('the test serves nothing') }), endpoint: { local: 'test' } }),
      failed,
    ])
  } catch (error) {
    stop()
    throw error
  }
  said = undefined
  hostProcess.unref()
  for (const stream of [hostProcess.stdout, hostProcess.stderr]) stream.unref?.()
  return { control, stop }
}
