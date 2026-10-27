import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { Session, ENDPOINT_PROTOCOL, MAX_MESSAGE, overLimit } from '../src/session.mjs'
import { decode } from '../src/codec.mjs'
import { decodeError } from '../src/errors.mjs'

const PROTOCOL = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).rutisProtocol
const data = value => ({ type: 'data', value })
function harness(dispatch, options = {}) {
  const sent = [], incoming = []
  let fault
  const peer = new Session({
    ...options,
    dispatch,
    send: line => sent.push(JSON.parse(line)),
    pump: () => { assert.ok(incoming.length, 'sync caller must not strand'); peer.receive(incoming.shift()) },
    abort: error => { fault = error },
  })
  peer.receive({ op: 'hello', version: PROTOCOL })
  return { peer, sent, incoming, fault: () => fault }
}
const invoke = (id, method, path = []) => ({ op: 'invoke', id: `rust:${id}`, path, target: 'test', method, args: data([]) })

test('admitted callback survives a following counted release before dispatch', async () => {
  let calls = 0
  const callback = () => ++calls
  const { peer, sent, fault } = harness(() => [callback, callback])
  peer.receive(invoke(1, 'get'))
  await Promise.resolve()
  const [a, b] = sent[0].value.value
  assert.equal(a.value.id, b.value.id)
  peer.receive({ op: 'call', id: 'rust:2', path: [], reference: a.value.id, args: data([]) })
  peer.receive({ op: 'release', reference: a.value.id, count: 2 })
  assert.equal(calls, 0)
  await Promise.resolve()
  assert.equal(calls, 1)
  assert.equal(sent[1].value.value, 1)
  assert.equal(fault(), undefined)
  peer.close()
})

test('old release preserves a new grant and identifiers are not reused', async () => {
  const callback = () => 42
  const { peer, sent, fault } = harness(() => callback)
  peer.receive(invoke(1, 'get')); await Promise.resolve()
  peer.receive(invoke(2, 'get')); await Promise.resolve()
  const id = sent[0].value.value.id
  assert.equal(sent[1].value.value.id, id)
  peer.receive({ op: 'release', reference: id, count: 1 })
  peer.receive({ op: 'call', id: 'rust:3', path: [], reference: id, args: data([]) })
  await Promise.resolve()
  assert.equal(sent[2].value.value, 42)
  peer.receive({ op: 'release', reference: id, count: 1 })
  peer.receive(invoke(4, 'get')); await Promise.resolve()
  assert.notEqual(sent[3].value.value.id, id)
  assert.equal(fault(), undefined)
  peer.close()
})

test('sync wait only pumps related calls and admits a prequeued related callback once', async () => {
  const seen = []
  const { peer, sent, incoming, fault } = harness((_, method) => { seen.push(method); return 42 })
  peer.receive(invoke(1, 'queued', ['node:1']))
  incoming.push(invoke(2, 'unrelated'), invoke(3, 'related', ['node:1']), { op: 'return', id: 'node:1', value: data(7) })
  assert.equal(peer.invoke('test', 'call', []), 7)
  assert.deepEqual(seen, ['queued', 'related'])
  await Promise.resolve()
  assert.deepEqual(seen, ['queued', 'related', 'unrelated'])
  assert.equal(sent.filter(frame => frame.id === 'rust:1').length, 1)
  assert.equal(fault(), undefined)
  peer.close()
})

// Events protocol §5.5: an emit's reply is taken as its frame is read,
// before the next frame is handled, not on a later tick.
test('invokeAnswered tells the reply as its frame is read, before the next frame', async () => {
  const seen = []
  const { peer, sent, fault } = harness((_, method) => { seen.push(method); return null }, { inline: ['events.elsewhere'] })
  peer.invokeAnswered('', 'events.dispatch', [{ mode: 'emit' }], result => seen.push(['answered', result.ok, result.value]))
  assert.equal(sent.at(-1).method, 'events.dispatch')
  const id = sent.at(-1).id
  peer.receive({ op: 'return', id, value: { type: 'undefined' } })
  assert.deepEqual(seen, [['answered', true, undefined]], 'in the same call, no tick in between')
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: '', method: 'events.elsewhere', args: data([{}]) })
  assert.deepEqual(seen, [['answered', true, undefined], 'events.elsewhere'])
  // A throw is told the same way.
  peer.invokeAnswered('', 'events.dispatch', [{ mode: 'emit' }], result => seen.push(['failed', result.ok, result.value.name]))
  peer.receive({ op: 'throw', id: sent.at(-1).id, error: { name: 'BindingError', message: 'refused' } })
  assert.deepEqual(seen.at(-1), ['failed', false, 'BindingError'])
  assert.equal(fault(), undefined)
  peer.close()
})

