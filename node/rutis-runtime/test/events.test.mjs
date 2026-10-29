// guarantee: events/js-bus — the rules of the logical bus as JavaScript
// runs them (#464, docs/design-events-protocol-2026-10-18.en.md): the five
// modes, scope, filter, order, emit's error reporting and unload waiting
// as a Cordis node's bus dispatches them here (../src/bridge/bus.mjs); the
// leaf runtime's bus, which dispatches through its host only (platform
// cleanup §7.5): the host messages, `events.call`, emit's reporting.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { Bus, kindOf } from '../src/events.mjs'
import { Bus as NodeBus } from '../src/bridge/bus.mjs'

const T1 = { id: '17', group: 'tenant' }
const S1 = { id: '42', group: 'session' }
const T2 = { id: '18', group: 'tenant' }

// A Cordis node's bus, which dispatches here, its reported lines kept.
function local() {
  const reported = []
  const bus = new NodeBus({ report: line => reported.push(line) })
  return { bus, reported }
}

// A recording peer: `answer(method, args)` answers what the host would.
function peer(answer = () => undefined) {
  const sent = []
  return {
    sent,
    callAsync(target, method, args) { sent.push([method, args]); return Promise.resolve(answer(method, args)) },
    call(target, method, args) { sent.push([method, args]); return answer(method, args) },
    callFunctionAsync(fn, args) { sent.push(['call', args]); return Promise.resolve(fn(...args)) },
  }
}

const listen = (bus, name, callback, more = {}) => bus.on({ name, callback, row: more.row ?? 'r', plugin: 'p', ...more })

test('emit reaches every listener in order, returns at once and reports failures where they ran', async () => {
  const { bus, reported } = local()
  const log = []
  listen(bus, 'e', async () => { log.push('a'); throw Object.assign(new Error('a failed'), { name: 'Boom' }) })
  listen(bus, 'e', () => { log.push('b'); throw new TypeError('b failed') }, { row: 'rb' })
  listen(bus, 'e', () => { log.push('c'); return 'C' })
  assert.equal(bus.dispatch('emit', 'e', []), undefined)
  assert.deepEqual(log, ['a', 'b', 'c'])
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(reported.sort(), [
    'rutis: listener of e in row r failed: Boom: a failed',
    'rutis: listener of e in row rb failed: TypeError: b failed',
  ])
})

test('parallel waits for all and aggregates failures in registration order', async () => {
  const { bus } = local()
  const log = []
  listen(bus, 'p', async () => { await new Promise(resolve => setTimeout(resolve, 30)); log.push('a'); throw Object.assign(new Error('a'), { name: 'AErr' }) })
  listen(bus, 'p', () => { log.push('b'); throw Object.assign(new Error('b'), { name: 'BErr' }) })
  listen(bus, 'p', () => { log.push('c') })
  await assert.rejects(bus.dispatch('parallel', 'p', []), error => {
    assert.equal(error.name, 'AggregateError')
    assert.equal(error.message, '2 listeners of p failed')
    assert.deepEqual(error.errors.map(e => e.name), ['AErr', 'BErr'])
    return true
  })
  assert.deepEqual(log.sort(), ['a', 'b', 'c'])
  const one = local().bus
  listen(one, 'q', () => { throw Object.assign(new Error('a'), { name: 'AErr' }) })
  await assert.rejects(one.dispatch('parallel', 'q', []), { name: 'AggregateError', message: '1 listeners of q failed' })
  assert.equal(await one.dispatch('parallel', 'nobody', []), undefined)
})

test('serial returns the first result; null, undefined and false are none, 0 is one', async () => {
  const { bus } = local()
  const log = []
  listen(bus, 's', () => { log.push('a'); return null })
  listen(bus, 's', async () => { log.push('b'); return false })
  listen(bus, 's', () => { log.push('c'); return undefined })
  listen(bus, 's', () => { log.push('d'); return 0 })
  listen(bus, 's', () => { log.push('e'); return 'E' })
  assert.equal(await bus.dispatch('serial', 's', []), 0)
  assert.deepEqual(log, ['a', 'b', 'c', 'd'])
  listen(bus, 'none', () => false)
  assert.equal(await bus.dispatch('serial', 'none', []), undefined)
  listen(bus, 'err', () => { throw Object.assign(new Error('a'), { name: 'AErr' }) })
  listen(bus, 'err', () => { log.push('after') })
  await assert.rejects(bus.dispatch('serial', 'err', []), { name: 'AErr' })
  assert.ok(!log.includes('after'))
})

