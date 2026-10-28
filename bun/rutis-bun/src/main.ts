// `bun --no-install --no-env-file src/main.ts <channel> --id <endpoint>
// [--peer <endpoint>] <project>`: one runtime process, started by a rutis
// host (rutis-bridge's `Launcher::bun`), or a remote one.
//
// The channel is `fd:<n>`, a socket inherited from the process that started
// this one; `unix:<path>` (or a bare path), a socket to dial;
// `tcp:<host>:<port>`, a loopback address to dial, presenting the token in
// RUTIS_CHANNEL_TOKEN first (how processes are started on Windows); or,
// remote (remote.ts), `listen:ws://…` / `listen:wss://…`, a WebSocket
// address to listen on for the controlling rutis (the token it must present
// in RUTIS_TOKEN, a listener certificate and key in RUTIS_CERT and
// RUTIS_KEY), or `dial:ws://…` / `dial:wss://…`, a rutis access point to
// dial and to dial again (dialing.ts), presenting the token in RUTIS_TOKEN
// and trusting the CA in RUTIS_CA besides the default roots. Every
// channel greets as `--id`, expecting `--peer` as the controller when
// given. Plugin modules resolve from <project>.
import { realpathSync, writeSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { Worker } from 'node:worker_threads'
import { Client } from './client.ts'
import { HOST_GONE, reportedStack } from './errors.ts'
import { takeToken } from './channel.ts'
import { dialOut, serve } from './remote.ts'
import { Runtime } from './runner.ts'
import { MANIFEST } from './session.ts'
import { say } from './output.mjs'

const USAGE = 'usage: bun --no-install --no-env-file <rutis-bun>/src/main.ts <channel> --id <endpoint> [--peer <endpoint>] <project>'

function fail(message: string): never {
  say(`runtime bun: ${message}\n`)
  process.exit(2)
}

// The oldest Bun this package is tested with (package.json `engines.bun`).
function checkBun() {
  const minimum = /(\d+)\.(\d+)(?:\.(\d+))?/.exec(MANIFEST.engines?.bun ?? '')
  if (typeof Bun === 'undefined') fail('this runtime runs in Bun (https://bun.com)')
  if (!minimum) return
  const have = Bun.version.split('.').map(Number)
  const need = minimum.slice(1).map(part => Number(part ?? 0))
  for (let i = 0; i < 3; i++) {
    if ((have[i] ?? 0) > need[i]) return
    if ((have[i] ?? 0) < need[i]) fail(`Bun ${Bun.version} is too old: ${MANIFEST.name} ${MANIFEST.version} needs Bun ${MANIFEST.engines.bun}`)
  }
}

checkBun()
// An error nobody handled is reported, and the process goes on: its rows
// share it, and Bun cannot tell which row the error came from (#688,
// docs/design-diagnostics-2026-10-26.en.md §4.7). A plugin calling
// process.exit still ends it.
// The line before says which row called it: the one row whose plugin file
// is on the stack, when there is one.
let current: Runtime | undefined
const runtimeName = () => current?.name ?? 'bun'
// Installed once connected: a failure before that is the runtime's own,
// and ends it.
function reportUnhandled() {
  for (const event of ['uncaughtException', 'unhandledRejection'] as const) {
    process.on(event, (error: any) => {
      const text = typeof error?.stack === 'string' && error.stack ? reportedStack(error.stack) : typeof error === 'string' ? JSON.stringify(error) : Bun.inspect(error)
      say(`runtime ${runtimeName()}: an unhandled error from an unknown row: ${text}\n`)
    })
  }
}
// The source files on a stack, as real paths.
function stackFiles(stack: string | undefined): Set<string> {
  const files = new Set<string>()
  for (const line of String(stack ?? '').split('\n')) {
    const frame = line.trim()
    if (!frame.startsWith('at ')) continue
    let where = /\((.*)\)$/.exec(frame)?.[1] ?? frame.slice(3)
    where = where.replace(/(:\d+){1,2}$/, '').replace(/\?.*$/, '')
    try {
      if (where.startsWith('file:')) where = fileURLToPath(where)
      else if (!where.startsWith('/') && !/^[A-Za-z]:[\\/]/.test(where)) continue
      files.add(realpathSync(where))
    } catch {}
  }
  return files
}
const exit = process.exit
process.exit = function (code?: number) {
  try {
    const row = current?.rowOfFiles(stackFiles(new Error().stack))
    if (row !== undefined) writeSync(2, `runtime ${runtimeName()}: row ${row} called process.exit(${code ?? process.exitCode ?? 0})\n`)
  } catch {}
  return Reflect.apply(exit, process, [code]) as never
} as typeof process.exit
// What was written to stderr, written out: a pipe takes it asynchronously,
// and exiting at once can lose the last lines (a refusal, say).
const flushed = () => new Promise<void>(resolve => { process.stderr.write('', () => resolve()) })
const [channel, ...rest] = process.argv.slice(2)
const flags: Record<string, string> = {}
while (rest[0]?.startsWith('--')) {
  const flag = rest.shift()!.slice(2)
  if (rest.length < 2) fail(USAGE)
  flags[flag] = rest.shift()!
}
if (!channel || rest.length !== 1) fail(USAGE)
const project = resolve(rest[0])
const remote = /^(listen|dial):/.test(channel)
if (remote && !flags.id) fail(`a network channel needs --id <endpoint>: ${channel}`)
if (!flags.id) fail(`a local channel needs --id <endpoint>: ${channel}; a host older than rutis 0.19 started this runtime: upgrade the host`)
if (channel.startsWith('listen:')) {
  try { await serve(channel.slice('listen:'.length), { id: flags.id, peer: flags.peer, project }) } catch (error: any) {
    fail(error.message)
  }
}
if (channel.startsWith('dial:')) {
  const code = await dialOut(channel.slice('dial:'.length), { id: flags.id, peer: flags.peer, project })
  await flushed()
  process.exit(code)
}
// A runner is a runtime: the controller manages its rows.
const endpoint = { local: flags.id, expected: flags.peer, declare: ['runtime'] }
// The session of a dialing runtime: its supervisor reports what ends it.
const dialed = flags.supervisor === 'dial'
const token = takeToken()

const runtime = new Runtime(project)
current = runtime
// The process lives until the session ends. The worker and its port should
// keep it alive, but Bun 1.2 may end a process whose only pending work is
// there (exiting normally, mid-session); a timer keeps it, and leaving
// before the session ended is said.
const alive = setInterval(() => {}, 1 << 30)
// The channel's end, seen by the I/O worker and by the watchdog, which
// ends this process a second after the deadline should a plugin keep it
// from exiting (watchdog.ts).
const channelEnded = new Int32Array(new SharedArrayBuffer(4))
const watchdog = new Worker(new URL('./watchdog.ts', import.meta.url), { workerData: { ended: channelEnded, after: shutdownDeadline() + 1000 } } as any)
watchdog.unref()
let client: Client
try {
  // The logical bus (#464): `events.elsewhere` runs as its frame arrives.
  // A function or object a row hands over runs as that row's (#678).
  client = await Client.connect(channel, { dispatch: runtime.dispatch, token, endpoint, inline: ['events.elsewhere', 'plugins.read'], ended: channelEnded, owner: runtime.running, within: runtime.within })
} catch (error: any) {
  // The supervisor ended the session before it greeted, and said why.
  if (dialed) process.exit(1)
  fail(`cannot connect on ${channel}: ${error.message}`)
}
runtime.attach(client)
reportUnhandled()
if (dialed) {
  // Stopped: the rows go while rutis still hears from this end, then the
  // session ends (and so does this process, below).
  const stop = () => {
    if (runtime.closing) return
    runtime.closing = true
    runtime.dispose().catch(() => {}).then(() => client.end())
  }
  process.on('SIGINT', stop)
  process.on('SIGTERM', stop)
}
let ended = false
process.on('exit', code => {
  if (!ended) say(`runtime bun: exiting (${code}) before the host disconnected\n`)
})
await client.closed()
ended = true
runtime.ended()
clearInterval(alive)
// The host ending the session is the normal end; anything else is said.
if (client.reason && client.reason !== HOST_GONE) {
  say(`runtime bun: disconnected from the host: ${client.reason}\n`)
}
runtime.closing = true
// The rows go now, unless the host disposed them first: it went away (it
// was killed, say) and nothing else ends this process. Cleanups that do not
// finish within the deadline are given up (exit status 1).
const deadline = shutdownDeadline()
setTimeout(() => {
  say(`runtime bun: cleanups did not finish within ${deadline / 1000} s after the host disconnected; exiting\n`)
  process.exit(1)
}, deadline)
await runtime.dispose().catch(error => {
  say(`rutis-bun: unloading failed: ${error?.stack ?? error}\n`)
  process.exitCode = 1
})
// What is still being written goes out first: under the prefix the
// plugins' console writes through the streams, stdout too (#678).
await Promise.all([process.stdout, process.stderr].map(stream => new Promise<void>(resolve => { stream.write('', () => resolve()) })))
// Stray plugin timers and sockets must not keep the process alive: the host
// waits for it to exit.
process.exit(process.exitCode ?? 0)

// How long cleanups may take once the session ended: RUTIS_SHUTDOWN_TIMEOUT
// seconds (rutis-host passes its own deadline on), 10 by default; in ms.
function shutdownDeadline(): number {
  const seconds = Number(process.env.RUTIS_SHUTDOWN_TIMEOUT)
  return Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : 10_000
}
