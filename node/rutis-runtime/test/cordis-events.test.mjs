// guarantee: events/one-bus — Cordis plugins on the logical bus (#464):
// `internal/listener` registers every listener that is not internal/* with
// the host, Cordis's five methods dispatch through the host in their mode,
// `Context.filter` sees the real Context for a listener of this process
// and `{ rutis }` for one elsewhere, `ctx.rutis` on every Context,
// `internal/dispatch` fired once, internal/* stays Cordis's.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Context } from '@deepseek-ai/cordis'
import { Bus } from '../src/events.mjs'
import { install } from '../src/cordis-events.mjs'
import { definePlugin } from '../../rutis/src/index.mjs'
import { supported, toCordis } from '../src/leaf.mjs'
import { eventually } from './support.mjs'
import { fakePeer } from './fixtures/fake-peer.mjs'

// A root Context on the runtime's bus; plugins are rows by name. Its host
// is `peer`, by default a fake host that dispatches on the listeners
// registered with it by calling them back (./fixtures/fake-peer.mjs).
function setup({ peer } = {}) {
  const root = new Context()
  const reported = []
  const report = line => reported.push(line)
  const host = peer ?? fakePeer()
  const bus = new Bus({ runtime: 'node', peer: host, report })
  if (!peer) host.connect(bus)
  const scopes = peer ? new Map() : host.scopes
  const signals = new Map()
  install(root, Context, bus, context => {
    for (let fiber = context?.fiber; fiber; fiber = fiber.parent?.fiber) {
      const row = fiber.runtime?.name
      if (row && scopes.has(row)) return { row, scope: scopes.get(row), signal: signals.get(row).signal }
      if (!fiber.runtime) break
    }
    return { row: null, scope: [], signal: undefined }
  })
  // Load `apply` as the row `row` in `scope`.
  const load = async (row, apply, scope = []) => {
    scopes.set(row, scope)
    signals.set(row, new AbortController())
    const fiber = root.plugin({ name: row, apply })
    await fiber.await()
    return fiber
  }
  return { root, bus, host, reported, load, signals }
}

test('Cordis listeners register on the bus and leave with their fiber', async () => {
  const sent = []
  const host = { callAsync: (target, method, args) => { sent.push([method, args]); return Promise.resolve() }, call() {}, callFunctionAsync() {}, callFunctionSync() {} }
  const { root, bus, load } = setup({ peer: host })
  const fiber = await load('chat', ctx => {
    ctx.on('session/created', () => {})
    ctx.on('session/created', async () => {}, { prepend: true, global: true })
    ctx.on('internal/service', () => {})
  })
  assert.deepEqual(sent.map(([method, [fields]]) => [method, fields.name, fields.row, fields.plugin, fields.kind, fields.prepend, fields.global]), [
    ['events.on', 'session/created', 'chat', 'chat', 'any', false, false],
    ['events.on', 'session/created', 'chat', 'chat', 'async', true, true],
  ])
  assert.equal(bus.registrations('chat').length, 2)
  await fiber.dispose()
  assert.deepEqual(sent.slice(2).map(([method]) => method), ['events.off', 'events.off'])
  // internal/* stays Cordis's own: registered and dispatched as before.
  const seen = []
  root.on('internal/custom', value => { seen.push(value) })
  root.emit('internal/custom', 1)
  assert.deepEqual(seen, [1])
})

