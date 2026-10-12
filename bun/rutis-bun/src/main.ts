// `bun --no-install --no-env-file src/main.ts <channel> <project>`: one runtime
// process, started by a rutis host (rutis-bridge's `Launcher::bun`).
//
// The channel is `fd:<n>`, a socket inherited from the process that started
// this one; `unix:<path>` (or a bare path), a socket to dial; or
// `tcp:<host>:<port>`, a loopback address to dial, presenting the token in
// RUTIS_CHANNEL_TOKEN first (how processes are started on Windows). Local
// channels speak the compat protocol. Plugin modules resolve from <project>.
import { resolve } from 'node:path'
import { Client } from './client.ts'
import { takeToken } from './channel.ts'
import { Runtime } from './runner.ts'
import { MANIFEST } from './session.ts'

const USAGE = 'usage: bun --no-install --no-env-file <rutis-bun>/src/main.ts <channel> <project>'

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
const args = process.argv.slice(2)
if (args.length !== 2 || args[1].startsWith('--')) fail(USAGE)
const [channel, project] = args
if (channel.startsWith('listen:')) fail(`${channel}: listening as a remote runtime is not supported by this version`)
const token = takeToken()

const runtime = new Runtime(resolve(project))
// The process lives until the session ends. The worker and its port should
// keep it alive, but Bun 1.2 may end a process whose only pending work is
// there (exiting normally, mid-session); a timer keeps it, and leaving
// before the session ended is said.
const alive = setInterval(() => {}, 1 << 30)
let client: Client
try {
  client = await Client.connect(channel, { dispatch: runtime.dispatch, token })
} catch (error: any) {
  fail(`cannot connect on ${channel}: ${error.message}`)
}
runtime.client = client
let ended = false
process.on('exit', code => {
  if (!ended) process.stderr.write(`rutis-bun: exiting (${code}) before the session ended\n`)
})
await client.closed()
ended = true
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
