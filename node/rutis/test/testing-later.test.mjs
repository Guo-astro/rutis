// The result of a function a plugin handed the test comes after a while:
// the test waits for it. While it was out nothing kept the process
// running, so `node --test` cancelled the test ("Promise resolution is
// still pending but the event loop has already resolved"): open-host.md
// §5's example on Linux (#932).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const FILE = fileURLToPath(new URL('./fixtures/later.test.mjs', import.meta.url))

test('a test waits for what a handed-over function returns', { timeout: 60_000 }, async () => {
  const env = { ...process.env }
  delete env.NODE_TEST_CONTEXT
  const child = spawn(process.execPath, ['--test', '--test-reporter=tap', FILE], { env, stdio: ['ignore', 'pipe', 'pipe'] })
  let output = ''
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output += chunk })
  const [code] = await new Promise(resolve => child.once('exit', (...end) => resolve(end)))
  assert.equal(code, 0, output)
  assert.match(output, /^ok 1 - a handed-over function answers later$/m)
})