test('the five Cordis methods dispatch through the host, each in its mode', async () => {
  const { root, host, load } = setup()
  await load('a', ctx => {
    ctx.on('s', () => false)
    ctx.on('w', async next => `a${await next()}`)
    ctx.on('b', () => Promise.resolve('x'))
  })
  await load('b', ctx => {
    ctx.on('s', () => 0)
    ctx.on('w', () => 'b')
  })
  root.emit('e', 1)
  assert.equal(await root.serial('s'), 0)
  await root.parallel('p')
  assert.equal(await root.waterfall('w', () => 't'), 'ab')
  // Cordis took a Promise for a result; the runtime refuses it.
  assert.throws(() => root.bail('b'), { name: 'AsyncListener' })
  assert.deepEqual(host.sent.filter(([method]) => method === 'events.dispatch').map(([, [fields]]) => [fields.mode, fields.name]), [
    ['emit', 'e'], ['serial', 's'], ['parallel', 'p'], ['waterfall', 'w'], ['bail', 'b'],
  ])
  assert.throws(() => root.emit('rutis/row', {}), { name: 'ReservedEvent' })
})

test('Context.filter sees the listener Context, which has ctx.rutis', async () => {
  const { root, load } = setup()
  const T1 = { id: '1', group: 'tenant' }
  const log = []
  let chat
  await load('chat', ctx => {
    chat = ctx
    ctx.on('f', () => { log.push('chat') })
  }, [T1])
  // Emitted from chat, in T1: the listeners at the root and in T1 hear it.
  await load('other', ctx => { ctx.on('f', () => { log.push('other') }) })
  await load('global', ctx => { ctx.on('f', () => { log.push('global') }, { global: true }) })
  const self = Object.create(root)
  const seen = []
  self[Context.filter] = context => {
    seen.push(context.rutis)
    return context.rutis.row === 'chat'
  }
  await chat.parallel(self, 'f')
  assert.deepEqual(log, ['chat', 'global'])
  assert.deepEqual(seen, [
    { name: 'f', prefix: false, plugin: 'chat', row: 'chat', runtime: 'node', node: null, scope: [T1], global: false },
    { name: 'f', prefix: false, plugin: 'other', row: 'other', runtime: 'node', node: null, scope: [], global: false },
  ])
  // Outside a filter: the Context's own descriptor.
  assert.equal(chat.rutis.row, 'chat')
  assert.deepEqual(chat.rutis.scope, [T1])
  assert.throws(() => { chat.rutis = {} })
})

// rule: EVP-5.3-15
// A dispatch from elsewhere (`own: false`) fires `internal/dispatch` once for
// its id, before this process's first listener, with the dispatch's mode,
// name and arguments and no `thisArg` (the fourth argument is `null`).
test('a dispatch from elsewhere fires internal/dispatch once, before its first listener here', async () => {
  const { root, bus, host, load } = setup()
  const log = []
  root.on('internal/dispatch', (mode, name, args, thisArg) => { log.push({ mode, name, args, thisArg }) })
  await load('a', ctx => { ctx.on('x', (...args) => { log.push(`a:${args.join(',')}`) }) })
  await load('b', ctx => { ctx.on('x', (...args) => { log.push(`b:${args.join(',')}`) }) })
  const [first, second] = host.entries.filter(entry => entry.name === 'x')
  const call = (entry, dispatch, args) => bus.call({ listener: entry.id, dispatch, own: false, name: 'x', mode: 'parallel', args })
  // Two listeners of this process: one internal/dispatch for the id.
  call(first, 'd1', [1, 2])
  call(second, 'd1', [1, 2])
  assert.deepEqual(log.splice(0), [
    { mode: 'parallel', name: 'x', args: [1, 2], thisArg: null },
    'a:1,2',
    'b:1,2',
  ])
  // Another dispatch id: once more, with its own arguments.
  call(first, 'd2', [3])
  assert.deepEqual(log.splice(0), [
    { mode: 'parallel', name: 'x', args: [3], thisArg: null },
    'a:3',
  ])
  // This process's own dispatch fired it in Cordis already; the call back
  // from the host does not fire it again.
  bus.call({ listener: first.id, dispatch: 'd3', own: true, name: 'x', mode: 'parallel', args: [4] })
  assert.deepEqual(log.splice(0), ['a:4'])
})

