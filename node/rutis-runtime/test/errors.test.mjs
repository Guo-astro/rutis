import { test } from 'node:test'
import assert from 'node:assert/strict'
import { encodeError, decodeError } from '../src/errors.mjs'
const roundtrip = value => decodeError(JSON.parse(JSON.stringify(encodeError(value))))

test('thrown primitives, shared objects and cycles survive error transport', () => {
  for (const value of [undefined, null, false, 0, '', 12n, NaN, Infinity, -Infinity]) {
    assert.ok(Object.is(roundtrip(value), value))
  }
  const shared = { value: 4 }
  const input = [shared, shared]
  shared.parent = input
  const output = roundtrip(input)
  assert.equal(output[0], output[1])
  assert.equal(output[0].parent, output)
})

test('custom error names and object keys do not select inherited constructors or setters', () => {
  const error = new Error('message', { cause: JSON.parse('{"__proto__":{"a":1}}') })
  error.name = 'constructor'
  const output = roundtrip(error)
  assert.ok(output instanceof Error)
  assert.equal(output.name, 'constructor')
  assert.equal(Object.getPrototypeOf(output.cause), Object.prototype)
  assert.deepEqual(output.cause.__proto__, { a: 1 })
  assert.throws(() => roundtrip(new Error('unsupported', { cause: () => {} })), /not supported/)
})

test('a Rust service error decodes with its code and data, and crosses on with them (#440)', () => {
  // As rutis-bridge's `exports::failure` sends it: a bigint inside data.
  const failure = {
    name: 'CalendarError', message: 'no entry 7',
    graph: {
      root: { type: 'reference', value: 0 },
      nodes: [
        { type: 'error', name: 'CalendarError', message: 'no entry 7', code: 'NotFound', data: { type: 'reference', value: 1 } },
        { type: 'object', values: [['NotFound', { type: 'bigint', value: '18446744073709551615' }]] },
      ],
    },
  }
  const error = decodeError(failure)
  assert.ok(error instanceof Error)
  assert.equal(error.name, 'CalendarError')
  assert.equal(error.message, 'no entry 7')
  assert.equal(error.code, 'NotFound')
  assert.deepEqual(error.data, { NotFound: 18446744073709551615n })
  const again = roundtrip(error)
  assert.equal(again.code, 'NotFound')
  assert.deepEqual(again.data, { NotFound: 18446744073709551615n })
  // Data the graph cannot hold is left out; the rest of the error crosses.
  const unsendable = Object.assign(new Error('x'), { code: 'E', data: { f() {} } })
  const sent = roundtrip(unsendable)
  assert.equal(sent.code, 'E')
  assert.equal('data' in sent, false)
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
  assert.match(error.stack, /errors\.test\.mjs/)
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
