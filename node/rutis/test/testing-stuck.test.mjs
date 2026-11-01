// A call of the plugin that never returns, and a `load` that a test's
// timeout ends: `node --test` reports and exits (#933), not waiting on what
// the test left in flight, the load among it (#997).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const FILE = fileURLToPath(new URL('./fixtures/stuck.test.mjs', import.meta.url))

// The fixture, its test's time limit `timeout` (ms), run as a test file of
// its own: `{ code, signal, output }`.
async function stuck(timeout) {
  const env = { ...process.env, STUCK_TIMEOUT: String(timeout) }
  delete env.NODE_TEST_CONTEXT
  const child = spawn(process.execPath, ['--test', '--test-reporter=tap', FILE], { env, stdio: ['ignore', 'pipe', 'pipe'], detached: process.platform !== 'win32' })
  let output = ''
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output += chunk })
  // Waiting on what the test left would keep it running: stop it after 30 s,
  // with the file's own test process (its process group).
  const deadline = setTimeout(() => {
    try { process.kill(process.platform === 'win32' ? child.pid : -child.pid, 'SIGKILL') } catch {}
  }, 30_000)
  const [code, signal] = await new Promise(resolve => child.once('exit', (...end) => resolve(end)))
  clearTimeout(deadline)
  return { code, signal, output }
}

// That run, with the timeout of the first test `timeout` and the test after
// it passed, whatever the timeout ended.
function assertEnded(run, timeout) {
  assert.deepEqual({ code: run.code, signal: run.signal }, { code: 1, signal: null }, run.output)
  assert.match(run.output, /^not ok 1 - a call that never returns$/m)
  assert.match(run.output, new RegExp(`error: 'test timed out after ${timeout}ms'`))
  assert.match(run.output, /^# cancelled 1$/m)
  assert.match(run.output, /^ok 2 - a plugin loads after a test a timeout ended$/m)
}

test('node --test exits once a test whose call never returns times out', { timeout: 60_000 }, async () => {
  assertEnded(await stuck(1000), 1000)
})

test('node --test exits once a test times out while the plugin is loading', { timeout: 60_000 }, async () => {
  // 10 ms: the timeout lands in the load whatever the host's speed (100 ms,
  // the issue's, has the host up first on a fast machine).
  assertEnded(await stuck(10), 10)
})

test('node --test exits once a test times out while the host is starting', { timeout: 60_000 }, async () => {
  // The issue's 100 ms (#997): the host was spawned and the test waits for
  // it to greet, the session holding the process meanwhile.
  assertEnded(await stuck(100), 100)
})