// rule: EVP-5.3-16
// An event this process emits through Cordis's own dispatch fires
// `internal/dispatch` here; a listener of it that throws fails that call.
test('this process\'s own dispatch fires internal/dispatch, whose failure fails the call', async () => {
  const { root, load } = setup()
  const seen = []
  root.on('internal/dispatch', (...args) => { seen.push(args) })
  await load('a', ctx => { ctx.on('x', value => value) })
  root.emit('x', 1)
  await root.parallel('x', 2)
  await root.serial('x', 3)
  await root.waterfall('x', 4, () => 'end')
  assert.equal(root.bail('x', 5), 5)
  assert.deepEqual(seen, [
    ['emit', 'x', [1], null],
    ['parallel', 'x', [2], null],
    ['serial', 'x', [3], null],
    ['waterfall', 'x', [4], null],
    ['bail', 'x', [5], null],
  ])
  const refused = Object.assign(new Error('no dispatch'), { name: 'DispatchRefused' })
  root.on('internal/dispatch', () => { throw refused })
  for (const call of [
    () => root.emit('x', 6),
    () => root.parallel('x', 6),
    () => root.serial('x', 6),
    () => root.waterfall('x', 6, () => 'end'),
    () => root.bail('x', 6),
  ]) assert.throws(call, error => error === refused)
})

// rule: EVP-5.3-17
// Cordis's primitive `events.dispatch(type, args)` for a bus name gives one
// callback: `emit` dispatches a parallel, `serial` serial, `bail` bail, with
// the arguments the callback gets; internal/* stays Cordis's own.
test('events.dispatch gives a callback that dispatches in emit\'s, serial\'s and bail\'s mode', async () => {
  const { root, host, load } = setup()
  const seen = []
  await load('a', ctx => {
    ctx.on('s', value => { seen.push(value); return value * 2 })
    ctx.on('internal/kept', value => { seen.push(`internal:${value}`) })
  })
  const modes = [['emit', 'parallel', undefined, 2], ['serial', 'serial', 6, 3], ['bail', 'bail', 8, 4]]
  for (const [type, mode, expected, value] of modes) {
    const callbacks = root.events.dispatch(type, ['s'])
    assert.equal(callbacks.length, 1)
    const result = type === 'bail' ? callbacks[0](value) : await callbacks[0](value)
    assert.equal(result, expected, type)
    const fields = host.sent.filter(([method]) => method === 'events.dispatch').at(-1)[1][0]
    assert.deepEqual([fields.mode, fields.name, fields.args], [mode, 's', [value]], type)
  }
  await host.settled()
  assert.deepEqual(seen, [2, 3, 4])
  const before = host.sent.filter(([method]) => method === 'events.dispatch').length
  const internal = root.events.dispatch('emit', ['internal/kept'])
  assert.equal(internal.length, 1)
  internal[0]('v')
  assert.equal(seen.at(-1), 'internal:v')
  assert.equal(host.sent.filter(([method]) => method === 'events.dispatch').length, before, 'internal/* stays Cordis\'s')
})

// rule: EVD-3.7-1
// A Cordis plugin here keeps using `internal/service`: providing a service
// fires it, the plugin's own listener hears it, and it never becomes an
// `events.on` on the host.
test('internal/service stays Cordis\'s own, and a service provided here fires it', async () => {
  const { root, host, load } = setup()
  const seen = []
  await load('a', ctx => {
    ctx.on('internal/service', (name, value) => { seen.push([name, typeof value]) })
    ctx.provide('own', { ping: () => 'pong' })
  })
  assert.ok(seen.some(([name]) => name === 'own'), `heard internal/service: ${JSON.stringify(seen)}`)
  assert.equal(root.get('own').ping(), 'pong')
  assert.ok(
    !host.sent.some(([method, [fields]]) => method === 'events.on' && String(fields?.name).startsWith('internal/')),
    'internal/* stays out of the bus',
  )
})