// Not reentrant (multi-language design §9): an emit-mode listener call
// that arrives while a synchronous call waits, not of its chain, waits for
// it to return, so it never sees the caller's state half updated and never
// makes a call the host's wait-cycle check cannot see (the call it makes
// goes out after the wait).
test('an emit-mode listener call waits while an unrelated synchronous call does', async () => {
  const log = []
  let state = 'idle'
  const holds = (method, args) => method === 'events.call' && args?.[0]?.mode === 'emit' ? args[0].name : undefined
  const { peer, incoming, sent, fault } = harness((_, method) => {
    if (method === 'x') {
      state = 'half-updated'
      log.push(`x got ${peer.invoke('b', 'slow', [])}`)
      state = 'done'
      return 'x-result'
    }
    if (method === 'events.call') log.push(`listener sees ${state}`)
    return undefined
  }, { inline: ['events.elsewhere'], holds })
  incoming.push(
    { op: 'invoke', id: 'rust:2', path: [], target: '', method: 'events.call', args: data([{ listener: '1', mode: 'emit', name: 'n', args: [] }]) },
    { op: 'return', id: 'node:1', value: data('slow') },
  )
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'x', args: data([]), sync: true })
  await Promise.resolve()
  assert.deepEqual(log, ['x got slow'])
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(log, ['x got slow', 'listener sees done'])
  assert.deepEqual(sent.filter(frame => frame.op === 'return').map(frame => frame.id), ['rust:1', 'rust:2'])
  assert.equal(fault(), undefined)
  peer.close()
})

// §5.5: a later emit of a name does not start ahead of an earlier emit of
// it that is held by a synchronous wait, even when its path is on that
// wait's chain.
test('a related later emit waits behind the earlier held emit of its name', async () => {
  const log = []
  const holds = (method, args) => method === 'events.call' && args?.[0]?.mode === 'emit' ? args[0].name : undefined
  const { peer, incoming, fault } = harness((_, method, args) => {
    if (method === 'events.call') log.push(`${args[0].name}:${args[0].args[0]}`)
    return undefined
  }, { holds })
  const call = (id, name, n, path = []) => ({
    op: 'invoke', id: `rust:${id}`, path, target: '', method: 'events.call',
    args: data([{ listener: '1', mode: 'emit', name, args: [n] }]),
  })
  incoming.push(
    call(1, 'n', 1),
    call(2, 'n', 2, ['node:1']),
    { op: 'return', id: 'node:1', value: data(7) },
  )
  assert.equal(peer.invoke('test', 'sync', []), 7)
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(log, ['n:1', 'n:2'])
  assert.equal(fault(), undefined)
  peer.close()
})

// §5.5: a local emit of a name goes behind the held listener calls of an
// earlier emit of it, in frame order; with none held it runs at once.
test('behind runs local work after the held calls of its key, in frame order', async () => {
  const log = []
  const holds = (method, args) => method === 'events.call' && args?.[0]?.mode === 'emit' ? args[0].name : undefined
  const { peer, incoming, fault } = harness((_, method, args) => { log.push(`call ${args[0].name}:${args[0].args[0]}`); return undefined }, { holds })
  const call = (id, name, n) => ({ op: 'invoke', id: `rust:${id}`, path: [], target: '', method: 'events.call', args: data([{ listener: '1', mode: 'emit', name, args: [n] }]) })
  assert.equal(peer.behind('n', () => log.push('local n:0')), false, 'nothing held: the caller runs it')
  incoming.push(call(1, 'n', 1), call(2, 'm', 1), { op: 'return', id: 'node:1', value: data(7) })
  assert.equal(peer.invoke('test', 'sync', []), 7)
  // Right after the wait, before the held calls ran: behind them.
  assert.equal(peer.behind('n', () => log.push('local n:2')), true)
  assert.equal(peer.behind('m', () => log.push('local m:2')), true)
  assert.equal(peer.behind('other', () => {}), false)
  assert.deepEqual(log, [])
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(log, ['call n:1', 'call m:1', 'local n:2', 'local m:2'])
  // Behind a queued local run too.
  incoming.push(call(3, 'n', 3), { op: 'return', id: 'node:2', value: data(8) })
  assert.equal(peer.invoke('test', 'sync', []), 8)
  assert.equal(peer.behind('n', () => log.push('local n:4')), true)
  assert.equal(peer.behind('n', () => log.push('local n:5')), true)
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(log.slice(4), ['call n:3', 'local n:4', 'local n:5'])
  assert.equal(fault(), undefined)
  peer.close()
})

