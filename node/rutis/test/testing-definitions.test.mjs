// A test that ends before unloading a definition row (`load(definition)`)
// leaves nothing on the test tool's host: the next test loads the same
// definition as if nothing had been left (#1004).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const FILE = fileURLToPath(new URL('./fixtures/definitions-leak.test.mjs', import.meta.url))

test('what a test loaded is unloaded when it ends, definitions included', { timeout: 60_000 }, async () => {
  const env = { ...process.env }
  delete env.NODE_TEST_CONTEXT
  const child = spawn(process.execPath, ['--test', '--test-reporter=tap', FILE], { env, stdio: ['ignore', 'pipe', 'pipe'] })
  let output = ''
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output += chunk })
  const [code] = await new Promise(resolve => child.once('exit', (...end) => resolve(end)))
  assert.equal(code, 1, output)
  assert.deepEqual(output.match(/^(?:not )?ok \d+ - .*$/gm), [
    'not ok 1 - leaves its definition row loaded',
    'ok 2 - loads the same definition again',
  ], output)
  assert.doesNotMatch(output, /already provided/)
})