// rule: EVD-3.7-1
test('internal/dispatch is fired once for each dispatch, before the listeners', async () => {
  const { root, bus, host, load } = setup()
  const log = []
  root.on('internal/dispatch', (mode, name) => { log.push(`dispatch:${mode}:${name}`) })
  await load('a', ctx => { ctx.on('x', () => { log.push('x') }) })
  await load('b', ctx => { ctx.on('x', () => { log.push('y') }) })
  await root.serial('x')
  assert.deepEqual(log.splice(0), ['dispatch:serial:x', 'x', 'y'])
  const ids = host.entries.filter(entry => entry.name === 'x').map(entry => entry.id)
  // From elsewhere: once per dispatch id, however many listeners run here;
  // never for this process's own dispatch, which fired it already.
  for (const listener of ids) bus.call({ listener, dispatch: 'd1', own: false, name: 'x', mode: 'serial', args: [] })
  bus.call({ listener: ids[0], dispatch: 'd2', own: true, name: 'x', mode: 'serial', args: [] })
  assert.deepEqual(log, ['dispatch:serial:x', 'x', 'y', 'x'])
})

test('leaf plugins: on with prepend and global, the five methods, filter, signal, reserved names', async () => {
  const { root, load, signals } = setup()
  const log = []
  let leaf
  const spec = definePlugin({
    apply(ctx) {
      leaf = ctx
      const off = ctx.on('o', () => { log.push('later') })
      ctx.on('o', () => { log.push('first') }, { prepend: true })
      ctx.on('g', () => { log.push('global') }, { global: true })
      ctx.on('w', async (x, next) => (await next()) + x)
      ctx.on('rutis/row', () => {})
      assert.equal(typeof off, 'function')
    },
  })
  const plugin = supported(toCordis(spec), 'leaf')
  await load('leaf', plugin.apply)
  await leaf.serial('o')
  assert.deepEqual(log.splice(0), ['first', 'later'])
  assert.equal(await leaf.parallel('o'), undefined)
  // Every argument is the event's; the terminal comes with `with`.
  assert.equal(await leaf.waterfall.with(() => 'end')('w', '!'), 'end!')
  assert.equal(await leaf.waterfall('w', '!'), 'undefined!')
  assert.equal(await leaf.filter(() => true).waterfall.with(() => 'f')('w', '?'), 'f?')
  assert.throws(() => leaf.waterfall.with('end'), TypeError)
  assert.equal(leaf.bail('nothing'), undefined)
  log.length = 0
  await leaf.filter(() => false).parallel('g')
  await leaf.filter(() => false).parallel('o')
  assert.deepEqual(log, ['global'])
  // A predicate that answers with a Promise keeps nothing: the dispatch
  // fails instead of treating it as true (#637).
  await assert.rejects(leaf.filter(async () => true).parallel('o'), {
    name: 'TypeError',
    message: 'a filter predicate must answer with a boolean, not a Promise',
  })
  assert.equal(leaf.signal, signals.get('leaf').signal)
  assert.throws(() => leaf.on('internal/dispatch', () => {}), /is internal to the runtime/)
  assert.throws(() => leaf.emit('internal/service'), /is internal to the runtime/)
  assert.throws(() => leaf.emit(null, 'internal/service'), TypeError)
  assert.throws(() => leaf.emit('rutis/fake'), { name: 'ReservedEvent' })
  assert.throws(() => leaf.serial('rutis/fake'), { name: 'ReservedEvent' })
  root.emit('nothing')
})

// Descriptors name a plugin by its declared name: Cordis's own, a leaf
// plugin's `name`, else (a leaf plugin without one) its row's id.
test('a leaf plugin without a name is named by its row\'s id', async () => {
  const { root, bus, host, load } = setup()
  bus.nameRow('anon', 'anon-id')
  const named = definePlugin({ name: 'greeter', apply(ctx) { ctx.on('p', () => {}) } })
  const unnamed = definePlugin({ apply(ctx) { ctx.on('p', () => {}) } })
  await load('named', supported(toCordis(named), 'named').apply)
  await load('anon', supported(toCordis(unnamed), 'anon').apply)
  await load('cordis', ctx => { ctx.on('p', () => {}) })
  assert.deepEqual(host.entries.filter(entry => entry.name === 'p').map(entry => entry.plugin), ['greeter', 'anon-id', 'cordis'])
  root.emit('nothing')
})