// §5.5 / #546: a local emit queued behind a held call is lost when the
// session ends; close says which ones instead of dropping them silently.
test('close reports local emits still queued behind a held call', async () => {
  const log = []
  const reports = []
  const holds = (method, args) => method === 'events.call' && args?.[0]?.mode === 'emit' ? args[0].name : undefined
  const { peer, incoming, fault } = harness((_, method, args) => {
    if (method === 'events.call') log.push(`call ${args[0].name}`)
  }, { holds, report: (line, details) => reports.push({ line, details }) })
  const call = (id, name) => ({
    op: 'invoke', id: `rust:${id}`, path: [], target: '', method: 'events.call',
    args: data([{ listener: '1', mode: 'emit', name, args: [] }]),
  })
  // The listener call is held while the unrelated synchronous call waits.
  incoming.push(call(1, 't/a'), { op: 'return', id: 'node:1', value: data(7) })
  assert.equal(peer.invoke('test', 'sync', []), 7)
  // The held call has not run yet; a local emit of the name goes behind it.
  assert.equal(peer.behind('t/a', () => log.push('local t/a')), true)
  peer.close()
  // Neither the held call nor the local emit runs; the loss is reported once.
  assert.deepEqual(log, [])
  assert.equal(reports.length, 1)
  assert.equal(reports[0].line, 'rutis: the connection to the host ended with 1 local emit still waiting behind an earlier one: t/a')
  assert.deepEqual(reports[0].details, { count: 1, events: ['t/a'] })
  peer.close()
  assert.equal(reports.length, 1, 'close is idempotent')
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(log, [])
  assert.equal(fault(), undefined)
})

test('close needs no report callback for local emits still queued', async () => {
  const holds = (method, args) => method === 'events.call' && args?.[0]?.mode === 'emit' ? args[0].name : undefined
  const { peer, incoming } = harness(() => undefined, { holds })
  incoming.push(
    { op: 'invoke', id: 'rust:1', path: [], target: '', method: 'events.call', args: data([{ listener: '1', mode: 'emit', name: 't/a', args: [] }]) },
    { op: 'return', id: 'node:1', value: data(7) },
  )
  assert.equal(peer.invoke('test', 'sync', []), 7)
  assert.equal(peer.behind('t/a', () => { throw new Error('the local emit must not run') }), true)
  assert.doesNotThrow(() => { peer.close(); peer.close() })
  await new Promise(resolve => setImmediate(resolve))
})

test('explicit release accounts repeated live imports without relying on GC', async () => {
  const { peer, sent, incoming } = harness(() => {})
  const ref = { type: 'reference', value: { id: 1, kind: 'function', home: false, origin: [] } }
  incoming.push({ op: 'return', id: 'node:1', value: { type: 'list', value: [ref, ref] } })
  const [a, b] = peer.invoke('test', 'get', [])
  assert.equal(a, b)
  peer.release(a)
  assert.deepEqual(sent.at(-1), { op: 'release', reference: 1, count: 2 })
  assert.throws(() => b(), /released/)
  peer.close()
})

test('a reference whose origin holds another session\'s tagged ids is taken (#225)', async () => {
  const { peer, incoming, fault } = harness(() => {})
  const origin = ['s1/rust:7', 's1/node:2', 'rust:12', 'node:1']
  incoming.push({ op: 'return', id: 'node:1', value: { type: 'reference', value: { id: 1, kind: 'future', home: false, origin } } })
  assert.ok(peer.invoke('test', 'later', []) instanceof Promise)
  assert.equal(fault(), undefined)
  // Untagged ids of another kind, and doubly tagged ones, are still refused.
  for (const bad of ['py:1', 's1/s2/node:1', 's1/node:0']) {
    const other = harness(() => {})
    other.incoming.push({ op: 'return', id: 'node:1', value: { type: 'reference', value: { id: 1, kind: 'future', home: false, origin: [bad] } } })
    assert.throws(() => other.peer.invoke('test', 'later', []), /invalid reference/, bad)
    assert.match(other.fault()?.message ?? '', /invalid reference/, bad)
  }
  peer.close()
})

test('unsupported nested references fail explicitly and roll back partial grants', async () => {
  const callback = () => 42
  const { peer, sent, incoming, fault } = harness(() => {})
  // An object with methods crosses as an object reference; a symbol cannot cross.
  assert.throws(() => peer.invoke('test', 'bad', [callback, { nested: Symbol('x') }]), { message: 'test.bad: argument 2 cannot be sent: a symbol cannot cross between processes' })
  // Failed encoding consumes the invocation id, but publishes no reference.
  incoming.push({ op: 'return', id: 'node:2', value: data(7) })
  assert.equal(peer.invoke('test', 'good', []), 7)
  assert.equal(sent.length, 1)
  assert.equal(fault(), undefined)
  peer.close()
})