test('waterfall chains next outward in, vetoes, ends at the terminal and keeps the value as is', async () => {
  const { bus } = local()
  const log = []
  listen(bus, 'w', async (x, next) => { log.push('a'); return `a${await next()}` })
  listen(bus, 'w', async (x, next) => { log.push('b'); return `b${await next()}` })
  assert.equal(await bus.dispatch('waterfall', 'w', [1], { terminal: x => { log.push(`terminal:${x}`); return 't' } }), 'abt')
  assert.deepEqual(log, ['a', 'b', 'terminal:1'])
  listen(bus, 'veto', () => 'v')
  listen(bus, 'veto', () => { throw new Error('not reached') })
  assert.equal(bus.dispatch('waterfall', 'veto', [], {}), 'v')
  listen(bus, 'false', () => false)
  assert.equal(bus.dispatch('waterfall', 'false', []), false)
  assert.equal(bus.dispatch('waterfall', 'empty', [], { terminal: () => 't' }), 't')
  assert.equal(bus.dispatch('waterfall', 'empty', []), undefined)
  assert.throws(() => bus.dispatch('waterfall', 'empty', [], { terminal: 3 }), TypeError)
})

test('next called twice or after the waterfall ended fails with WaterfallNext', async () => {
  const { bus } = local()
  listen(bus, 'twice', async (next) => { await next(); return next() })
  listen(bus, 'twice', () => 'b')
  await assert.rejects(bus.dispatch('waterfall', 'twice', []), { name: 'WaterfallNext', message: 'next of twice was already called' })
  let kept
  listen(bus, 'late', next => { kept = next; return 'x' })
  assert.equal(bus.dispatch('waterfall', 'late', []), 'x')
  // As through the host, a failing next is a rejected Promise, not a throw.
  await assert.rejects(kept(), { name: 'WaterfallNext', message: 'next of late was called after its waterfall ended' })
  listen(bus, 'sync-twice', next => { next(); return next() })
  listen(bus, 'sync-twice', () => 'b')
  await assert.rejects(Promise.resolve(bus.dispatch('waterfall', 'sync-twice', [])), { name: 'WaterfallNext', message: 'next of sync-twice was already called' })
})

test('bail is synchronous; an async listener is refused, one returning a Promise fails', () => {
  const { bus } = local()
  const log = []
  listen(bus, 'b', () => { log.push('a'); return null })
  listen(bus, 'b', () => { log.push('b'); return 'B' })
  listen(bus, 'b', () => { log.push('c'); return 'C' })
  assert.equal(bus.dispatch('bail', 'b', []), 'B')
  assert.deepEqual(log, ['a', 'b'])
  listen(bus, 'async', async () => { log.push('async') }, { row: 'y' })
  assert.throws(() => bus.dispatch('bail', 'async', []), {
    name: 'AsyncListener', message: 'listener of async in row y is asynchronous and cannot run in a synchronous bail',
  })
  assert.ok(!log.includes('async'), 'an async listener is not called')
  listen(bus, 'any', () => { log.push('any'); return Promise.reject(new Error('ignored')) })
  assert.throws(() => bus.dispatch('bail', 'any', []), { name: 'AsyncListener' })
  assert.ok(log.includes('any'))
  assert.equal(kindOf(async () => {}), 'async')
  assert.equal(kindOf(() => {}), 'any')
})

test('every listener hears every event; a scope is a descriptor', async () => {
  const { bus } = local()
  const log = []
  for (const [label, scope] of [['r', []], ['u', [T1]], ['s', [T1, S1]], ['v', [T2]]]) {
    listen(bus, 'scope', () => { log.push(label) }, { scope })
  }
  await bus.dispatch('parallel', 'scope', [])
  assert.deepEqual(log.splice(0), ['r', 'u', 's', 'v'])
})

