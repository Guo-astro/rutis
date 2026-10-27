// Errors in the Bun session: what a Rust service's `Err(e)` carries (#440).
import { test } from 'bun:test'
import assert from 'node:assert/strict'
import { decodeError, encodeError, hostGone, noMethod, notDeclared, unavailable } from '../src/errors.ts'
import { fileURLToPath } from 'node:url'

const roundtrip = (value: unknown) => decodeError(JSON.parse(JSON.stringify(encodeError(value))))
// A stack as it crosses: the runtime's own frames left out.
const withoutRuntime = (stack: string) => stack.split('\n').filter(line => !line.includes(fileURLToPath(new URL('../src/', import.meta.url))) && !line.includes('(node:')).join('\n')

test('a Rust service error decodes with its code and data, and crosses on with them (#440)', () => {
  const error = decodeError({
    name: 'CalendarError', message: 'no entry 7',
    graph: {
      root: { type: 'reference', value: 0 },
      nodes: [
        { type: 'error', name: 'CalendarError', message: 'no entry 7', code: 'NotFound', data: { type: 'reference', value: 1 } },
        { type: 'object', values: [['NotFound', { type: 'bigint', value: '18446744073709551615' }]] },
      ],
    },
  })
  assert.equal(error.code, 'NotFound')
  assert.deepEqual(error.data, { NotFound: 18446744073709551615n })
  const again = decodeError(JSON.parse(JSON.stringify(encodeError(error))))
  assert.equal(again.code, 'NotFound')
  assert.deepEqual(again.data, { NotFound: 18446744073709551615n })
})
// As the other side sends it (§4.4): its own stack, a code and data, a site
// on the root, and a cause.
const remote = () => ({
  name: 'CalendarError', message: 'no entry 7',
  graph: {
    root: { type: 'reference', value: 0 },
    nodes: [
      { type: 'error', name: 'CalendarError', message: 'no entry 7', stack: 'CalendarError: no entry 7\n    at far (remote.go:12)', code: 'NotFound', data: { type: 'reference', value: 1 }, cause: { type: 'reference', value: 2 }, site: { plugin: 'calendar', doing: 'calling get' } },
      { type: 'object', values: [['id', { type: 'bigint', value: '7' }]] },
      { type: 'error', name: 'TypeError', message: 'bad id', stack: 'TypeError: bad id\n    at deeper (remote.go:3)' },
    ],
  },
})

test('an error from the other side has this process\'s stack, not the other side\'s (#677)', () => {
  const error = decodeError(remote())
  assert.equal(error.name, 'CalendarError')
  assert.equal(error.message, 'no entry 7')
  assert.equal(error.code, 'NotFound')
  assert.deepEqual(error.data, { id: 7n })
  assert.equal(error.cause.message, 'bad id')
  assert.equal(typeof error.stack, 'string')
  assert.doesNotMatch(error.stack, /remote\.go/)
  assert.doesNotMatch(error.cause.stack, /remote\.go/)
  assert.match(error.stack, /errors\.test\.ts/)
  assert.doesNotMatch(error.stack, /decodeGraph/)
  // Nothing new to see on it.
  assert.deepEqual(Object.keys(error).sort(), ['code', 'data', 'name'])
  assert.deepEqual(Reflect.ownKeys(error).filter(key => typeof key === 'symbol'), [])
  assert.equal('site' in error, false)
})

