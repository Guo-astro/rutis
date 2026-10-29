// What a call cannot send (#675): a value the far end cannot take, or a
// message over the limit, fails only that call, on the side that would send
// it; the session goes on. The cases of node/rutis-runtime's session tests.
import { expect, test } from 'bun:test'
import { Session, MAX_MESSAGE, overLimit, ENDPOINT_PROTOCOL } from '../src/session.ts'
import { decodeError } from '../src/errors.ts'

const data = (value: unknown) => ({ type: 'data', value })
const invoke = (id: number, method: string) => ({ op: 'invoke', id: `main:${id}`, path: [], target: 'test', method, args: data([]) })
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
    endpoint: { local: 'bun' },
  })
  peer.receive({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'main', capabilities: ['objects'] })
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
  peer.receive({ op: 'await', id: 'main:4', path: [], reference: future })
  await tick()
  expect(sent.at(-1).error.message).toMatch(/^test\.later: the result cannot be sent: the message would be/)
  incoming.push({ op: 'return', id: 'bun:2', value: data('fine') })
  expect(peer.invoke('t', 'after', [])).toBe('fine')
  expect(fault()).toBeUndefined()
  peer.close()
})

// guarantee: sdk/error-texts
test('a plugin definition from the far end fails only the call it came in (#677)', async () => {
  const { peer, sent, fault } = harness(() => 'ok')
  const plugin = { type: 'reference', value: { id: 6, kind: 'plugin', home: false, origin: [] } }
  peer.receive({ op: 'invoke', id: 'main:1', path: [], target: 'test', method: 'take', args: { type: 'list', value: [plugin] } })
  await tick()
  const reply = sent.find(frame => frame.op === 'throw')
  expect(reply.error.message).toBe('received a plugin definition made in another process, which cannot be used here; pass plain data or a function instead')
  expect(sent.find(frame => frame.op === 'release')).toEqual({ op: 'release', reference: 6, count: 1 })
  expect(fault()).toBeUndefined()
  peer.close()
})

// objects-case(bun): send-call-state
// objects-case(bun): same-proxy-home
// objects-case(bun): receiver-releases
test('an object from the far end is a proxy whose methods call it (#687)', async () => {
  const util = require('node:util')
  const got: any[] = []
  const { peer, sent, fault } = harness((_, _method, args) => { got.push(...args); return 'ok' })
  const object = { type: 'reference', value: { id: 5, kind: 'object', home: false, origin: [] } }
  peer.receive({ op: 'invoke', id: 'main:1', path: [], target: 'test', method: 'take', args: { type: 'list', value: [object, object] } })
  await tick()
  const [cache, again] = got
  expect(cache).toBe(again)
  expect(String(cache)).toBe('[remote object]')
  expect(util.inspect(cache)).toBe('[remote object]')
  expect(cache.then).toBeUndefined()
  expect(await cache).toBe(cache)
  const result = cache.get('a')
  const call = sent.at(-1)
  expect([call.op, call.reference, call.method, call.args]).toEqual(['call', 5, 'get', { type: 'list', value: [data('a')] }])
  peer.receive({ op: 'return', id: call.id, value: data(1) })
  expect(await result).toBe(1)
  const missing = cache.gett()
  const asked = sent.at(-1)
  expect(asked.method).toBe('gett')
  peer.receive({ op: 'throw', id: asked.id, error: { name: 'TypeError', message: 'Cache has no method gett (methods: get, set)' } })
  await expect(missing).rejects.toThrow('Cache has no method gett (methods: get, set)')
  peer.invokeAsync('t', 'give', [cache]).catch(() => {})
  expect(sent.at(-1).args.value[0].value).toEqual({ id: 5, kind: 'object', home: true, origin: [] })
  peer.release(cache)
  expect(sent.at(-1)).toEqual({ op: 'release', reference: 5, count: 2 })
  await expect(cache.get('a')).rejects.toThrow('reference released')
  expect(fault()).toBeUndefined()
  peer.close()
})

