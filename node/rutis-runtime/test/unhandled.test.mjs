// Errors nobody handled (#688, ../src/unhandled.mjs): the helpers, and the
// runtime's own failures, which still end it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { realpathSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { errorText, failureText, reportText, stackFiles, unhandledError } from '../src/unhandled.mjs'
import { encodeError } from '../src/errors.mjs'

const here = realpathSync(fileURLToPath(import.meta.url))

test('a failure says the error and where it was thrown', () => {
  const error = new Error('boom')
  const text = failureText('unhandled rejection', error)
  assert.match(text, /^unhandled rejection: Error: boom \(at .*unhandled\.test\.mjs:\d+:\d+\)$/)
  assert.equal(errorText('need 5 words'), "'need 5 words'")
  assert.equal(failureText('unhandled rejection', 42), 'unhandled rejection: 42')
  // Only the plugin's frames: this file's, not Node's own.
  assert.equal(reportText(error), error.stack.split('\n').filter(line => !line.includes('(node:') && !line.includes('at node:')).join('\n'))
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

// guarantee: loader/unhandled-errors, sdk/error-texts
test('an error left unhandled while loading crosses with its cause and its errors (#677)', () => {
  const inner = new Error('the cause')
  const aggregate = new AggregateError([new Error('one'), new TypeError('two')], 'two failed', { cause: inner })
  const failure = unhandledError('unhandled rejection', aggregate)
  assert.equal(failure.message, 'unhandled rejection: AggregateError: two failed')
  assert.equal(failure.cause, aggregate)
  const { graph } = encodeError(failure)
  const root = graph.nodes[graph.root.value]
  const cause = graph.nodes[root.cause.value]
  assert.equal(cause.name, 'AggregateError')
  assert.deepEqual(cause.errors.map(ref => graph.nodes[ref.value].message), ['one', 'two'])
  assert.equal(graph.nodes[cause.cause.value].message, 'the cause')
  // The failure's own stack is the error's frames once, not its text again.
  assert.equal(root.stack.split('\n').filter(line => line.includes('two failed')).length, 1)
})

// guarantee: loader/unhandled-errors, sdk/error-texts
test('an error from no row lists only the plugin\'s frames, or all of them when none is the plugin\'s (#677)', () => {
  const runtime = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
  const error = new Error('boom')
  error.stack = `Error: boom\n    at boom (${here}:3:7)\n    at listOnTimeout (node:internal/timers:581:17)\n    at loadRow (${runtime}:1:1)`
  assert.equal(reportText(error), `Error: boom\n    at boom (${here}:3:7)`)
  const none = new Error('boom')
  none.stack = `Error: boom\n    at listOnTimeout (node:internal/timers:581:17)\n    at loadRow (${runtime}:1:1)`
  assert.equal(reportText(none), none.stack)
})
