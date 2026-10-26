#!/usr/bin/env node
// Runs the rutis-host binary of this platform's package, with the Node
// runtime installed next to it (@arcships/rutis-runtime) as the fallback for
// projects that have none of their own.
import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { constants } from 'node:os'
import { dirname } from 'node:path'

const require = createRequire(import.meta.url)
const platform = `${process.platform}-${process.arch}`
let binary
try {
  binary = require.resolve(`@arcships/rutis-host-${platform}/bin/rutis-host${process.platform === 'win32' ? '.exe' : ''}`)
} catch {
  process.stderr.write(`rutis-host: no binary for ${platform} (Linux and macOS on x64 and arm64, and Windows on x64, are supported)\n`)
  process.exit(1)
}
const env = { ...process.env }
if (!env.RUTIS_NODE_RUNTIME) {
  try { env.RUTIS_NODE_RUNTIME = dirname(require.resolve('@arcships/rutis-runtime/package.json')) } catch {}
}
const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit', env })
// A terminal sends Ctrl-C (SIGINT) and its hang-up (SIGHUP) to the whole
// process group, and Windows sends its console events to every process on
// the console: rutis-host gets them itself. Forwarded too, one Ctrl-C would
// reach it twice, and a second signal ends it before its cleanups (#267);
// on Windows, child.kill ends the process outright. Those only keep this
// launcher waiting for rutis-host to stop. What a service manager sends to
// the main process alone, which is this launcher, is forwarded: SIGTERM,
// and SIGUSR1, which makes rutis-host read its configuration again
// (`ExecReload=/bin/kill -USR1 $MAINPID`) and, handled here, does not open
// Node's debugger (#664).
const shared = process.platform === 'win32' ? ['SIGINT', 'SIGBREAK', 'SIGHUP'] : ['SIGINT', 'SIGHUP']
const forwarded = process.platform === 'win32' ? [] : ['SIGTERM', 'SIGUSR1']
for (const signal of shared) process.on(signal, () => {})
for (const signal of forwarded) process.on(signal, () => child.kill(signal))
child.on('exit', (code, signal) => {
  if (signal === 'SIGUSR1') {
    // Ended by it (a command that does not read it): Node's own SIGUSR1
    // would open its debugger instead of ending, so end as a shell says.
    process.exit(128 + constants.signals.SIGUSR1)
  } else if (signal) {
    // End the same way, which a listener would otherwise catch.
    process.removeAllListeners(signal)
    process.kill(process.pid, signal)
  } else process.exit(code ?? 1)
})