// Risk P8: a far end that greets with a protocol version this side does
// not speak is refused at the handshake, not after loading, and the refusal
// names both versions and which end to upgrade (standard Q6.2.4): this side,
// naming its package, when the far end is newer; the far end, naming its
// implementation when it said, when it is older. The wording is
// rutis-bridge's (`session::rpc::version_mismatch`).
// guarantee: bridge/version-refusal
test('a far end of another protocol version is refused, saying which end to upgrade', async () => {
  const far = { name: 'rutis', version: '0.1.0' }
  for (const [endpoint, ours] of [[{ local: 'node' }, ENDPOINT_PROTOCOL], [undefined, PROTOCOL]]) {
    const cases = [
      [99, undefined, `upgrade this side (npm @arcships/rutis-runtime)`],
      [1, endpoint && far, endpoint ? 'upgrade the far end (rutis 0.1.0)' : 'upgrade the far end'],
    ]
    for (const [version, implementation, hint] of cases) {
      const sent = []
      let fault
      const session = new Session({ dispatch: () => {}, send: line => sent.push(JSON.parse(line)), pump: () => {}, abort: error => { fault = error }, endpoint })
      session.start()
      assert.equal(sent[0].version, ours, 'this side greets first, with its own version')
      session.receive({ op: 'hello', version, ...(endpoint ? { endpoint: 'main', implementation, capabilities: [] } : {}) })
      const message = `incompatible session: the far end speaks protocol ${version}, this side ${ours}: ${hint}`
      await assert.rejects(session.ready, error => error.category === 'incompatible' && error.message === message, message)
      assert.equal(fault?.message, message, 'the channel is closed')
      await assert.rejects(session.invokeAsync('t', 'm', []), undefined, 'and the session with it')
    }
  }
})