// Cordis's `emit` calls the callbacks of `events.dispatch('emit')` without
// awaiting them: a failure is reported, not an unhandled rejection that
// ends the process. `parallel` awaits them and gets the failure itself.
test('a failure of events.dispatch(\'emit\') nobody awaits is reported', async () => {
  const { root, load, reported } = setup()
  const unhandled = []
  const onUnhandled = reason => unhandled.push(reason)
  process.on('unhandledRejection', onUnhandled)
  try {
    await load('a', ctx => { ctx.on('d', () => { throw new Error('a failed') }) })
    for (const callback of root.events.dispatch('emit', ['d'])) callback()
    // The parallel the host dispatched failed; this process reports it as
    // the emit's failure.
    await eventually("the emit's failure to be reported", () => reported.length > 0)
    assert.deepEqual(reported, ['rutis: emit of d from the mount failed: AggregateError: 1 listeners of d failed'])
    reported.length = 0
    const results = await Promise.allSettled(root.events.dispatch('emit', ['d']).map(async callback => callback()))
    assert.equal(results[0].status, 'rejected')
    assert.equal(results[0].reason.name, 'AggregateError')
    // clock: weak-negative — reported once: an awaited caller has the failure itself
    await new Promise(resolve => setTimeout(resolve, 10))
    assert.deepEqual(reported, [], 'awaited: the caller has the failure')
    assert.deepEqual(unhandled, [])
  } finally {
    process.off('unhandledRejection', onUnhandled)
  }
})

// Through the host the filter gets each listener's own Context by the id
// the host gives: one plugin listening from two Contexts (two child
// fibers) has two equal descriptors.
test('through the host, Context.filter gets the Context the host asks about', async () => {
  const sent = []
  const host = { callAsync: (target, method, args) => { sent.push([method, args]); return Promise.resolve() }, call() {}, callFunctionAsync() {}, callFunctionSync() {} }
  const { root, bus, load } = setup({ peer: host })
  const contexts = []
  await load('chat', ctx => {
    for (const label of ['one', 'two']) {
      ctx.plugin({ name: 'child', apply(child) { contexts.push(child); child.on('f', () => {}) } })
    }
  })
  const ids = sent.filter(([method]) => method === 'events.on').map(([, [fields]]) => fields.id)
  assert.equal(ids.length, 2)
  const self = Object.create(root)
  const seen = []
  self[Context.filter] = context => { seen.push(context); return context === contexts[1] }
  root.parallel(self, 'f')
  const [, [fields]] = sent.at(-1)
  const descriptor = { name: 'f', prefix: false, plugin: 'child', row: 'chat', runtime: 'node', node: null, scope: [], global: false }
  assert.deepEqual(fields.filter([{ ...descriptor, listener: ids[0] }, { ...descriptor, listener: ids[1] }]), [false, true])
  assert.equal(seen[0], contexts[0])
  assert.equal(seen[1], contexts[1])
})

// Some plugins call Cordis's primitive themselves (`ctx.events.dispatch`,
// then each callback, as dsh-credentials fans its events out): with the
// listeners on the bus, it gives one callback that dispatches through the
// host, which calls this process's listener back.
test('events.dispatch gives one callback that dispatches through the host', async () => {
  const { root, host, load } = setup()
  const seen = []
  await load('chat', ctx => { ctx.on('credentials/updated', value => { seen.push(value) }) })
  const callbacks = root.events.dispatch('emit', ['credentials/updated', 'b'])
  assert.equal(callbacks.length, 1)
  await callbacks[0]('b')
  const dispatched = host.sent.filter(([method]) => method === 'events.dispatch').map(([, [fields]]) => [fields.name, fields.mode, fields.args])
  assert.deepEqual(dispatched, [['credentials/updated', 'parallel', ['b']]])
  assert.deepEqual(seen, ['b'], 'called back by the host, once')
})

