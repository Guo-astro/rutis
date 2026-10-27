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
  expect(() => peer.invoke('host:svc\0row', 'take', [() => 1, big])).toThrow(/^svc\.take: argument 2 cannot be sent: the message would be \d+ bytes, over the limit of 1024 bytes$/)
  expect(sent.length).toBe(0)
  expect(peer.references()).toEqual({ exports: 0, imports: 0 })
  peer.receive(invoke(1, 'big'))
  await tick()
  expect(sent.at(-1).error.message).toMatch(/^test\.big: the result cannot be sent: the message would be \d+ bytes, over the limit of 1024 bytes$/)
  peer.receive(invoke(2, 'boom'))
  await tick()
  expect(sent.at(-1).error.message).toMatch(/^test\.boom: the error Error cannot be sent: the message would be \d+ bytes/)
  // A Promise's result, awaited later, names the call that returned it.
  peer.receive(invoke(3, 'later'))
  await tick()
  const future = sent.at(-1).value.value.id
  peer.receive({ op: 'await', id: 'rust:4', path: [], reference: future })
  await tick()
  expect(sent.at(-1).error.message).toMatch(/^test\.later: the result cannot be sent: the message would be/)
  incoming.push({ op: 'return', id: 'node:2', value: data('fine') })
  expect(peer.invoke('t', 'after', [])).toBe('fine')
  expect(fault()).toBeUndefined()
  peer.close()
})

// guarantee: sdk/error-texts
test('a plugin definition from the far end fails only the call it came in (#677)', async () => {
  const { peer, sent, fault } = harness(() => 'ok')
  const plugin = { type: 'reference', value: { id: 6, kind: 'plugin', home: false, origin: [] } }
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'take', args: { type: 'list', value: [plugin] } })
  await tick()
  const reply = sent.find(frame => frame.op === 'throw')
  expect(reply.error.message).toBe('received a plugin definition made in another process, which cannot be used here; pass plain data or a function instead')
  expect(sent.find(frame => frame.op === 'release')).toEqual({ op: 'release', reference: 6, count: 1 })
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
  expect(sent.at(-1).error.message).toBe('svc.get: the result cannot be sent: an object with methods (Counter) cannot cross to main; pass plain data or a function instead')
  expect(() => session.invoke('host:svc', 'take', [new Counter()])).toThrow('svc.take: argument 1 cannot be sent: an object with methods (Counter) cannot cross to main; pass plain data or a function instead')
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

// A getter run while a value is encoded may throw, again and again, or a
// frozen error: the call fails with a new error naming the argument (or the
// result), the thrown one its cause, left as it was.
test('an error thrown while a value is encoded is wrapped, not changed', async () => {
  const frozen = Object.freeze(new Error('no access'))
  const value = { get secret() { throw frozen } }
  const { peer, sent, fault } = harness(() => value)
  for (let i = 0; i < 2; i++) {
    let thrown: any
    try { peer.invoke('host:svc', 'take', [value]) } catch (error) { thrown = error }
    expect(thrown.message).toBe('svc.take: argument 1 cannot be sent: no access')
    expect(thrown.cause).toBe(frozen)
  }
  expect(frozen.message).toBe('no access')
  expect(sent.length).toBe(0)
  peer.receive(invoke(1, 'get'))
  await tick()
  expect(sent.at(-1).error.message).toBe('test.get: the result cannot be sent: no access')
  expect(frozen.message).toBe('no access')
  expect(fault()).toBeUndefined()
  peer.close()
})

// guarantee: sdk/error-codes
// A value that cannot be sent: code bad-value, data `{ part, argument?, limit? }` (§4.3, #677).
test('a value that cannot be sent carries code bad-value and its part (#677)', async () => {
  const big = 'x'.repeat(2048)
  const { peer, sent } = harness((_, method) => method === 'big' ? big : method === 'boom' ? (() => { throw new Error(big) })() : { f: Symbol('s') }, { maxMessage: 1024 })
  const refusal = (run: () => unknown) => { try { run() } catch (error: any) { return { code: error.code, data: error.data } } }
  expect(refusal(() => peer.invoke('host:svc', 'take', [1, big]))).toEqual({ code: 'bad-value', data: { part: 'argument', argument: 2, limit: 1024 } })
  expect(refusal(() => peer.invoke('host:svc', 'take', [1, { a: [Symbol('s')] }]))).toEqual({ code: 'bad-value', data: { part: 'argument', argument: 2 } })
  const wire = async (id: number, method: string) => { peer.receive(invoke(id, method)); await tick(); return decodeError(sent.at(-1).error) }
  const back = await wire(1, 'big')
  expect([back.code, back.data]).toEqual(['bad-value', { part: 'result', limit: 1024 }])
  const boom = await wire(2, 'boom')
  expect([boom.code, boom.data]).toEqual(['bad-value', { part: 'error', limit: 1024 }])
  const symbol = await wire(3, 'symbol')
  expect([symbol.message, symbol.code, symbol.data]).toEqual(['test.symbol: the result cannot be sent: a symbol cannot cross between processes', 'bad-value', { part: 'result' }])
  peer.close()
})

// guarantee: sdk/error-positions, sdk/error-rethrown
// An error answering a call has the stack of the plugin's code where it made
// the call; thrown on, it adds that place to `rethrown` (§4.4, #677).
test('an error answering a call has the caller\'s stack, and a rethrow adds it (#677)', async () => {
  const { encodeError } = await import('../src/errors.ts')
  const { peer, incoming } = harness(() => {})
  const remote = { name: 'KeyError', message: "'region'", graph: { root: { type: 'reference', value: 0 }, nodes: [{ type: 'error', name: 'KeyError', message: "'region'", stack: 'KeyError: \'region\'\n  File "weather.py", line 31, in tomorrow' }] } }
  incoming.push({ op: 'throw', id: 'node:1', error: remote })
  // Not in tail position: JavaScriptCore drops a tail call's frame (and
  // Bun's transpiler turns `const a = f(); return a` into one).
  const asked: string[] = []
  function askTheWeatherSync() { const answer = peer.invoke('host:weather', 'tomorrow', []); asked.push('sync'); return answer }
  let sync: any
  try { askTheWeatherSync() } catch (error) { sync = error }
  expect(sync.stack).toMatch(/^KeyError: 'region'\n\s+at askTheWeatherSync .*bad-values\.test\.ts/)
  expect(sync.stack).not.toMatch(/src[\\/]session\.ts|weather\.py/)
  function askTheWeather() { const answer = peer.invokeAsync('host:weather', 'tomorrow', []); asked.push('async'); return answer }
  const pending = askTheWeather()
  peer.receive({ op: 'throw', id: 'node:2', error: remote })
  const failed: any = await pending.catch((error: unknown) => error)
  expect(failed.stack).toMatch(/^KeyError: 'region'\n\s+at askTheWeather .*bad-values\.test\.ts/)
  const sentNode = encodeError(failed).graph.nodes[0]
  expect(sentNode.stack).toMatch(/weather\.py/)
  expect(sentNode.rethrown.length).toBe(1)
  expect(sentNode.rethrown[0]).toMatch(/^\s+at askTheWeather /)
  peer.close()
})
