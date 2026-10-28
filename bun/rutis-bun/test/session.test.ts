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

// A session takes another session's tagged ids in a reference's origin:
// refusing them ended the session of a nested call (#225).
test('a reference whose origin holds another session\'s tagged ids is taken', () => {
  const { Session, ENDPOINT_PROTOCOL } = require('../src/session.ts')
  const run = (origin: string[]) => {
    const incoming: any[] = []
    let fault: Error | undefined
    const peer = new Session({
      dispatch: () => {}, send: () => {}, abort: (error: Error) => { fault = error },
      pump: () => peer.receive(incoming.shift()), endpoint: { local: 'bun' },
    })
    peer.receive({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'main', capabilities: [] })
    incoming.push({ op: 'return', id: 'bun:1', value: { type: 'reference', value: { id: 1, kind: 'future', home: false, origin } } })
    try { peer.invoke('test', 'later', []) } catch {}
    peer.close()
    return fault?.message
  }
  expect(run(['s1/main:7', 's1/bun:2', 'main:12', 'bun:1'])).toBeUndefined()
  for (const bad of ['Bun:1', 's1/s2/bun:1', 's1/bun:0']) expect(run([bad])).toBe('invalid reference')
})

// Risk P8: a far end that greets with a protocol version this side does
// not speak is refused at the handshake, and the refusal says which end to
// upgrade (standard Q6.2.4): this side, naming its package, when the far
// end is newer; the far end, naming its implementation when it said, when
// it is older. The wording is rutis-bridge's
// (`session::rpc::version_mismatch`).
// guarantee: bridge/version-refusal
test('a far end of another protocol version is refused, saying which end to upgrade', async () => {
  const { Session, ENDPOINT_PROTOCOL } = require('../src/session.ts')
  expect(ENDPOINT_PROTOCOL).toBe(4)
  const cases = [
    [99, undefined, 'upgrade this side (npm @arcships/rutis-bun)'],
    // rutis 0.18 speaks protocol 3.
    [3, { name: 'rutis-host', version: '0.18.0' }, 'upgrade the far end (rutis-host 0.18.0)'],
    [3, undefined, 'upgrade the far end'],
  ] as const
  for (const [version, implementation, hint] of cases) {
    const sent: any[] = []
    let fault: Error | undefined
    const session = new Session({
      dispatch: () => {}, send: (line: string) => sent.push(JSON.parse(line)), pump: () => {},
      abort: (error: Error) => { fault = error }, endpoint: { local: 'bun' },
    })
    session.start()
    expect(sent[0].version).toBe(ENDPOINT_PROTOCOL)
    session.receive({ op: 'hello', version, endpoint: 'main', implementation, capabilities: [] })
    const message = `incompatible session: the far end speaks protocol ${version}, this side ${ENDPOINT_PROTOCOL}: ${hint}`
    const error = await session.ready.then(() => undefined, (error: any) => error)
    expect(error?.category).toBe('incompatible')
    expect(error?.message).toBe(message)
    expect(fault?.message).toBe(message)
  }
})

// Events protocol §5.5: an emit's reply is taken as its frame is read,
// before the next frame is handled, not on a later tick.
test('invokeAnswered tells the reply as its frame is read, before the next frame', () => {
  const { Session, ENDPOINT_PROTOCOL } = require('../src/session.ts')
  const sent: any[] = [], seen: any[] = []
  const peer = new Session({
    dispatch: (_: string, method: string) => { seen.push(method); return null },
    send: (line: string) => sent.push(JSON.parse(line)), abort: () => {}, pump: () => {},
    inline: ['events.elsewhere'], endpoint: { local: 'bun' },
  })
  peer.receive({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'main', capabilities: [] })
  peer.invokeAnswered('', 'events.dispatch', [{ mode: 'emit' }], (result: any) => seen.push(['answered', result.ok]))
  peer.receive({ op: 'return', id: sent.at(-1).id, value: { type: 'undefined' } })
  expect(seen).toEqual([['answered', true]])
  peer.receive({ op: 'invoke', id: 'main:1', path: [], target: '', method: 'events.elsewhere', args: { type: 'data', value: [{}] } })
  expect(seen).toEqual([['answered', true], 'events.elsewhere'])
  peer.invokeAnswered('', 'events.dispatch', [{ mode: 'emit' }], (result: any) => seen.push(['failed', result.ok, result.value.name]))
  peer.receive({ op: 'throw', id: sent.at(-1).id, error: { name: 'BindingError', message: 'refused' } })
  expect(seen.at(-1)).toEqual(['failed', false, 'BindingError'])
  peer.close()
})
