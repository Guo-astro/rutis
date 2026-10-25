// `bun --no-install --no-env-file src/main.ts <channel> [--id <endpoint>]
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
// and trusting the CA in RUTIS_CA besides the default roots. Local channels
// speak the compat protocol unless `--format endpoint`; remote ones the
// endpoint format, as `--id`, expecting `--peer` as the controller when
// given. Plugin modules resolve from <project>.
import { resolve } from 'node:path'
import { Client } from './client.ts'
import { takeToken } from './channel.ts'
import { dialOut, serve } from './remote.ts'
import { Runtime } from './runner.ts'
import { MANIFEST } from './session.ts'
import { EVENTS_V2, NO_EVENTS_V2 } from './events.mjs'

const USAGE = 'usage: bun --no-install --no-env-file <rutis-bun>/src/main.ts <channel> [--id <endpoint>] [--peer <endpoint>] <project>'

function fail(message: string): never {
  process.stderr.write(`rutis-bun: ${message}\n`)
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
// An uncaught error ends the process, and with it every service of this
// runtime (boundary rule 8): no plugin runs on in a state nobody handled.
// Bun alone reports some (a timer's throw) and carries on.
for (const event of ['uncaughtException', 'unhandledRejection'] as const) {
  process.on(event, (error: any) => {
    process.stderr.write(`rutis-bun: ${event === 'uncaughtException' ? 'uncaught error' : 'unhandled rejection'}: ${error?.stack ?? error}\n`)
    process.exit(1)
  })
}
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
if ((remote || flags.format === 'endpoint') && !flags.id) fail(`a network channel needs --id <endpoint>: ${channel}`)
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
const endpoint = flags.format === 'endpoint' ? { local: flags.id, expected: flags.peer, declare: ['runtime'] } : undefined
// The session of a dialing runtime: its supervisor reports what ends it.
const dialed = flags.supervisor === 'dial'
const token = takeToken()

const runtime = new Runtime(project)
// The process lives until the session ends. The worker and its port should
// keep it alive, but Bun 1.2 may end a process whose only pending work is
// there (exiting normally, mid-session); a timer keeps it, and leaving
// before the session ended is said.
const alive = setInterval(() => {}, 1 << 30)
let client: Client
try {
  // The logical bus (#464): `events.elsewhere` runs as its frame arrives;
  // a host older than 0.13 (no events-v2) is refused (protocol §2.2).
  client = await Client.connect(channel, { dispatch: runtime.dispatch, token, endpoint, inline: ['events.elsewhere', 'plugins.read'], requires: [EVENTS_V2] })
} catch (error: any) {
  if (error?.message === NO_EVENTS_V2) {
    process.stderr.write(`rutis: refused: incompatible (${NO_EVENTS_V2}); stopping\n`)
    await flushed()
    process.exit(1)
  }
  // The supervisor ended the session before it greeted, and said why.
  if (dialed) process.exit(1)
  fail(`cannot connect on ${channel}: ${error.message}`)
}
runtime.attach(client)
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
  if (!ended) process.stderr.write(`rutis-bun: exiting (${code}) before the session ended\n`)
})
await client.closed()
ended = true
runtime.ended()
clearInterval(alive)
// The host ending the session is the normal end; anything else is said.
if (client.reason && client.reason !== 'the rutis host disconnected') {
  process.stderr.write(`rutis-bun: the session ended: ${client.reason}\n`)
}
runtime.closing = true
// The rows go now, unless the host disposed them first: it went away (it
// was killed, say) and nothing else ends this process. Cleanups that do not
// finish within the deadline are given up (exit status 1).
const deadline = shutdownDeadline()
setTimeout(() => {
  process.stderr.write(`rutis-bun: cleanups did not finish within ${deadline / 1000} s after the session ended; exiting\n`)
  process.exit(1)
}, deadline)
await runtime.dispose()
// Stray plugin timers and sockets must not keep the process alive: the host
// waits for it to exit.
process.exit(0)

// How long cleanups may take once the session ended: RUTIS_SHUTDOWN_TIMEOUT
// seconds (rutis-host passes its own deadline on), 10 by default; in ms.
function shutdownDeadline(): number {
  const seconds = Number(process.env.RUTIS_SHUTDOWN_TIMEOUT)
  return Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : 10_000
}
