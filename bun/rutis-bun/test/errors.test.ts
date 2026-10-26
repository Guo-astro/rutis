// Errors in the Bun session: what a Rust service's `Err(e)` carries (#440).
import { test } from 'bun:test'
import assert from 'node:assert/strict'
import { decodeError, encodeError } from '../src/errors.ts'

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
  assert.deepEqual(sent, failure)
  // Its cause alone: the same nodes, rooted at the cause.
  const cause = encodeError(decodeError(remote()).cause)
  assert.deepEqual(cause.graph, { root: { type: 'reference', value: 2 }, nodes: failure.graph.nodes })
  // Inside an error of this process: spliced in, references shifted.
  const outer = new Error('outer', { cause: decodeError(remote()) })
  const wrapped = encodeError(outer)
  assert.equal(wrapped.graph.nodes.length, 4)
  assert.equal(wrapped.graph.nodes[0].stack, outer.stack)
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

test('errors made in this process encode as before (#677)', () => {
  const error = Object.assign(new TypeError('local', { cause: 1 }), { code: 'E', data: [1] })
  assert.deepEqual(encodeError(error).graph, {
    root: { type: 'reference', value: 0 },
    nodes: [
      { type: 'error', name: 'TypeError', message: 'local', stack: error.stack, cause: { type: 'data', value: 1 }, code: 'E', data: { type: 'reference', value: 1 } },
      { type: 'array', values: [{ type: 'data', value: 1 }] },
    ],
  })
  // A graph not in its shape decodes to name and message; thrown on, it is
  // encoded from the object.
  const odd = decodeError({ name: 'Odd', message: 'm', graph: { nodes: 'x' } })
  assert.equal(encodeError(odd).graph.nodes[0].stack, odd.stack)
})
