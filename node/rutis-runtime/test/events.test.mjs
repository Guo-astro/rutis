// guarantee: events/js-bus — the rules of the logical bus as one JavaScript
// process runs them (#464, docs/design-events-protocol-2026-10-18.en.md):
// the five modes, scope, filter, order, emit's error reporting, the fast
// path and the host messages, unload waiting.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { Bus, kindOf } from '../src/events.mjs'

const T1 = { id: '17', group: 'tenant' }
const S1 = { id: '42', group: 'session' }
const T2 = { id: '18', group: 'tenant' }

// A bus without a host, its reported lines kept.
function local() {
  const reported = []
  const bus = new Bus({ runtime: 'node', report: line => reported.push(line) })
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
  assert.throws(() => kept(), { name: 'WaterfallNext', message: 'next of late was called after its waterfall ended' })
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

test('a listener hears its own scope and those under it, never another', async () => {
  const { bus } = local()
  const log = []
  for (const [label, scope] of [['r', []], ['u', [T1]], ['s', [T1, S1]], ['v', [T2]]]) {
    listen(bus, 'scope', () => { log.push(label) }, { scope })
  }
  await bus.dispatch('parallel', 'scope', [], { scope: [T1, S1] })
  assert.deepEqual(log.splice(0), ['r', 'u', 's'])
  await bus.dispatch('parallel', 'scope', [], { scope: [T1] })
  assert.deepEqual(log.splice(0), ['r', 'u'])
  await bus.dispatch('parallel', 'scope', [], { scope: [T2] })
  assert.deepEqual(log.splice(0), ['r', 'v'])
  await bus.dispatch('parallel', 'scope', [], { scope: [] })
  assert.deepEqual(log.splice(0), ['r'])
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
  assert.deepEqual(log.splice(0), ['filter:a', 'filter:b', 'a', 'g'])
  const seen = []
  await bus.dispatch('serial', 'f', [], { keep: descriptor => { seen.push(descriptor); return false } })
  assert.deepEqual(seen[0], { name: 'f', prefix: false, plugin: 'p', row: 'a', runtime: 'node', node: null, scope: [], global: false })
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
  const { bus } = local()
  for (const mode of ['emit', 'parallel', 'serial', 'waterfall', 'bail']) {
    assert.throws(() => bus.dispatch(mode, 'rutis/fake', []), { name: 'ReservedEvent', message: 'rutis/* events are emitted by rutis only: rutis/fake' })
  }
  assert.ok(listen(bus, 'rutis/row', () => {}).live)
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

test('registration, withdrawal and the fast path with the host', async () => {
  const host = peer()
  const bus = new Bus({ runtime: 'node', peer: host, report: () => {} })
  const entry = bus.on({ name: 'n', callback: async () => 'A', row: 'chat', plugin: 'chat-ui', prepend: true, global: true })
  assert.deepEqual(host.sent.shift(), ['events.on', [{ id: entry.id, name: 'n', row: 'chat', plugin: 'chat-ui', kind: 'async', prepend: true, global: true }]])
  assert.equal(bus.registrations('chat').length, 1)
  // All here: dispatched here, the host hears nothing.
  assert.equal(await bus.dispatch('serial', 'n', []), 'A')
  assert.deepEqual(host.sent, [])
  // Elsewhere too: through the host.
  bus.elsewhere({ add: ['n'] })
  await bus.dispatch('serial', 'n', [1], { row: 'chat' })
  assert.deepEqual(host.sent.shift(), ['events.dispatch', [{ row: 'chat', name: 'n', mode: 'serial', args: [1], [Symbol.for('rutis.session.record')]: true }]])
  bus.elsewhere({ remove: ['n'], addPrefixes: ['n'] })
  bus.dispatch('bail', 'nx', [])
  assert.equal(host.sent.shift()[1][0].mode, 'bail')
  bus.elsewhere({ removePrefixes: ['n'] })
  assert.ok(bus.local('serial', 'nx'))
  bus.off(entry)
  assert.deepEqual(host.sent.shift(), ['events.off', [[entry.id]]])
  bus.ended()
  assert.ok(bus.local('serial', 'n'))
})

test('an emit through the host keeps later emits of the name there until it is answered', async () => {
  const answers = []
  const host = peer(method => method === 'events.dispatch' ? new Promise(resolve => { answers.push(resolve) }) : undefined)
  const bus = new Bus({ peer: host, report: () => {} })
  const log = []
  bus.on({ name: 'o', callback: x => { log.push(x) }, row: 'r' })
  host.sent.length = 0
  bus.elsewhere({ add: ['o'] })
  bus.dispatch('emit', 'o', [1])
  bus.elsewhere({ remove: ['o'] })
  bus.dispatch('emit', 'o', [2])
  assert.deepEqual(host.sent.map(([method, [fields]]) => [method, fields.args]), [['events.dispatch', [1]], ['events.dispatch', [2]]])
  assert.ok(!bus.local('emit', 'o'))
  assert.ok(bus.local('serial', 'o'), 'only emit keeps to the host')
  answers[0]()
  await new Promise(resolve => setImmediate(resolve))
  assert.ok(!bus.local('emit', 'o'), 'one emit is still unanswered')
  answers[1]()
  await new Promise(resolve => setImmediate(resolve))
  assert.ok(bus.local('emit', 'o'))
})

test('through the host, the filter is a selector over descriptors and the terminal a function', async () => {
  const host = peer()
  const bus = new Bus({ runtime: 'node', peer: host, report: () => {} })
  const own = bus.on({ name: 'f', callback: () => {}, row: 'chat', plugin: 'chat-ui', owner: 'ctx' })
  bus.elsewhere({ add: ['f'] })
  const owners = []
  bus.dispatch('parallel', 'f', [], { row: 'e', keep: (descriptor, entry) => { owners.push(entry?.owner ?? null); return descriptor.row === 'chat' } })
  const [, [fields]] = host.sent.at(-1)
  const descriptor = row => ({ name: 'f', prefix: false, plugin: row === 'chat' ? 'chat-ui' : 'x', row, runtime: row === 'chat' ? 'node' : 'py', node: null, scope: [], global: false })
  assert.deepEqual(fields.filter([descriptor('chat'), descriptor('other')]), [true, false])
  assert.deepEqual(owners, ['ctx', null])
  bus.dispatch('waterfall', 'f', [7], { terminal: x => x * 2 })
  assert.equal(host.sent.at(-1)[1][0].terminal(7), 14)
  assert.ok(own.live)
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
  assert.equal(await bus.call({ listener: failing.id, dispatch: 'd4', name: 'n', mode: 'emit', args: [] }), undefined)
  assert.deepEqual(reported, ['rutis: listener of n in row r failed: Error: boom'])
  assert.throws(() => bus.call({ listener: asynchronous.id, dispatch: 'd5', name: 'n', mode: 'bail', args: [] }), { name: 'AsyncListener' })
  const sync = bus.on({ name: 'n', callback: next => `s${next()}`, row: 'r' })
  assert.equal(bus.call({ listener: sync.id, dispatch: 'd6', name: 'n', mode: 'waterfall-sync', args: [], next: () => 'S' }), 'sS')
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

test('the bus is one file in the Node runtime, the Bun runtime and the testing tool', () => {
  const read = path => readFileSync(new URL(path, import.meta.url), 'utf8')
  const mine = read('../src/events.mjs')
  assert.equal(read('../../rutis/src/events.mjs'), mine, 'node/rutis/src/events.mjs')
  assert.equal(read('../../../bun/rutis-bun/src/events.mjs'), mine, 'bun/rutis-bun/src/events.mjs')
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
