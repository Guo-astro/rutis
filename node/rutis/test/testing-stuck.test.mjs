// A call of the plugin that never returns: node:test's timeout fails the
// test, and `node --test` reports and exits (#933), not waiting on the call.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const FILE = fileURLToPath(new URL('./fixtures/stuck.test.mjs', import.meta.url))

test('node --test exits once a test whose call never returns times out', { timeout: 60_000 }, async () => {
  const env = { ...process.env }
  delete env.NODE_TEST_CONTEXT
  const child = spawn(process.execPath, ['--test', '--test-reporter=tap', FILE], { env, stdio: ['ignore', 'pipe', 'pipe'], detached: process.platform !== 'win32' })
  let output = ''
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output += chunk })
  // Waiting on the call would keep it running: stop it after 30 s, with
  // the file's own test process (its process group).
  const deadline = setTimeout(() => {
    try { process.kill(process.platform === 'win32' ? child.pid : -child.pid, 'SIGKILL') } catch {}
  }, 30_000)
  const [code, signal] = await new Promise(resolve => child.once('exit', (...end) => resolve(end)))
  clearTimeout(deadline)
  assert.deepEqual({ code, signal }, { code: 1, signal: null }, output)
  assert.match(output, /^not ok 1 - a call that never returns$/m)
  assert.match(output, /error: 'test timed out after 1000ms'/)
  assert.match(output, /^# cancelled 1$/m)
})