test('a filter sees each candidate once, before any runs; global listeners skip it', async () => {
  const { bus, reported } = local()
  const log = []
  listen(bus, 'f', () => { log.push('a') }, { row: 'a' })
  listen(bus, 'f', () => { log.push('b') }, { row: 'b' })
  listen(bus, 'f', () => { log.push('g') }, { row: 'g', global: true })
  listen(bus, 'f', () => { log.push('d') }, { row: 'd', scope: [T2] })
  const keep = descriptor => { log.push(`filter:${descriptor.row}`); return descriptor.row === 'a' }
  await bus.dispatch('serial', 'f', [], { keep })
  assert.deepEqual(log.splice(0), ['filter:a', 'filter:b', 'filter:d', 'a', 'g'])
  const seen = []
  await bus.dispatch('serial', 'f', [], { keep: descriptor => { seen.push(descriptor); return false } })
  assert.deepEqual(seen[0], { name: 'f', prefix: false, plugin: 'p', row: 'a', runtime: null, node: null, scope: [], global: false })
  // A filter that throws fails the dispatch before any listener runs;
  // in an emit it is reported.
  log.length = 0
  await assert.rejects(bus.dispatch('parallel', 'f', [], { keep: () => { throw new Error('bad filter') } }), /bad filter/)
  assert.throws(() => bus.dispatch('bail', 'f', [], { keep: () => { throw new Error('bad filter') } }), /bad filter/)
  assert.equal(bus.dispatch('emit', 'f', [], { row: 'e', keep: () => { throw new Error('bad filter') } }), undefined)
  assert.deepEqual(log, [])
  assert.deepEqual(reported, ['rutis: filter of f from row e failed: Error: bad filter'])
})

test('prepend goes before every listener so far', async () => {
  const { bus } = local()
  const log = []
  listen(bus, 'o', () => { log.push('b') })
  listen(bus, 'o', () => { log.push('a') }, { prepend: true })
  listen(bus, 'o', () => { log.push('c') })
  await bus.dispatch('serial', 'o', [])
  assert.deepEqual(log, ['a', 'b', 'c'])
})

test('rutis/* is refused synchronously in every mode; it can be listened to', async () => {
  const host = peer()
  for (const bus of [local().bus, new Bus({ runtime: 'node', peer: host, report: () => {} })]) {
    for (const mode of ['emit', 'parallel', 'serial', 'waterfall', 'bail']) {
      assert.throws(() => bus.dispatch(mode, 'rutis/fake', []), { name: 'ReservedEvent', message: 'rutis/* events are emitted by rutis only: rutis/fake' })
    }
    assert.ok(listen(bus, 'rutis/row', () => {}).live)
  }
  assert.ok(!host.sent.some(([method]) => method === 'events.dispatch'), 'a refused emission reaches no host')
})

test('a listener withdrawn before its turn is skipped', async () => {
  const { bus } = local()
  const log = []
  let second
  listen(bus, 'x', () => { log.push('a'); bus.off(second) })
  second = listen(bus, 'x', () => { log.push('b') })
  await bus.dispatch('parallel', 'x', [])
  await bus.dispatch('serial', 'x', [])
  assert.deepEqual(log, ['a', 'a'])
})

