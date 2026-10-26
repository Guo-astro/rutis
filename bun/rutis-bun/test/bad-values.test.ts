// What a call cannot send (#675): a value the far end cannot take, or a
// message over the limit, fails only that call, on the side that would send
// it; the session goes on. The cases of node/rutis-runtime's session tests.
import { expect, test } from 'bun:test'
import { Session, MAX_MESSAGE, overLimit } from '../src/session.ts'
import { decodeError } from '../src/errors.ts'

const PROTOCOL = require('../package.json').rutisProtocol
const data = (value: unknown) => ({ type: 'data', value })
const invoke = (id: number, method: string) => ({ op: 'invoke', id: `rust:${id}`, path: [], target: 'test', method, args: data([]) })
const tick = () => new Promise(resolve => setImmediate(resolve))

function harness(dispatch: (target: string, method: string, args: unknown[]) => unknown, options = {}) {
  const sent: any[] = [], incoming: any[] = []
  let fault: Error | undefined
  const peer = new Session({
    ...options,
    dispatch,
    send: (line: string) => sent.push(JSON.parse(line)),
    pump: () => peer.receive(incoming.shift()),
    abort: (error: Error) => { fault = error },
  })
  peer.receive({ op: 'hello', version: PROTOCOL })
  return { peer, sent, incoming, fault: () => fault }
}

// guarantee: bridge/bad-values
test('a message over the limit fails only its call, either way', async () => {
  expect(MAX_MESSAGE).toBe(16 * 1024 * 1024)
  expect(overLimit(30889096, MAX_MESSAGE)).toBe('the message would be 30889096 bytes, over the limit of 16777216 bytes (16 MiB)')
  const big = 'x'.repeat(2048)
  const { peer, sent, incoming, fault } = harness((_, method) => {
    if (method === 'big') return big
    if (method === 'later') return Promise.resolve(big)
    if (method === 'boom') throw new Error(big)
    return 'small'
  }, { maxMessage: 1024 })
  expect(() => peer.invoke('host:svc\0row', 'take', [() => 1, big])).toThrow(/^argument 2 of svc\.take: the message would be \d+ bytes, over the limit of 1024 bytes$/)
  expect(sent.length).toBe(0)
  expect(peer.references()).toEqual({ exports: 0, imports: 0 })
  peer.receive(invoke(1, 'big'))
  await tick()
  expect(sent.at(-1).error.message).toMatch(/^the result of test\.big: the message would be \d+ bytes, over the limit of 1024 bytes$/)
  peer.receive(invoke(2, 'boom'))
  await tick()
  expect(sent.at(-1).error.message).toMatch(/^the error Error of test\.boom: the message would be \d+ bytes/)
  // A Promise's result, awaited later, names the call that returned it.
  peer.receive(invoke(3, 'later'))
  await tick()
  const future = sent.at(-1).value.value.id
  peer.receive({ op: 'await', id: 'rust:4', path: [], reference: future })
  await tick()
  expect(sent.at(-1).error.message).toMatch(/^the result of test\.later: the message would be/)
  incoming.push({ op: 'return', id: 'node:2', value: data('fine') })
  expect(peer.invoke('t', 'after', [])).toBe('fine')
  expect(fault()).toBeUndefined()
  peer.close()
})

// guarantee: bridge/bad-values
test('an object reference from the far end fails only the call it came in', async () => {
  const ran: string[] = []
  const { peer, sent, incoming, fault } = harness((_, method) => { ran.push(method); return 'ok' })
  const object = { type: 'reference', value: { id: 5, kind: 'object', home: false, origin: [] } }
  const message = 'received an object with methods, which JavaScript cannot take; pass plain data or a function instead'
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'take', args: { type: 'list', value: [data(1), object] } })
  await tick()
  expect(sent.find(frame => frame.op === 'release')).toEqual({ op: 'release', reference: 5, count: 1 })
  const reply = sent.find(frame => frame.op === 'throw')
  expect([reply.id, reply.error.message]).toEqual(['rust:1', message])
  expect(ran).toEqual([])
  incoming.push({ op: 'return', id: 'node:1', value: object })
  expect(() => peer.invoke('t', 'get', [])).toThrow(message)
  peer.receive(invoke(2, 'after'))
  await tick()
  expect(sent.at(-1).value.value).toBe('ok')
  expect(fault()).toBeUndefined()
  peer.close()
})

test('an object with methods goes only to a far end that declared objects', async () => {
  class Counter { n = 0; inc() { return ++this.n } }
  const sent: any[] = []
  const session = new Session({ dispatch: () => new Counter(), send: (line: string) => sent.push(JSON.parse(line)), pump: () => {}, endpoint: { local: 'bun', expected: 'main' } })
  session.start()
  session.receive({ op: 'hello', version: 3, endpoint: 'main', capabilities: ['events-v2'] })
  await session.ready
  session.receive({ op: 'invoke', id: 'main:1', path: [], target: 'svc', method: 'get', args: data([]) })
  await tick()
  expect(sent.at(-1).error.message).toBe('the result of svc.get: an object with methods (Counter) cannot cross to main; pass plain data or a function instead')
  expect(() => session.invoke('host:svc', 'take', [new Counter()])).toThrow('argument 1 of svc.take: an object with methods (Counter) cannot cross to main; pass plain data or a function instead')
  session.close()
})

test('an array with empty slots crosses with undefined in them', async () => {
  // eslint-disable-next-line no-sparse-arrays
  const sparse = () => [1, , 3]
  const { peer, sent, incoming, fault } = harness(sparse)
  peer.receive(invoke(1, 'sparse'))
  await tick()
  const crossed = { type: 'list', value: [data(1), { type: 'undefined' }, data(3)] }
  expect(sent.at(-1).value).toEqual(crossed)
  incoming.push({ op: 'return', id: 'node:1', value: data(null) })
  peer.invoke('t', 'take', [sparse()])
  expect(sent.at(-1).args.value[0]).toEqual(crossed)
  expect(fault()).toBeUndefined()
  peer.close()
})

test('an error graph not in its shape fails only its call', () => {
  const { peer, incoming, fault } = harness(() => {})
  incoming.push({ op: 'throw', id: 'node:1', error: { name: 'ThrownValue', message: 'a string', graph: { root: { type: 'data', value: 'a string' }, nodes: null } } })
  let thrown: any
  try { peer.invoke('t', 'm', []) } catch (error) { thrown = error }
  expect([thrown.name, thrown.message]).toEqual(['ThrownValue', 'a string'])
  // A thrown string relayed with an empty graph is the string again.
  expect(decodeError({ name: 'ThrownValue', message: 'a string', graph: { root: { type: 'data', value: 'a string' }, nodes: [] } })).toBe('a string')
  expect(fault()).toBeUndefined()
  peer.close()
})