test('objects with behaviour cross as live object references', async () => {
  class Account {
    #balance = 5
    get balance() { return this.#balance }
    deposit(amount) { this.#balance += amount; return this.#balance }
  }
  const account = new Account()
  const plain = { id: 'a', tags: ['x'] }
  const { peer, sent, fault } = harness((target, method) => method === 'open' ? { account, plain, again: account } : undefined)
  peer.receive(invoke(1, 'open'))
  await new Promise(resolve => setImmediate(resolve))
  const reply = sent.find(frame => frame.op === 'return')
  // A plain object holding a live one is a record; data stays data; the same
  // object is one reference however often it appears.
  assert.equal(reply.value.type, 'record')
  assert.deepEqual(reply.value.value.plain, data(plain))
  const { id, kind, home } = reply.value.value.account.value
  assert.deepEqual([kind, home], ['object', false])
  assert.equal(reply.value.value.again.value.id, id)
  // Property reads and method calls reach the original object.
  peer.receive({ op: 'get', id: 'rust:2', path: [], reference: id, property: 'balance' })
  peer.receive({ op: 'call', id: 'rust:3', path: [], reference: id, method: 'deposit', args: { type: 'list', value: [data(2)] } })
  await new Promise(resolve => setImmediate(resolve))
  const results = sent.filter(frame => frame.op === 'return').map(frame => frame.value)
  assert.deepEqual(results.slice(1), [data(5), data(7)])
  assert.equal(account.balance, 7)
  assert.equal(fault(), undefined)
  peer.close()
})

// objects-case(node): no-objects-declared
test('an object reference goes only to a far end that declared objects', async () => {
  for (const [capabilities, expected] of [[['signals'], 'throw'], [['objects'], 'return']]) {
    const sent = []
    const session = new Session({
      dispatch: () => ({ today() { return 'monday' } }),
      send: line => sent.push(JSON.parse(line)),
      pump: () => {},
      endpoint: { local: 'node', expected: 'main' },
    })
    session.start()
    session.receive({ op: 'hello', version: 3, endpoint: 'main', capabilities })
    await session.ready
    session.receive({ op: 'invoke', id: 'main:1', path: [], target: 'svc', method: 'get', args: data([]) })
    await Promise.resolve()
    const reply = sent.at(-1)
    assert.equal(reply.op, expected, JSON.stringify(reply))
    if (expected === 'throw') assert.equal(reply.error.message, 'svc.get: the result cannot be sent: an object with methods cannot cross to main; pass plain data or a function instead, or upgrade main to rutis 0.18')
    else assert.equal(reply.value.value.kind, 'object')
    session.close()
  }
})

// Q6.2.3, risk P3: a frame of the wrong shape, or naming a call or reference
// this side does not have, ends the session; the cases of rutis-bridge's
// `malformed_and_dangling_frames_close_the_session`.
test('malformed and dangling frames close the session', async () => {
  // Text that is not JSON never reaches the session: the codec refuses it,
  // and the I/O worker closes the channel.
  assert.throws(() => decode('{"op":"invoke","id":"rust:1"'), SyntaxError)
  const cases = {
    'not an object': 42,
    'unknown op': { op: 'frobnicate', id: 'rust:1' },
    'wrong field type': { op: 'invoke', id: 'rust:1', path: [], target: 7, method: 'm', args: { type: 'undefined' } },
    'missing field': { op: 'invoke', id: 'rust:1', path: [], target: 't', args: { type: 'undefined' } },
    'unknown wire value': { op: 'invoke', id: 'rust:1', path: [], target: 't', method: 'm', args: { type: 'bogus' } },
    'call of an unknown reference': { op: 'call', id: 'rust:1', path: [], reference: 99, args: { type: 'undefined' } },
    'await of an unknown reference': { op: 'await', id: 'rust:1', path: [], reference: 99 },
    'release of an unknown reference': { op: 'release', reference: 99, count: 1 },
    'reply to an unknown call': { op: 'return', id: 'node:99', value: { type: 'undefined' } },
    'cancel without an id': { op: 'cancel', id: 7 },
  }
  for (const [name, frame] of Object.entries(cases)) {
    const { peer, fault } = harness(() => 'served')
    peer.receive(frame)
    assert.ok(fault() instanceof Error, name)
    await assert.rejects(peer.invokeAsync('t', 'm', []), undefined, name)
  }
  // A cancel may cross the reply of the call it cancels: one for a call
  // this side does not have is ignored.
  const { peer, sent, fault } = harness(() => 'served')
  peer.receive({ op: 'cancel', id: 'rust:7' })
  peer.receive(invoke(1, 'm'))
  await Promise.resolve()
  assert.equal(fault(), undefined)
  assert.equal(sent.at(-1).value.value, 'served')
  peer.close()
})

// ── What a call cannot send (#675) ───────────────────────────────────

// guarantee: bridge/bad-values
// A call or a result whose message would be over the limit (16 MiB; here a
// smaller one) fails on this side before it is sent, naming the call, the
// argument (the largest) or the result, the size and the limit; references
// it would grant are given back and the session goes on.
test('a message over the limit fails only its call, either way', async () => {
  assert.equal(MAX_MESSAGE, 16 * 1024 * 1024)
  assert.equal(overLimit(30889096, MAX_MESSAGE), 'the message would be 30889096 bytes, over the limit of 16777216 bytes (16 MiB)')
  const big = 'x'.repeat(2048)
  const { peer, sent, incoming, fault } = harness((_, method) => {
    if (method === 'big') return big
    if (method === 'later') return Promise.resolve(big)
    if (method === 'boom') throw new Error(big)
    return 'small'
  }, { maxMessage: 1024 })
  assert.throws(() => peer.invoke('host:svc\0row', 'take', [() => 1, big]), {
    name: 'RangeError', message: /^svc\.take: argument 2 cannot be sent: the message would be \d+ bytes, over the limit of 1024 bytes$/,
  })
  assert.equal(sent.length, 0, 'nothing was sent')
  assert.deepEqual(peer.references(), { exports: 0, imports: 0 }, 'the callback is not left granted')
  peer.receive(invoke(1, 'big'))
  await Promise.resolve()
  assert.match(sent.at(-1).error.message, /^test\.big: the result cannot be sent: the message would be \d+ bytes, over the limit of 1024 bytes$/)
  peer.receive(invoke(2, 'boom'))
  await Promise.resolve()
  assert.match(sent.at(-1).error.message, /^test\.boom: the error Error cannot be sent: the message would be \d+ bytes/)
  // A Promise's result, awaited later, names the call that returned it.
  peer.receive(invoke(3, 'later'))
  await Promise.resolve()
  const future = sent.at(-1).value.value.id
  peer.receive({ op: 'await', id: 'rust:4', path: [], reference: future })
  await new Promise(resolve => setImmediate(resolve))
  assert.match(sent.at(-1).error.message, /^test\.later: the result cannot be sent: the message would be/)
  incoming.push({ op: 'return', id: 'node:2', value: data('fine') })
  assert.equal(peer.invoke('t', 'after', []), 'fine')
  assert.equal(fault(), undefined)
  peer.close()
})

// guarantee: bridge/bad-values
// An object reference the far end sends (an older host relaying one) is
// refused as a value this side does not take: only the call it came in
// fails, and it is released; the session goes on.
// objects-case(node): send-call-state
// objects-case(node): same-proxy-home
// objects-case(node): receiver-releases
test('an object from the far end is a proxy whose methods call it (#687)', async () => {
  const util = await import('node:util')
  const got = []
  const { peer, sent, fault } = harness((_, method, args) => { got.push(...args); return 'ok' })
  const object = { type: 'reference', value: { id: 5, kind: 'object', home: false, origin: [] } }
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'take', args: { type: 'list', value: [object, object] } })
  await Promise.resolve()
  const [cache, again] = got
  // Passed twice, it is one proxy.
  assert.equal(cache, again)
  assert.equal(String(cache), '[remote object]')
  assert.equal(util.inspect(cache), '[remote object]')
  assert.equal(cache.then, undefined)
  assert.equal(await cache, cache, 'awaiting a proxy gives the proxy')
  // A method is an asynchronous call of the original object.
  const result = cache.get('a')
  const call = sent.at(-1)
  assert.deepEqual([call.op, call.reference, call.method, call.args], ['call', 5, 'get', { type: 'list', value: [data('a')] }])
  peer.receive({ op: 'return', id: call.id, value: data(1) })
  assert.equal(await result, 1)
  // A missing method is the owner's to answer: its error, as it sent it.
  const missing = cache.gett()
  const asked = sent.at(-1)
  assert.equal(asked.method, 'gett')
  peer.receive({ op: 'throw', id: asked.id, error: { name: 'TypeError', message: 'Cache has no method gett (methods: get, set)' } })
  await assert.rejects(missing, { message: 'Cache has no method gett (methods: get, set)' })
  // Passed back, it is the far end's own again.
  peer.invokeAsync('t', 'give', [cache]).catch(() => {}) // only its frame is read
  assert.deepEqual(sent.at(-1).args.value[0].value, { id: 5, kind: 'object', home: true, origin: [] })
  // Let go of, it is released.
  peer.release(cache)
  assert.deepEqual(sent.at(-1), { op: 'release', reference: 5, count: 2 })
  await assert.rejects(cache.get('a'), { message: 'reference released' })
  assert.equal(fault(), undefined)
  peer.close()
})

