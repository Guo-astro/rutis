import { expect, test } from 'bun:test'
import { CAPABILITIES } from '../src/session.ts'
import { encode, decode } from '../src/codec.ts'

test('the runtime declares that it is re-entrant', () => {
  expect(CAPABILITIES).toContain('reentrant-sync')
})

// That an unrelated call runs while a synchronous call waits is checked
// against real processes: crates/rutis-bridge/tests/bun_runtime.rs
// (`incoming_calls_run_while_a_synchronous_call_waits`) and
// crates/rutis-loader/tests/bun_multilang.rs.

test('codec escapes newlines and refuses non-finite numbers', () => {
  expect(encode({ text: 'a\nb' })).not.toContain('\n')
  expect(() => encode({ n: Infinity })).toThrow('non-finite')
})

// A compat session takes another session's tagged ids in a reference's
// origin: refusing them ended the session of a nested call (#225).
test('a reference whose origin holds another session\'s tagged ids is taken', () => {
  const { Session } = require('../src/session.ts')
  const PROTOCOL = require('../package.json').rutisProtocol
  const run = (origin: string[]) => {
    const incoming: any[] = []
    let fault: Error | undefined
    const peer = new Session({
      dispatch: () => {}, send: () => {}, abort: (error: Error) => { fault = error },
      pump: () => peer.receive(incoming.shift()),
    })
    peer.receive({ op: 'hello', version: PROTOCOL })
    incoming.push({ op: 'return', id: 'node:1', value: { type: 'reference', value: { id: 1, kind: 'future', home: false, origin } } })
    try { peer.invoke('test', 'later', []) } catch {}
    peer.close()
    return fault?.message
  }
  expect(run(['s1/rust:7', 's1/node:2', 'rust:12', 'node:1'])).toBeUndefined()
  for (const bad of ['py:1', 's1/s2/node:1', 's1/node:0']) expect(run([bad])).toBe('invalid reference')
})