test('an error from the other side thrown on crosses with the graph it came in (#677)', () => {
  const failure = remote()
  const sent = JSON.parse(JSON.stringify(encodeError(decodeError(failure))))
  assert.deepEqual(withoutRethrown(sent), failure)
  // Its cause alone: the same nodes, rooted at the cause.
  const cause = encodeError(decodeError(remote()).cause)
  assert.deepEqual(withoutRethrown(cause).graph, { root: { type: 'reference', value: 2 }, nodes: failure.graph.nodes })
  // Inside an error of this process: spliced in, references shifted.
  const outer = new Error('outer', { cause: decodeError(remote()) })
  const wrapped = encodeError(outer)
  assert.equal(wrapped.graph.nodes.length, 4)
  assert.equal(wrapped.graph.nodes[0].stack, withoutRuntime(outer.stack!))
  assert.deepEqual(wrapped.graph.nodes[0].cause, { type: 'reference', value: 1 })
  const spliced = wrapped.graph.nodes[1]
  assert.equal(spliced.stack, failure.graph.nodes[0].stack)
  assert.deepEqual(spliced.site, failure.graph.nodes[0].site)
  assert.equal(spliced.code, 'NotFound')
  assert.deepEqual(spliced.data, { type: 'reference', value: 2 })
  assert.deepEqual(spliced.cause, { type: 'reference', value: 3 })
  assert.deepEqual(wrapped.graph.nodes[3], failure.graph.nodes[2])
  const back = decodeError(JSON.parse(JSON.stringify(wrapped)))
  assert.equal(back.cause.code, 'NotFound')
  assert.deepEqual(back.cause.data, { id: 7n })
  assert.equal(back.cause.cause.message, 'bad id')
})

// The encoded error with `rethrown` left out of its nodes.
const withoutRethrown = ({ graph, ...rest }) => ({ ...rest, graph: { ...graph, nodes: graph.nodes.map(({ rethrown, ...node }) => node) } })
const frameLines = text => text.split('\n').every(line => /^\s*at /.test(line))

test('an error from the other side thrown on adds where it was thrown on to its root (#677)', () => {
  const failure = remote()
  const error = decodeError(failure)
  const once = encodeError(error)
  const root = once.graph.nodes[0]
  assert.equal(root.rethrown.length, 1)
  assert.match(root.rethrown[0], /errors\.test\.ts/)
  assert.ok(frameLines(root.rethrown[0]))
  assert.doesNotMatch(root.rethrown[0], /CalendarError/)
  // Only the root; the graph kept is not changed, so encoding again adds one again.
  assert.equal('rethrown' in once.graph.nodes[2], false)
  assert.equal('rethrown' in failure.graph.nodes[0], false)
  assert.equal(encodeError(error).graph.nodes[0].rethrown.length, 1)
  // Received and thrown on again: a second item, after the first.
  const again = decodeError(JSON.parse(JSON.stringify(once)))
  const twice = encodeError(again).graph.nodes[0].rethrown
  assert.equal(twice.length, 2)
  assert.equal(twice[0], root.rethrown[0])
  assert.match(twice[1], /errors\.test\.ts/)
  // At 8 items nothing more is added.
  const full = remote()
  full.graph.nodes[0].rethrown = Array.from({ length: 8 }, (_, i) => `    at f${i} (x.js:1)`)
  assert.deepEqual(encodeError(decodeError(full)).graph.nodes[0].rethrown, full.graph.nodes[0].rethrown)
  // No frames, nothing added.
  const bare = decodeError(remote())
  Object.defineProperty(bare, 'stack', { value: 'CalendarError: no entry 7', configurable: true, writable: true })
  assert.equal('rethrown' in encodeError(bare).graph.nodes[0], false)
})

test('errors made in this process encode as before (#677)', () => {
  const error = Object.assign(new TypeError('local', { cause: 1 }), { code: 'E', data: [1] })
  assert.deepEqual(encodeError(error).graph, {
    root: { type: 'reference', value: 0 },
    nodes: [
      { type: 'error', name: 'TypeError', message: 'local', stack: withoutRuntime(error.stack!), cause: { type: 'data', value: 1 }, code: 'E', data: { type: 'reference', value: 1 } },
      { type: 'array', values: [{ type: 'data', value: 1 }] },
    ],
  })
  // A graph not in its shape decodes to name and message; thrown on, it is
  // encoded from the object.
  const odd = decodeError({ name: 'Odd', message: 'm', graph: { nodes: 'x' } })
  assert.equal(encodeError(odd).graph.nodes[0].stack, withoutRuntime(odd.stack))
})