// objects-case(node): no-method
test('a method an object passed by reference lacks is no-method, saying what it has', async () => {
  class Cache { #m = new Map(); size = 0; get(k) { return this.#m.get(k) } set(k, v) { this.#m.set(k, v) } }
  const objects = { cache: new Cache(), plain: { ping() { return 'pong' } } }
  const { peer, sent, fault } = harness((_, method) => objects[method])
  const ids = {}
  for (const [n, name] of [[1, 'cache'], [2, 'plain']]) {
    peer.receive(invoke(n, name)); await Promise.resolve()
    ids[name] = sent.at(-1).value.value.id
  }
  const ask = async (n, reference, method) => {
    peer.receive({ op: 'call', id: `rust:${n}`, path: [], reference, method, args: data([]) })
    await new Promise(resolve => setImmediate(resolve))
    return sent.find(frame => frame.id === `rust:${n}`)
  }
  const cases = [
    [3, ids.cache, 'gett', 'Cache has no method gett (methods: get, set)', { object: 'Cache', method: 'gett', methods: ['get', 'set'] }],
    [4, ids.cache, 'size', 'Cache has no method size (size is a field; objects passed by reference expose only their methods)', { object: 'Cache', method: 'size', methods: ['get', 'set'] }],
    [5, ids.plain, 'pong', 'the object has no method pong (methods: ping)', { object: null, method: 'pong', methods: ['ping'] }],
  ]
  for (const [n, reference, method, message, details] of cases) {
    const reply = await ask(n, reference, method)
    assert.equal(reply.op, 'throw')
    assert.equal(reply.error.message, message)
    const decoded = decodeError(reply.error)
    assert.equal(decoded.code, 'no-method')
    assert.deepEqual(decoded.data, details)
  }
  // The session goes on: the next call is answered.
  const ok = await ask(6, ids.plain, 'ping')
  assert.deepEqual(ok.value, data('pong'))
  assert.equal(fault(), undefined)
  peer.close()
})

// guarantee: sdk/error-texts
// A plugin definition made in another process fails only the call it came
// in, in words without the platform's names (#677).
test('a plugin definition from the far end fails only the call it came in', async () => {
  const { peer, sent, fault } = harness(() => 'ok')
  const plugin = { type: 'reference', value: { id: 6, kind: 'plugin', home: false, origin: [] } }
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'take', args: { type: 'list', value: [plugin] } })
  await Promise.resolve()
  const reply = sent.find(frame => frame.op === 'throw')
  assert.equal(reply.error.message, 'received a plugin definition made in another process, which cannot be used here; pass plain data or a function instead')
  assert.deepEqual(sent.find(frame => frame.op === 'release'), { op: 'release', reference: 6, count: 1 })
  assert.equal(fault(), undefined)
  peer.close()
})

// objects-case(node): undefined-nested-null
// Undefined inside a value is null at any depth, also an empty slot of an
// array, both ways; a whole argument or result keeps it (#687 §七).
test('undefined is null inside a value, and kept as a whole argument', async () => {
  // eslint-disable-next-line no-sparse-arrays
  const sparse = () => [1, , 3]
  const got = []
  const { peer, sent, incoming, fault } = harness((_, method, args) => { got.push(args); return method === 'sparse' ? sparse() : { a: undefined, b: [undefined] } })
  peer.receive(invoke(1, 'sparse'))
  await Promise.resolve()
  const crossed = { type: 'list', value: [data(1), data(null), data(3)] }
  assert.deepEqual(sent.at(-1).value, crossed)
  incoming.push({ op: 'return', id: 'node:1', value: data(null) })
  peer.invoke('t', 'take', [sparse(), undefined])
  assert.deepEqual(sent.at(-1).args.value, [crossed, { type: 'undefined' }])
  // Received: the marker inside a value is null; a whole argument stays.
  peer.receive({ op: 'invoke', id: 'rust:2', path: [], target: 't', method: 'record', args: { type: 'list', value: [{ type: 'undefined' }, { type: 'list', value: [data(1), { type: 'undefined' }] }, { type: 'record', value: { a: { type: 'undefined' } } }] } })
  await Promise.resolve()
  assert.deepEqual(got.at(-1), [undefined, [1, null], { a: null }])
  assert.equal(got.at(-1).length, 3)
  assert.equal(fault(), undefined)
  peer.close()
})