test('registration, withdrawal and every dispatch go to the host', async () => {
  const host = peer(method => method === 'events.dispatch' ? 'H' : undefined)
  const reported = []
  const bus = new Bus({ runtime: 'node', peer: host, report: line => reported.push(line) })
  let called = 0
  const entry = bus.on({ name: 'n', callback: async () => { called++; return 'A' }, row: 'chat', plugin: 'chat-ui', prepend: true, global: true })
  assert.deepEqual(host.sent.shift(), ['events.on', [{ id: entry.id, name: 'n', row: 'chat', plugin: 'chat-ui', kind: 'async', prepend: true, global: true }]])
  assert.equal(bus.registrations('chat').length, 1)
  // Even with every listener of the name here, the host dispatches; it
  // calls the listeners here back (events.call), none runs here meanwhile.
  assert.equal(await bus.dispatch('serial', 'n', [1], { row: 'chat' }), 'H')
  assert.deepEqual(host.sent.shift(), ['events.dispatch', [{ row: 'chat', name: 'n', mode: 'serial', args: [1], [Symbol.for('rutis.session.record')]: true }]])
  assert.equal(await bus.dispatch('parallel', 'n', []), 'H')
  assert.equal(await bus.dispatch('waterfall', 'n', []), 'H')
  assert.equal(bus.dispatch('bail', 'n', []), 'H', 'bail is a synchronous call to the host')
  assert.equal(bus.dispatch('emit', 'n', [2]), undefined)
  assert.deepEqual(host.sent.splice(0).map(([method, [fields]]) => [method, fields.mode]), [
    ['events.dispatch', 'parallel'], ['events.dispatch', 'waterfall'], ['events.dispatch', 'bail'], ['events.dispatch', 'emit'],
  ])
  assert.equal(called, 0)
  bus.off(entry)
  assert.deepEqual(host.sent.shift(), ['events.off', [[entry.id]]])
  // The session ended: nothing is dispatched; an emit is reported, the
  // other modes fail.
  bus.ended()
  assert.equal(bus.dispatch('emit', 'n', [], { row: 'chat' }), undefined)
  assert.deepEqual(reported, ['rutis: emit of n from row chat failed: BindingError: the host disconnected'])
  for (const mode of ['parallel', 'serial', 'waterfall']) {
    await assert.rejects(bus.dispatch(mode, 'n', []), { name: 'BindingError', message: 'the host disconnected', code: 'peer-gone' }, mode)
  }
  assert.throws(() => bus.dispatch('bail', 'n', []), { name: 'BindingError', message: 'the host disconnected', code: 'peer-gone' })
  assert.deepEqual(host.sent, [])
})

// §5.6: with a session that tells the reply as its frame is read
// (`callAnswered`), an emit's failure is reported from the reply: a
// refusal at once, a 0.13 host's future once it fails.
test('an emit through the host is reported from its reply: a refusal, or a future that fails', async () => {
  const replies = []
  const sent = []
  const host = {
    callAnswered(target, method, args, answered) { sent.push(method); replies.push(answered) },
    callAsync(target, method) { sent.push(method); return Promise.resolve() },
  }
  const reported = []
  const bus = new Bus({ runtime: 'node', peer: host, report: line => reported.push(line) })
  bus.on({ name: 'o', callback: () => {}, row: 'r' })
  for (let i = 0; i < 3; i++) assert.equal(bus.dispatch('emit', 'o', [i]), undefined)
  assert.equal(sent.filter(method => method === 'events.dispatch').length, 3)
  replies.shift()({ ok: true, value: undefined })
  let fail
  replies.shift()({ ok: true, value: new Promise((_, reject) => { fail = reject }) })
  replies.shift()({ ok: false, value: Object.assign(new Error('refused'), { name: 'BindingError' }) })
  assert.deepEqual(reported, ['rutis: emit of o from the mount failed: BindingError: refused'])
  fail(Object.assign(new Error('later'), { name: 'BindingError' }))
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(reported, ['rutis: emit of o from the mount failed: BindingError: refused', 'rutis: emit of o from the mount failed: BindingError: later'])
})

// §11: failures nobody waits for are reported, one line each: an emit the
// host refuses or that cannot be sent, a registration the host refuses.
test('a refused or unsent emit through the host and a refused registration are reported', async () => {
  const reported = []
  const refusing = {
    callAsync(target, method) { return Promise.reject(Object.assign(new Error(`${method} refused`), { name: 'BindingError' })) },
  }
  const bus = new Bus({ runtime: 'node', peer: refusing, report: line => reported.push(line) })
  bus.nameRow('k1', 'chat')
  bus.on({ name: 't/on', callback: () => {}, row: 'k1', plugin: 'p' })
  assert.equal(bus.dispatch('emit', 't/e', [], { row: 'k1' }), undefined)
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(reported.splice(0), [
    'rutis: registration of listener of t/on in row chat failed: BindingError: events.on refused',
    'rutis: emit of t/e from row chat failed: BindingError: events.dispatch refused',
  ])
  // The session ended under it: the frame cannot be sent.
  refusing.callAsync = () => { throw Object.assign(new Error('the session ended'), { name: 'BindingError' }) }
  assert.equal(bus.dispatch('emit', 't/e', [], { row: 'k1' }), undefined)
  assert.deepEqual(reported, ['rutis: emit of t/e from row chat failed: BindingError: the session ended'])
})

