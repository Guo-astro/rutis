// Errors nobody handled (#688, ../src/unhandled.mjs): the helpers, and the
// runtime's own failures, which still end it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { realpathSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { errorText, failureText, reportText, stackFiles } from '../src/unhandled.mjs'

const here = realpathSync(fileURLToPath(import.meta.url))

test('a failure says the error and where it was thrown', () => {
  const error = new Error('boom')
  const text = failureText('unhandled rejection', error)
  assert.match(text, /^unhandled rejection: Error: boom \(at .*unhandled\.test\.mjs:\d+:\d+\)$/)
  assert.equal(errorText('need 5 words'), "'need 5 words'")
  assert.equal(failureText('unhandled rejection', 42), 'unhandled rejection: 42')
  assert.equal(reportText(error), error.stack)
})

test('the files on a stack are real paths, a module query left out', () => {
  const url = `${pathToFileURL(here).href}?rutis-reload=1-2`
  const files = stackFiles(`Error: x\n    at f (${url}:3:7)\n    at node:internal/timers:1:1\n    at ${here}:9:1`)
  assert.deepEqual([...files], [here])
})

test("the runtime's own failure to connect still ends it with status 1", () => {
  const runner = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
  const anchor = fileURLToPath(new URL('../package.json', import.meta.url))
  const ran = spawnSync(process.execPath, ['--import', 'tsx', runner, '/nonexistent/rutis.sock', anchor], { encoding: 'utf8', timeout: 30_000 })
  assert.equal(ran.status, 1, ran.stderr)
  assert.doesNotMatch(ran.stderr, /unknown row/)
})