// A thrown error whose graph is not in its shape (from an older far end:
// Go's `nodes: null`) fails the call with its name and message; it used to
// end the session (`invalid error graph`, #675).
test('an error graph not in its shape fails only its call', async () => {
  const { peer, incoming, fault } = harness(() => {})
  incoming.push({ op: 'throw', id: 'node:1', error: { name: 'ThrownValue', message: 'a string', graph: { root: { type: 'data', value: 'a string' }, nodes: null } } })
  assert.throws(() => peer.invoke('t', 'm', []), error => error.name === 'ThrownValue' && error.message === 'a string')
  // A thrown string relayed with an empty graph is the string again.
  incoming.push({ op: 'throw', id: 'node:2', error: { name: 'ThrownValue', message: 'a string', graph: { root: { type: 'data', value: 'a string' }, nodes: [] } } })
  assert.throws(() => peer.invoke('t', 'm', []), thrown => thrown === 'a string')
  assert.equal(fault(), undefined)
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
    assert.throws(() => peer.invoke('host:svc', 'take', [value]), error =>
      error.message === 'svc.take: argument 1 cannot be sent: no access' && error.cause === frozen)
  }
  assert.equal(frozen.message, 'no access')
  assert.equal(sent.length, 0, 'nothing was sent')
  peer.receive(invoke(1, 'get'))
  await Promise.resolve()
  assert.equal(sent.at(-1).error.message, 'test.get: the result cannot be sent: no access')
  assert.equal(frozen.message, 'no access')
  assert.equal(fault(), undefined)
  peer.close()
})

// guarantee: sdk/error-codes
// A value that cannot be sent is refused with code bad-value and data
// `{ part, argument?, limit? }`, as every language says it (§4.3, #677).
test('a value that cannot be sent carries code bad-value and its part', async () => {
  const big = 'x'.repeat(2048)
  const { peer, sent } = harness((_, method) => method === 'big' ? big : method === 'boom' ? (() => { throw new Error(big) })() : { f: Symbol('s') }, { maxMessage: 1024 })
  const refusal = run => { try { run() } catch (error) { return { code: error.code, data: error.data } } }
  assert.deepEqual(refusal(() => peer.invoke('host:svc', 'take', [1, big])), { code: 'bad-value', data: { part: 'argument', argument: 2, limit: 1024 } })
  assert.deepEqual(refusal(() => peer.invoke('host:svc', 'take', [1, { a: [Symbol('s')] }])), { code: 'bad-value', data: { part: 'argument', argument: 2 } })
  const { decodeError } = await import('../src/errors.mjs')
  // The refusal this side sends, as the far end decodes it.
  const wire = async (id, method) => { peer.receive(invoke(id, method)); await Promise.resolve(); return decodeError(sent.at(-1).error) }
  const back = await wire(1, 'big')
  assert.deepEqual([back.code, back.data], ['bad-value', { part: 'result', limit: 1024 }])
  const boom = await wire(2, 'boom')
  assert.deepEqual([boom.code, boom.data], ['bad-value', { part: 'error', limit: 1024 }])
  const symbol = await wire(3, 'symbol')
  assert.deepEqual([symbol.message, symbol.code, symbol.data], ['test.symbol: the result cannot be sent: a symbol cannot cross between processes', 'bad-value', { part: 'result' }])
  peer.close()
})