// rule: EVP-6-12
// `thisArg[Context.filter]` filters `emit` and the other four methods; a
// listener here passes its own Cordis Context, one elsewhere `{ rutis }`.
test('Context.filter filters every Cordis method, with the Context or { rutis }', async () => {
  const modes = []
  let localId
  const descriptor = row => ({ name: 'm', prefix: false, plugin: row === 'chat' ? 'chat' : 'other', row, runtime: 'node', node: null, scope: [], global: false })
  const contexts = []
  const handle = (method, args) => {
    const [fields] = args ?? []
    if (method === 'events.on') { localId = fields.id; return undefined }
    if (method !== 'events.dispatch') return undefined
    modes.push(fields.mode)
    if (fields.filter) contexts.push(fields.filter([{ ...descriptor('chat'), listener: localId }, descriptor('other')]))
    return undefined
  }
  const host = {
    call: (target, method, args) => handle(method, args),
    callAsync: (target, method, args) => Promise.resolve(handle(method, args)),
    callFunctionAsync() {}, callFunctionSync() {},
  }
  const { root, load } = setup({ peer: host })
  let chat
  await load('chat', ctx => { chat = ctx; ctx.on('m', () => 'mine') })
  const seen = []
  const self = Object.create(root)
  self[Context.filter] = function (context) {
    assert.equal(this, self)
    seen.push(context)
    return context.rutis.row === 'chat'
  }
  root.emit(self, 'm')
  await root.parallel(self, 'm')
  await root.serial(self, 'm')
  assert.equal(root.waterfall(self, 'm', () => 'end'), undefined)
  assert.equal(root.bail(self, 'm'), undefined)
  assert.deepEqual(modes, ['emit', 'parallel', 'serial', 'waterfall', 'bail'])
  assert.deepEqual(contexts, Array.from({ length: 5 }, () => [true, false]))
  assert.equal(seen.length, 10)
  for (let i = 0; i < seen.length; i += 2) {
    assert.equal(seen[i], chat)
    assert.deepEqual(seen[i + 1], { rutis: descriptor('other') })
  }
})

// rule: EVP-6-13
// Every Context (the root's, a plugin's) has a read-only `ctx.rutis`, and
// `c => c.rutis.row === …` works as a filter for a Context here and a
// `{ rutis }` from elsewhere alike.
test('every Context has a read-only ctx.rutis, and c.rutis.row names its row', async () => {
  const { root, host, load } = setup()
  let child
  root.on('x', () => {})
  await load('chat', ctx => { child = ctx; ctx.on('x', () => {}) })
  for (const [context, row] of [[root, null], [child, 'chat']]) {
    assert.equal(context.rutis.row, row)
    assert.equal(context.rutis.runtime, 'node')
    assert.throws(() => { context.rutis = {} }, TypeError)
  }
  const rows = []
  const self = Object.create(root)
  self[Context.filter] = context => { rows.push(context.rutis.row); return true }
  await root.parallel(self, 'x')
  assert.deepEqual(rows, [null, 'chat'])
  // The host asks the same predicate about a listener elsewhere: it gets a
  // `{ rutis }`, whose row `c.rutis.row` reads too.
  const fields = host.sent.filter(([method]) => method === 'events.dispatch').at(-1)[1][0]
  const other = { name: 'x', prefix: false, plugin: 'other', row: 'other', runtime: 'node', node: null, scope: [], global: false }
  assert.deepEqual(fields.filter([other]), [true])
  assert.deepEqual(rows, [null, 'chat', 'other'])
})