test('through the host, the filter is a selector over descriptors and the terminal a function', async () => {
  const host = peer()
  const bus = new Bus({ runtime: 'node', peer: host, report: () => {} })
  const own = bus.on({ name: 'f', callback: () => {}, row: 'chat', plugin: 'chat-ui', owner: 'ctx' })
  const owners = []
  bus.dispatch('parallel', 'f', [], { row: 'e', keep: (descriptor, entry) => { owners.push(entry?.owner ?? null); return descriptor.row === 'chat' } })
  const [, [fields]] = host.sent.at(-1)
  const descriptor = row => ({ name: 'f', prefix: false, plugin: row === 'chat' ? 'chat-ui' : 'x', row, runtime: row === 'chat' ? 'node' : 'py', node: null, scope: [], global: false })
  // The host marks this process's own listeners with their id.
  assert.deepEqual(fields.filter([{ ...descriptor('chat'), listener: own.id }, descriptor('other')]), [true, false])
  assert.deepEqual(owners, ['ctx', null])
  bus.dispatch('waterfall', 'f', [7], { terminal: x => x * 2 })
  assert.equal(host.sent.at(-1)[1][0].terminal(7), 14)
  assert.ok(own.live)
})

// The selector finds this process's own listener by the id the host gives
// (`listener`), never by what the descriptor says: two copies of one row
// in two instances, or one plugin in two Cordis Contexts, have equal rows
// and plugins; the filter sees the host's descriptor (the copy's own
// scope), without the id.
test('through the host, the filter judges the very listener the host asks about', async () => {
  const host = peer()
  const bus = new Bus({ runtime: 'node', peer: host, report: () => {} })
  bus.nameRow('c#2', 'c')
  bus.nameRow('c#1', 'c')
  const t2 = bus.on({ name: 'f', callback: () => {}, row: 'c#2', plugin: 'p', scope: [T2], owner: 'ctx-t2' })
  const t1 = bus.on({ name: 'f', callback: () => {}, row: 'c#1', plugin: 'p', scope: [T1], owner: 'ctx-t1' })
  const seen = []
  bus.dispatch('parallel', 'f', [], { row: 'e', keep: (descriptor, entry) => { seen.push([descriptor, entry?.owner]); return descriptor.scope[0]?.id === T1.id } })
  const [, [fields]] = host.sent.at(-1)
  const at = scope => ({ name: 'f', prefix: false, plugin: 'p', row: 'c', runtime: 'node', node: null, scope, global: false })
  // Only the t1 copy hears an emission in t1; the host asks about it alone.
  assert.deepEqual(fields.filter([{ ...at([T1]), listener: t1.id }]), [true])
  assert.deepEqual(seen, [[at([T1]), 'ctx-t1']])
  // The same descriptor from another runtime or node is not this process's.
  seen.length = 0
  assert.deepEqual(fields.filter([{ ...at([T1]), runtime: 'py' }]), [true])
  assert.deepEqual(seen, [[{ ...at([T1]), runtime: 'py' }, undefined]])
  assert.ok(t2.live)
})

test('a filter gets a copy of the scope; an unloaded row cannot emit', () => {
  const { bus } = local()
  const chain = [T1]
  bus.on({ name: 's', callback: () => 'x', row: 'r', scope: chain })
  bus.dispatch('bail', 's', [], { keep: descriptor => { descriptor.scope[0].id = 'changed'; descriptor.scope.push(S1); return true } })
  assert.deepEqual(chain, [T1])
  assert.equal(T1.id, '17')
  bus.nameRow('r', 'r')
  bus.withdraw('r')
  for (const mode of ['emit', 'parallel', 'serial', 'waterfall', 'bail']) {
    assert.throws(() => bus.dispatch(mode, 's', [], { row: 'r' }), { name: 'BindingError', message: 'row r is not loaded in this runtime' }, mode)
  }
  // Loaded again under the same key: it may.
  bus.nameRow('r', 'r')
  assert.equal(bus.dispatch('bail', 's', [], { row: 'r' }), undefined)
  // A context of the old row still cannot.
  assert.throws(() => bus.dispatch('emit', 's', [], { row: 'r', gone: true }), { name: 'BindingError' })
})