// guarantee: sdk/error-texts
// A thrown value that is not an error crosses as ThrownValue, its message
// the value as written, which is what is printed (§8.2) (#677).
test('a thrown value that is not an error has the value as its message (#677)', () => {
  const cases: [unknown, string][] = [['need 5 words', '"need 5 words"'], [{ code: 3 }, '{code: 3}'], [[1, 2], '[1, 2]'], [42, '42'], [null, 'null'], [undefined, 'undefined'], [12n, '12n'], [{ 'a-b': { c: { d: 1 } } }, '{"a-b": {c: {…}}}']]
  for (const [value, message] of cases) {
    const sent = encodeError(value)
    assert.deepEqual([sent.name, sent.message], ['ThrownValue', message])
    assert.deepEqual(roundtrip(value), value)
  }
})

// guarantee: sdk/error-texts
// The stack that crosses has the plugin's frames (and its dependencies'),
// not this runtime's or Cordis's (§4.5).
test('the stack that crosses leaves out the runtime\'s frames (#677)', () => {
  const runtime = fileURLToPath(new URL('../src/runner.ts', import.meta.url))
  const error = new Error('no region')
  error.stack = [
    'Error: no region',
    '    at apply (/work/weather/src/index.ts:12:9)',
    '    at helper (/work/weather/node_modules/left-pad/index.js:3:1)',
    `    at load (${runtime}:300:20)`,
    '    at execute (/work/weather/node_modules/@deepseek-ai/cordis/lib/index.js:900:7)',
  ].join('\n')
  assert.equal(encodeError(error).graph.nodes[0].stack, [
    'Error: no region',
    '    at apply (/work/weather/src/index.ts:12:9)',
    '    at helper (/work/weather/node_modules/left-pad/index.js:3:1)',
  ].join('\n'))
})

// guarantee: sdk/error-codes
test('the errors this runtime makes have their code, data and text (#677)', () => {
  const pick = (error: any) => ({ name: error.name, message: error.message, code: error.code, data: error.data })
  assert.deepEqual(pick(noMethod('weather', 'tomorow', ['tomorrow', 'today'])), { name: 'BindingError', message: 'weather has no method tomorow (methods: today, tomorrow)', code: 'no-method', data: { service: 'weather', method: 'tomorow', methods: ['today', 'tomorrow'] } })
  assert.equal(noMethod('weather', 'x', []).message, 'weather has no method x (no methods)')
  assert.deepEqual(pick(notDeclared('llm')), { name: 'BindingError', message: "service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject", code: 'not-declared', data: { service: 'llm' } })
  assert.deepEqual(pick(unavailable('llm')), { name: 'BindingError', message: 'service llm is not available', code: 'unavailable', data: { service: 'llm' } })
  assert.deepEqual(pick(hostGone('the host disconnected')), { name: 'BindingError', message: 'the host disconnected', code: 'peer-gone', data: {} })
  assert.deepEqual(pick(hostGone('connection reset')), { name: 'BindingError', message: 'the host disconnected', code: 'peer-gone', data: { reason: 'connection reset' } })
  assert.deepEqual(pick(roundtrip(noMethod('weather', 'x', ['a']))), pick(noMethod('weather', 'x', ['a'])))
})

// guarantee: sdk/error-texts
// A module that throws a value that is not an error at import: the value
// alone, as written (#677).
test('a thrown value at import is shown alone (#677)', async () => {
  const { cannotLoad } = await import('../src/errors.ts')
  const error = cannotLoad('need 5 words', '/work/m/str.mjs', '/work')
  assert.deepEqual([error.name, error.message], ['LoadError', 'cannot load m/str.mjs: "need 5 words"'])
  assert.equal(cannotLoad({ code: 3 }, '/work/m/obj.mjs', '/work').message, 'cannot load m/obj.mjs: {code: 3}')
})
