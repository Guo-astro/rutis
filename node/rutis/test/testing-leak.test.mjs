// A test that fails before `unload` leaves nothing on the test tool's
// host: what it loaded is unloaded and its services are withdrawn when it
// ends, so the next test does not fail with `llm is already provided by
// rutis-test` (#933).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const FILE = fileURLToPath(new URL('./fixtures/leak.test.mjs', import.meta.url))

test('what a failed test loaded is unloaded when it ends', { timeout: 60_000 }, async () => {
  const env = { ...process.env }
  delete env.NODE_TEST_CONTEXT
  const child = spawn(process.execPath, ['--test', '--test-reporter=tap', FILE], { env, stdio: ['ignore', 'pipe', 'pipe'] })
  let output = ''
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output += chunk })
  const [code] = await new Promise(resolve => child.once('exit', (...end) => resolve(end)))
  assert.equal(code, 1, output)
  assert.deepEqual(output.match(/^(?:not )?ok \d+ - .*$/gm), [
    'not ok 1 - fails before unload',
    'not ok 2 - fails before the host unloads',
    'ok 3 - loads with the same services',
  ], output)
  assert.doesNotMatch(output, /already provided/)
})
