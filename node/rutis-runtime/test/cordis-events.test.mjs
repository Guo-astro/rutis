// guarantee: events/cordis — Cordis plugins on the logical bus (#464):
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
  const host = { callAsync: (target, method, args) => { sent.push([method, args]); return Promise.resolve() }, call() {}, callFunctionAsync() {} }
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
    await new Promise(resolve => setTimeout(resolve, 10))
    // The parallel the host dispatched failed; this process reports it as
    // the emit's failure.
    assert.deepEqual(reported, ['rutis: emit of d from the mount failed: AggregateError: 1 listeners of d failed'])
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