// guarantee: sdk/error-positions, sdk/error-rethrown
// An error answering a call has the stack of the plugin's code where it made
// the call, not the session's receive loop; thrown on, it adds that place
// to `rethrown` (§4.4, #677).
test('an error answering a call has the caller\'s stack, and a rethrow adds it', async () => {
  const { encodeError } = await import('../src/errors.mjs')
  const { peer, incoming } = harness(() => {})
  const remote = { name: 'KeyError', message: "'region'", graph: { root: { type: 'reference', value: 0 }, nodes: [{ type: 'error', name: 'KeyError', message: "'region'", stack: 'KeyError: \'region\'\n  File "weather.py", line 31, in tomorrow' }] } }
  // Synchronous.
  incoming.push({ op: 'throw', id: 'node:1', error: remote })
  function askTheWeatherSync() { return peer.invoke('host:weather', 'tomorrow', []) }
  let sync
  try { askTheWeatherSync() } catch (error) { sync = error }
  assert.match(sync.stack, /^KeyError: 'region'\n\s+at askTheWeatherSync .*session\.test\.mjs/)
  assert.doesNotMatch(sync.stack, /src[\\/]session\.mjs|weather\.py/)
  // Asynchronous.
  function askTheWeather() { return peer.invokeAsync('host:weather', 'tomorrow', []) }
  const pending = askTheWeather()
  peer.receive({ op: 'throw', id: 'node:2', error: remote })
  const failed = await pending.catch(error => error)
  assert.match(failed.stack, /^KeyError: 'region'\n\s+at askTheWeather .*session\.test\.mjs/)
  // Thrown on: the graph as received, one `rethrown` item, the caller's frames.
  const sent = encodeError(failed).graph.nodes[0]
  assert.match(sent.stack, /weather\.py/)
  assert.equal(sent.rethrown.length, 1)
  assert.match(sent.rethrown[0], /^\s+at askTheWeather /)
  peer.close()
})

// objects-case(node): cancel
// objects-case(node): holder-killed-releases
// An object passed by reference: a call of its method the caller gives up
// aborts the method's signal; once the host releases it (its holder let go
// or went, the host's session to the holder ending), it is let go of here.
test('an object passed by reference is cancelled through and released by the host', async () => {
  let aborted
  class Job { run(signal) { return new Promise(resolve => signal.addEventListener('abort', () => { aborted = signal.reason?.name; resolve('stopped') })) } }
  const { peer, sent, fault } = harness(() => new Job())
  peer.receive(invoke(1, 'job')); await Promise.resolve()
  const id = sent.at(-1).value.value.id
  assert.equal(peer.references().exports, 1)
  peer.receive({ op: 'call', id: 'rust:2', path: [], reference: id, method: 'run', args: { type: 'list', value: [{ type: 'signal' }] } })
  await new Promise(resolve => setImmediate(resolve))
  peer.receive({ op: 'cancel', id: 'rust:2' })
  await new Promise(resolve => setImmediate(resolve))
  assert.equal(aborted, 'AbortError')
  const before = peer.references().exports
  peer.receive({ op: 'release', reference: id, count: 1 })
  assert.equal(peer.references().exports, before - 1, 'the object is let go of')
  assert.equal(fault(), undefined)
  peer.close()
})

// A received object stays a reference wherever it is put: inside a plain
// object, as in an array.
test('a received object nested in data is still a reference', async () => {
  const got = []
  const { peer, sent, fault } = harness((_, _method, args) => { got.push(...args); return 'ok' })
  const object = { type: 'reference', value: { id: 7, kind: 'object', home: false, origin: [] } }
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: 'test', method: 'take', args: { type: 'list', value: [object] } })
  await Promise.resolve()
  peer.invokeAsync('t', 'give', [{ cache: got[0], n: 1 }]).catch(() => {})
  assert.deepEqual(sent.at(-1).args.value[0], { type: 'record', value: { cache: { type: 'reference', value: { id: 7, kind: 'object', home: true, origin: [] } }, n: data(1) } })
  assert.equal(fault(), undefined)
  peer.close()
})

// objects-case(node): receiver-releases
// A method taken off a received object keeps the object, as a function
// reference would; the object let go of entirely is released (collected).
test('a method taken off a received object keeps it; collected, it is released', async () => {
  const v8 = await import('node:v8'), vm = await import('node:vm')
  v8.setFlagsFromString('--expose-gc')
  const gc = vm.runInNewContext('gc')
  const collect = async () => { for (let i = 0; i < 5; i++) { gc(); await new Promise(resolve => setTimeout(resolve, 10)) } }
  let got
  const { peer, sent, fault } = harness((_, _method, args) => { got = args[0]; return 'ok' })
  const take = id => peer.receive({ op: 'invoke', id: `rust:${id}`, path: [], target: 'test', method: 'take', args: { type: 'list', value: [{ type: 'reference', value: { id, kind: 'object', home: false, origin: [] } }] } })
  take(1); await new Promise(resolve => setImmediate(resolve))
  let inc = got.inc; got = undefined
  await collect()
  assert.equal(sent.some(frame => frame.op === 'release'), false, 'not released while a method is held')
  const result = inc()
  const call = sent.at(-1)
  assert.deepEqual([call.op, call.method], ['call', 'inc'])
  peer.receive({ op: 'return', id: call.id, value: data(1) })
  assert.equal(await result, 1)
  inc = undefined
  await collect()
  assert.deepEqual(sent.find(frame => frame.op === 'release'), { op: 'release', reference: 1, count: 1 })
  assert.equal(fault(), undefined)
  peer.close()
})
