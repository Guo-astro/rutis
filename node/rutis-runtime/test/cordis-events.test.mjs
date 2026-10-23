// guarantee: events/cordis — Cordis plugins on the logical bus (#464):
// `internal/listener` registers every listener that is not internal/*,
// Cordis's five methods dispatch by the bus's rules, `Context.filter` sees
// the real Context here and `{ rutis }` elsewhere, `ctx.rutis` on every
// Context, `internal/dispatch` fired once, internal/* stays Cordis's.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Context } from '@deepseek-ai/cordis'
import { Bus } from '../src/events.mjs'
import { install } from '../src/cordis-events.mjs'
import { definePlugin } from '../../rutis/src/index.mjs'
import { supported, toCordis } from '../src/leaf.mjs'

// A root Context on a bus without a host; plugins are rows by name.
function setup(options = {}) {
  const root = new Context()
  const reported = []
  const bus = new Bus({ runtime: 'node', report: line => reported.push(line), ...options })
  const scopes = new Map()
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
  return { root, bus, reported, load, signals }
}

test('Cordis listeners register on the bus and leave with their fiber', async () => {
  const sent = []
  const host = { callAsync: (target, method, args) => { sent.push([method, args]); return Promise.resolve() }, call() {}, callFunctionAsync() {} }
  const { root, bus, load } = setup()
  bus.peer = host
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

test('the five Cordis methods dispatch by the bus rules', async () => {
  const { root, load, reported } = setup()
  const log = []
  await load('a', ctx => {
    ctx.on('e', () => { log.push('a'); throw new Error('a failed') })
    ctx.on('s', () => false)
    ctx.on('w', async next => `a${await next()}`)
    ctx.on('b', () => Promise.resolve('x'))
  })
  await load('b', ctx => {
    ctx.on('e', () => { log.push('b') })
    ctx.on('s', () => 0)
    ctx.on('w', () => 'b')
  })
  // emit: a failing listener stops nothing and reaches nobody but the report.
  root.emit('e')
  assert.deepEqual(log, ['a', 'b'])
  assert.deepEqual(reported, ['rutis: listener of e in row a failed: Error: a failed'])
  assert.equal(await root.serial('s'), 0)
  assert.equal(await root.waterfall('w', () => 't'), 'ab')
  // Cordis took a Promise for a result; the bus refuses it.
  assert.throws(() => root.bail('b'), { name: 'AsyncListener' })
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

test('internal/dispatch is fired once for each dispatch, before the listeners', async () => {
  const { root, bus, load } = setup()
  const ids = []
  bus.peer = { callAsync: (target, method, [fields]) => { if (method === 'events.on') ids.push(fields.id); return Promise.resolve() }, call() {}, callFunctionAsync() {} }
  const log = []
  root.on('internal/dispatch', (mode, name) => { log.push(`dispatch:${mode}:${name}`) })
  await load('a', ctx => { ctx.on('x', () => { log.push('x') }) })
  await load('b', ctx => { ctx.on('x', () => { log.push('y') }) })
  await root.serial('x')
  assert.deepEqual(log.splice(0), ['dispatch:serial:x', 'x', 'y'])
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
      ctx.on('w', (x, next) => next() + x)
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
  assert.equal(leaf.waterfall.with(() => 'end')('w', '!'), 'end!')
  assert.equal(leaf.waterfall('w', '!'), 'undefined!')
  assert.equal(leaf.filter(() => true).waterfall.with(() => 'f')('w', '?'), 'f?')
  assert.throws(() => leaf.waterfall.with('end'), TypeError)
  assert.equal(leaf.bail('nothing'), undefined)
  log.length = 0
  await leaf.filter(() => false).parallel('g')
  await leaf.filter(() => false).parallel('o')
  assert.deepEqual(log, ['global'])
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
  const { root, bus, load } = setup()
  bus.nameRow('anon', 'anon-id')
  const named = definePlugin({ name: 'greeter', apply(ctx) { ctx.on('p', () => {}) } })
  const unnamed = definePlugin({ apply(ctx) { ctx.on('p', () => {}) } })
  await load('named', supported(toCordis(named), 'named').apply)
  await load('anon', supported(toCordis(unnamed), 'anon').apply)
  await load('cordis', ctx => { ctx.on('p', () => {}) })
  assert.deepEqual(bus.entries().filter(entry => entry.name === 'p').map(entry => entry.plugin), ['greeter', 'anon-id', 'cordis'])
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
    await new Promise(resolve => setTimeout(resolve, 10))
    assert.deepEqual(reported, ['rutis: listener of d in row a failed: Error: a failed'])
    reported.length = 0
    const results = await Promise.allSettled(root.events.dispatch('emit', ['d']).map(async callback => callback()))
    assert.equal(results[0].status, 'rejected')
    assert.equal(results[0].reason.name, 'AggregateError')
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
  const host = { callAsync: (target, method, args) => { sent.push([method, args]); return Promise.resolve() }, call() {}, callFunctionAsync() {} }
  const { root, bus, load } = setup()
  bus.peer = host
  const contexts = []
  await load('chat', ctx => {
    for (const label of ['one', 'two']) {
      ctx.plugin({ name: 'child', apply(child) { contexts.push(child); child.on('f', () => {}) } })
    }
  })
  const ids = sent.filter(([method]) => method === 'events.on').map(([, [fields]]) => fields.id)
  assert.equal(ids.length, 2)
  bus.elsewhere({ add: ['f'] })
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
// listeners on the bus, here or elsewhere, it gives one callback that
// dispatches on the bus.
test('events.dispatch dispatches on the bus, here or through the host', async () => {
  const sent = []
  const host = { callAsync: (target, method, args) => { sent.push([method, args]); return Promise.resolve() }, call() {}, callFunctionAsync() {} }
  const { root, bus, load } = setup()
  const seen = []
  await load('chat', ctx => { ctx.on('credentials/updated', value => { seen.push(value) }) })
  // Only here: one callback, dispatching on the bus here.
  for (const callback of root.events.dispatch('emit', ['credentials/updated', 'a'])) await callback('a')
  assert.deepEqual(seen, ['a'])
  assert.deepEqual(sent, [])
  bus.peer = host
  bus.elsewhere({ add: ['credentials/updated'] })
  const callbacks = root.events.dispatch('emit', ['credentials/updated', 'b'])
  assert.equal(callbacks.length, 1)
  await callbacks[0]('b')
  const dispatched = sent.filter(([method]) => method === 'events.dispatch').map(([, [fields]]) => [fields.name, fields.mode, fields.args])
  assert.deepEqual(dispatched, [['credentials/updated', 'parallel', ['b']]])
  assert.deepEqual(seen, ['a'], 'this process\'s listener is called through the host, not twice')
})

test('the Events plugin refuses the options removed in rutis 0.13', async () => {
  const { Events } = await import('../src/bridge/features.mjs')
  assert.throws(() => Events.apply({}, { peer: 'main', out: ['tock'] }), {
    message: 'the Events options out / in were removed in rutis 0.13: use export / import (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)',
  })
  assert.throws(() => Events.apply({}, { peer: 'main', in: ['tick'] }), { message: /the Events options out \/ in were removed/ })
  assert.throws(() => Events.apply({}, { peer: 'main', events: { out: ['tock'] } }), { message: /the peer config field events was removed/ })
})

// guarantee: events/node-links — a node's announcements carry the offer
// they were made under (`<epoch>.<n>`): the peer's restarted offer of
// `events` replaces what it announced before, a late announcement of an
// older offer is not taken, and this end announces under its own offer.
test('the Events plugin announces and accepts entries by offer', async () => {
  const { Events, bus } = await import('../src/bridge/features.mjs')
  const root = new Context()
  const sent = []
  const offers = new Set()
  let handler, families = new Set(), since = new Map()
  root.provide('rutisPeer.main', {
    id: 'main',
    epoch: family => family === 'events' && handler ? 7 : undefined,
    register(family, served) { handler = served; return () => { handler = undefined } },
    onOffers(listener) {
      offers.add(listener)
      listener(families, family => families.has(family) ? since.get(family) : undefined)
      return () => offers.delete(listener)
    },
    callAsync: async (_target, method, args) => { sent.push([method, args[0]]) },
    call: () => undefined,
  })
  const offer = epoch => {
    families = new Set(['events'])
    since = new Map([['events', epoch]])
    for (const listener of offers) listener(families, family => families.has(family) ? since.get(family) : undefined)
  }
  const events = bus(root)
  root.plugin(Events, { peer: 'main', export: ['t/*'], import: ['t/*'] })
  root.on('t/mine', () => {})
  await new Promise(resolve => setTimeout(resolve, 0))
  assert.ok(handler, 'the events family is served')
  const remote = () => events.entries().filter(entry => entry.remote).map(entry => entry.name)

  offer(1)
  assert.deepEqual(sent.filter(([method]) => method === 'events.on').map(([, a]) => a.id), ['7.1'], 'announced under this end\'s offer')
  handler('', 'events.on', [{ id: '1.1', name: 't/a', kind: 'any' }])
  assert.deepEqual(remote(), ['t/a'])
  offer(2)
  assert.deepEqual(remote(), [], 'the restarted offer replaces the old entries')
  handler('', 'events.on', [{ id: '1.2', name: 't/late', kind: 'any' }])
  assert.deepEqual(remote(), [], 'a late announcement of an older offer is not taken')
  handler('', 'events.on', [{ id: '2.1', name: 't/b', kind: 'any' }])
  assert.deepEqual(remote(), ['t/b'])
  handler('', 'events.on', [{ id: '3.1', name: 't/c', kind: 'any' }])
  assert.deepEqual(remote(), ['t/c'], 'an announcement of a newer offer, before the offer itself, replaces the older ones')
})

// guarantee: events/node-links — the peer's announcement can come before
// its offer of `events` (#530): offers that do not show that offer yet do
// not withdraw its entry; once the offer shown is withdrawn, it goes.
test('the Events plugin keeps an entry announced before its offer', async () => {
  const { Events, bus } = await import('../src/bridge/features.mjs')
  const root = new Context()
  const offers = new Set()
  let handler, families = new Set(), since = new Map()
  root.provide('rutisPeer.main', {
    id: 'main',
    epoch: family => family === 'events' && handler ? 7 : undefined,
    register(family, served) { handler = served; return () => { handler = undefined } },
    onOffers(listener) {
      offers.add(listener)
      listener(families, family => families.has(family) ? since.get(family) : undefined)
      return () => offers.delete(listener)
    },
    callAsync: async () => {},
    call: () => undefined,
  })
  const offer = epoch => {
    families = new Set(epoch === undefined ? ['services'] : ['services', 'events'])
    since = new Map([['services', 1], ['events', epoch]])
    for (const listener of offers) listener(families, family => families.has(family) ? since.get(family) : undefined)
  }
  const events = bus(root)
  root.plugin(Events, { peer: 'main', export: ['t/*'], import: ['t/*'] })
  await new Promise(resolve => setTimeout(resolve, 0))
  assert.ok(handler, 'the events family is served')
  const remote = () => events.entries().filter(entry => entry.remote).map(entry => entry.name)

  handler('', 'events.on', [{ id: '3.1', name: 't/a', kind: 'any' }])
  offer(undefined)
  assert.deepEqual(remote(), ['t/a'], 'offers not showing events yet withdraw nothing')
  offer(3)
  assert.deepEqual(remote(), ['t/a'])
  offer(undefined)
  assert.deepEqual(remote(), [], 'the offer shown is withdrawn, and its entry with it')
  handler('', 'events.on', [{ id: '5.1', name: 't/b', kind: 'any' }])
  offer(undefined)
  assert.deepEqual(remote(), ['t/b'], 'an entry of a later offer, come before it, stays')
})
