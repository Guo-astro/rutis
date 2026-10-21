import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { Session, ENDPOINT_PROTOCOL } from '../src/session.mjs'
import { decode } from '../src/codec.mjs'

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
  assert.throws(() => peer.invoke('test', 'bad', [callback, { nested: Symbol('x') }]), /unsupported binding/)
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
    if (expected === 'throw') assert.match(reply.error.message, /cannot receive object references/)
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
