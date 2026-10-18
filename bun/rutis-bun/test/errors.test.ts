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