test('a listener its row\'s unload stops is not reported; a mount emitter is "the mount"', async () => {
  // A Cordis node's bus: the failure of an emit listener the unload's
  // signal stops is not reported, another one is.
  const { bus: here, reported: lines } = local()
  const stop = new AbortController()
  here.on({ name: 'u', callback: () => new Promise((_, reject) => stop.signal.addEventListener('abort', () => reject(stop.signal.reason))), row: 'r' })
  here.on({ name: 'u', callback: async () => { throw new Error('plain') }, row: 'r' })
  here.dispatch('emit', 'u', [])
  await new Promise(resolve => setImmediate(resolve))
  here.withdraw('r')
  stop.abort(new DOMException('the row is unloading', 'AbortError'))
  await here.settle('r', 1_000)
  assert.deepEqual(lines, ['rutis: listener of u in row r failed: Error: plain'])

  // The leaf runtime's bus: the host calls an emit listener here and
  // reports its failure; one the unload stops answers as if it returned.
  const host = peer(method => method === 'events.dispatch' ? Promise.reject(Object.assign(new Error('no'), { name: 'BindingError' })) : undefined)
  const reported = []
  const bus = new Bus({ runtime: 'node', peer: host, report: line => reported.push(line) })
  const abort = new AbortController()
  const stopped = bus.on({ name: 'u', callback: () => new Promise((_, reject) => abort.signal.addEventListener('abort', () => reject(abort.signal.reason))), row: 'r' })
  const plain = bus.on({ name: 'u', callback: async () => { throw new Error('plain') }, row: 'r' })
  const first = bus.call({ listener: stopped.id, dispatch: 'd1', own: true, name: 'u', mode: 'emit', args: [] })
  const second = bus.call({ listener: plain.id, dispatch: 'd1', own: true, name: 'u', mode: 'emit', args: [] })
  bus.withdraw('r')
  abort.abort(new DOMException('the row is unloading', 'AbortError'))
  assert.equal(await first, undefined)
  await assert.rejects(second, { message: 'plain' })
  await bus.settle('r', 1_000)
  assert.deepEqual(reported, [], 'the host reports a listener\'s failure, not this process')
  bus.dispatch('emit', 'm', [])
  bus.dispatch('emit', 'm', [], { row: null, keep: () => { throw new Error('bad filter') } })
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(reported, ['rutis: emit of m from the mount failed: BindingError: no', 'rutis: emit of m from the mount failed: BindingError: no'])
})

test('a registration the host refuses while its row loads fails the load, later it is reported', async () => {
  const refuse = Object.assign(new Error('refused'), { name: 'BindingError' })
  const host = peer(method => method === 'events.on' ? Promise.reject(refuse) : undefined)
  const reported = []
  let loading = true
  const bus = new Bus({ peer: host, report: line => reported.push(line), loading: () => loading })
  bus.on({ name: 'n', callback: () => {}, row: 'r' })
  await assert.rejects(Promise.all(bus.registrations('r')), { message: 'refused' })
  loading = false
  bus.on({ name: 'later', callback: () => {}, row: 'r' })
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(reported, ['rutis: registration of listener of later in row r failed: BindingError: refused'])
})

test('a restart withdraws the row\'s listeners here and on the host, and keeps it loaded', () => {
  const host = peer()
  const bus = new Bus({ peer: host, report: () => {} })
  const a = bus.on({ name: 'n', callback: () => {}, row: 'r' })
  host.sent.length = 0
  bus.withdraw('r', { restart: true })
  assert.deepEqual(host.sent, [['events.off', [[a.id]]]])
  assert.ok(!a.live)
  assert.equal(bus.dispatch('bail', 'n', [], { row: 'r' }), undefined)
})