// objects-case(bun): no-method
test('a method an object passed by reference lacks is no-method, saying what it has', async () => {
  class Cache { #m = new Map(); size = 0; get(k: string) { return this.#m.get(k) } set(k: string, v: unknown) { this.#m.set(k, v) } }
  const objects: Record<string, unknown> = { cache: new Cache(), plain: { ping() { return 'pong' } } }
  const { peer, sent, fault } = harness((_, method) => objects[method])
  const ids: Record<string, number> = {}
  for (const [n, name] of [[1, 'cache'], [2, 'plain']] as const) {
    peer.receive(invoke(n, name)); await tick()
    ids[name] = sent.at(-1).value.value.id
  }
  const ask = async (n: number, reference: number, method: string) => {
    peer.receive({ op: 'call', id: `main:${n}`, path: [], reference, method, args: data([]) })
    await tick()
    return sent.find(frame => frame.id === `main:${n}`)
  }
  const cases: [number, number, string, string, unknown][] = [
    [3, ids.cache, 'gett', 'Cache has no method gett (methods: get, set)', { object: 'Cache', method: 'gett', methods: ['get', 'set'] }],
    [4, ids.cache, 'size', 'Cache has no method size (size is a field; objects passed by reference expose only their methods)', { object: 'Cache', method: 'size', methods: ['get', 'set'] }],
    [5, ids.plain, 'pong', 'the object has no method pong (methods: ping)', { object: null, method: 'pong', methods: ['ping'] }],
  ]
  for (const [n, reference, method, message, details] of cases) {
    const reply = await ask(n, reference, method)
    expect(reply.op).toBe('throw')
    expect(reply.error.message).toBe(message)
    const decoded: any = decodeError(reply.error)
    expect(decoded.code).toBe('no-method')
    expect(decoded.data).toEqual(details)
  }
  expect((await ask(6, ids.plain, 'ping')).value).toEqual(data('pong'))
  expect(fault()).toBeUndefined()
  peer.close()
})

// objects-case(bun): undefined-nested-null
test('undefined is null inside a value, and kept as a whole argument', async () => {
  // eslint-disable-next-line no-sparse-arrays
  const sparse = () => [1, , 3]
  const got: any[] = []
  const { peer, sent, incoming, fault } = harness((_, method, args) => { got.push(args); return method === 'sparse' ? sparse() : null })
  peer.receive(invoke(1, 'sparse'))
  await tick()
  const crossed = { type: 'list', value: [data(1), data(null), data(3)] }
  expect(sent.at(-1).value).toEqual(crossed)
  incoming.push({ op: 'return', id: 'bun:1', value: data(null) })
  peer.invoke('t', 'take', [sparse(), undefined])
  expect(sent.at(-1).args.value).toEqual([crossed, { type: 'undefined' }])
  peer.receive({ op: 'invoke', id: 'main:2', path: [], target: 't', method: 'record', args: { type: 'list', value: [{ type: 'undefined' }, { type: 'list', value: [data(1), { type: 'undefined' }] }, { type: 'record', value: { a: { type: 'undefined' } } }] } })
  await tick()
  expect(got.at(-1)).toEqual([undefined, [1, null], { a: null }])
  expect(got.at(-1).length).toBe(3)
  expect(fault()).toBeUndefined()
  peer.close()
})

test('an error graph not in its shape fails only its call', () => {
  const { peer, incoming, fault } = harness(() => {})
  incoming.push({ op: 'throw', id: 'bun:1', error: { name: 'ThrownValue', message: 'a string', graph: { root: { type: 'data', value: 'a string' }, nodes: null } } })
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
  incoming.push({ op: 'throw', id: 'bun:1', error: remote })
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
  peer.receive({ op: 'throw', id: 'bun:2', error: remote })
  const failed: any = await pending.catch((error: unknown) => error)
  expect(failed.stack).toMatch(/^KeyError: 'region'\n\s+at askTheWeather .*bad-values\.test\.ts/)
  const sentNode = encodeError(failed).graph.nodes[0]
  expect(sentNode.stack).toMatch(/weather\.py/)
  expect(sentNode.rethrown.length).toBe(1)
  expect(sentNode.rethrown[0]).toMatch(/^\s+at askTheWeather /)
  peer.close()
})

// objects-case(bun): cancel
// objects-case(bun): holder-killed-releases
// An object passed by reference: a call of its method the caller gives up
// aborts the method's signal; once the host releases it (its holder let go
// or went), it is let go of here.
test('an object passed by reference is cancelled through and released by the host', async () => {
  let aborted: string | undefined
  class Job { run(signal: AbortSignal) { return new Promise(resolve => signal.addEventListener('abort', () => { aborted = (signal.reason as Error)?.name; resolve('stopped') })) } }
  const { peer, sent, fault } = harness(() => new Job())
  peer.receive(invoke(1, 'job')); await tick()
  const id = sent.at(-1).value.value.id
  expect(peer.references().exports).toBe(1)
  peer.receive({ op: 'call', id: 'main:2', path: [], reference: id, method: 'run', args: { type: 'list', value: [{ type: 'signal' }] } })
  await tick()
  peer.receive({ op: 'cancel', id: 'main:2' })
  await tick()
  expect(aborted).toBe('AbortError')
  const before = peer.references().exports
  peer.receive({ op: 'release', reference: id, count: 1 })
  expect(peer.references().exports).toBe(before - 1)
  expect(fault()).toBeUndefined()
  peer.close()
})

// A received object stays a reference wherever it is put: inside a plain
// object, as in an array.
test('a received object nested in data is still a reference', async () => {
  const got: any[] = []
  const { peer, sent, fault } = harness((_, _method, args) => { got.push(...args); return 'ok' })
  const object = { type: 'reference', value: { id: 7, kind: 'object', home: false, origin: [] } }
  peer.receive({ op: 'invoke', id: 'main:1', path: [], target: 'test', method: 'take', args: { type: 'list', value: [object] } })
  await tick()
  peer.invokeAsync('t', 'give', [{ cache: got[0], n: 1 }]).catch(() => {})
  expect(sent.at(-1).args.value[0]).toEqual({ type: 'record', value: { cache: { type: 'reference', value: { id: 7, kind: 'object', home: true, origin: [] } }, n: data(1) } })
  expect(fault()).toBeUndefined()
  peer.close()
})

// A method taken off a received object keeps the object, as a function
// reference would. (Bun's collector scans conservatively: when the object
// is collected is not tested here; Node's test does.)
test('a method taken off a received object keeps it', async () => {
  const collect = async () => { for (let i = 0; i < 5; i++) { Bun.gc(true); await new Promise(resolve => setTimeout(resolve, 10)) } }
  let got: any
  const { peer, sent, fault } = harness((_, _method, args) => { got = args[0]; return 'ok' })
  const take = (id: number) => peer.receive({ op: 'invoke', id: `main:${id}`, path: [], target: 'test', method: 'take', args: { type: 'list', value: [{ type: 'reference', value: { id, kind: 'object', home: false, origin: [] } }] } })
  take(1); await tick()
  const inc: any = got.inc; got = undefined
  await collect()
  expect(sent.some(frame => frame.op === 'release')).toBe(false)
  const result = inc()
  const call = sent.at(-1)
  expect([call.op, call.method]).toEqual(['call', 'inc'])
  peer.receive({ op: 'return', id: call.id, value: data(1) })
  expect(await result).toBe(1)
  expect(fault()).toBeUndefined()
  peer.close()
})
