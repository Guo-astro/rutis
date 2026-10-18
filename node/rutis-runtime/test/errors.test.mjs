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
