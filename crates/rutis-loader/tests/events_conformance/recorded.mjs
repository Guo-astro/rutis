// A runtime of events_conformance.rs on Windows, where there is no `sh`:
// `node recorded.mjs <pid file> <stderr file> <program> <args...>` starts
// the runtime with this script's standard input and output, its standard
// error appended to <stderr file>, writes the runtime's pid to <pid file>
// (the suite kills that process, as a crash would), and ends as it ends.
import { spawn } from 'node:child_process'
import { openSync, writeFileSync } from 'node:fs'

const [pidFile, stderrFile, program, ...args] = process.argv.slice(2)
const stderr = openSync(stderrFile, 'a')
const child = spawn(program, args, { stdio: ['inherit', 'inherit', stderr], windowsHide: true })
child.on('spawn', () => writeFileSync(pidFile, String(child.pid)))
child.on('error', error => {
  process.stderr.write(`recorded.mjs: ${program}: ${error.message}\n`)
  process.exit(127)
})
child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)))
// The host ending this script ends the runtime too.
for (const signal of ['SIGINT', 'SIGTERM', 'SIGBREAK']) {
  process.on(signal, () => child.kill())
}