test('events.call runs a listener here by mode; withdrawn ones fail', async () => {
  const host = peer()
  const reported = []
  const dispatched = []
  const bus = new Bus({ peer: host, report: line => reported.push(line), dispatched: (...args) => dispatched.push(args) })
  const a = bus.on({ name: 'n', callback: (...args) => typeof args.at(-1) === 'function' ? Promise.resolve(args.at(-1)()).then(r => `a${r}`) : `a${args[0]}`, row: 'r' })
  const failing = bus.on({ name: 'n', callback: async () => { throw new Error('boom') }, row: 'r' })
  const asynchronous = bus.on({ name: 'n', callback: async () => 'x', row: 'r' })
  assert.equal(await bus.call({ listener: a.id, dispatch: 'd1', own: false, name: 'n', mode: 'serial', args: [1] }), 'a1')
  assert.equal(await bus.call({ listener: a.id, dispatch: 'd1', own: false, name: 'n', mode: 'parallel', args: [2] }), 'a2')
  assert.equal(await bus.call({ listener: a.id, dispatch: 'd2', own: true, name: 'n', mode: 'serial', args: [3] }), 'a3')
  // internal/dispatch: once per dispatch from elsewhere, not for one's own.
  assert.deepEqual(dispatched, [['serial', 'n', [1]]])
  assert.equal(await bus.call({ listener: a.id, dispatch: 'd3', name: 'n', mode: 'waterfall', args: [], next: () => 'rest' }), 'arest')
  assert.deepEqual(host.sent.at(-1), ['call', []])
  // An emit listener's failure goes back to the host, which reports it.
  await assert.rejects(bus.call({ listener: failing.id, dispatch: 'd4', name: 'n', mode: 'emit', args: [] }), { message: 'boom' })
  assert.deepEqual(reported, [])
  assert.throws(() => bus.call({ listener: asynchronous.id, dispatch: 'd5', name: 'n', mode: 'bail', args: [] }), { name: 'AsyncListener' })
  bus.withdraw('r')
  assert.throws(() => bus.call({ listener: a.id, dispatch: 'd7', name: 'n', mode: 'serial', args: [] }), { name: 'BindingError', message: `listener ${a.id} of n is withdrawn` })
  assert.throws(() => bus.call({ listener: '999', name: 'n', mode: 'serial', args: [] }), { message: 'listener 999 of n is withdrawn' })
})

test('unload waits for running listeners, at most the deadline', async () => {
  const { bus, reported } = local()
  let finish
  bus.on({ name: 'u', callback: () => new Promise(resolve => { finish = resolve }), row: 'r' })
  bus.dispatch('emit', 'u', [])
  bus.withdraw('r')
  const settled = bus.settle('r', 5_000)
  finish()
  assert.equal(await settled, true)
  bus.on({ name: 'v', callback: () => new Promise(() => {}), row: 'stuck' })
  bus.dispatch('emit', 'v', [])
  assert.equal(await bus.settle('stuck', 20), false)
  assert.deepEqual(reported, ['rutis: listeners of row stuck still running after 0.02s; unloading anyway'])
})

test('the bus is one file in the Node runtime and the Bun runtime', () => {
  const read = path => readFileSync(new URL(path, import.meta.url), 'utf8')
  assert.equal(read('../../../bun/rutis-bun/src/events.mjs'), read('../src/events.mjs'), 'bun/rutis-bun/src/events.mjs')
})

// A Cordis application linked to other nodes (§9): a linked node's listener
// is a remote entry of its bus, called with the dispatch's mode; prefix
// entries match by prefix; the table is followed for announcements.
test('remote entries take part in every mode, and the table can be followed', async () => {
  const { bus } = local()
  const changes = []
  const unwatch = bus.watch(change => changes.push(change.added ? `+${change.added.name}` : `-${change.removed.name}`))
  const calls = []
  const remote = bus.onRemote({
    name: 't/', prefix: true, kind: 'any', node: 'mac', plugin: 'far',
    invoke(mode, name, args) { calls.push([mode, name, args.length]); return mode === 'serial' ? `far ${name}` : undefined },
  })
  bus.on({ name: 't/x', callback: () => undefined, row: 'r' })
  assert.equal(await bus.dispatch('serial', 't/x', [1]), 'far t/x')
  await bus.dispatch('parallel', 't/y', [])
  assert.deepEqual(calls, [['serial', 't/x', 1], ['parallel', 't/y', 0]])
  assert.deepEqual(bus.describe(remote), { name: 't/', prefix: true, plugin: 'far', row: null, runtime: null, node: 'mac', scope: [], global: false })
  bus.off(remote)
  assert.deepEqual(changes, ['+t/', '+t/x', '-t/'])
  unwatch()
})
