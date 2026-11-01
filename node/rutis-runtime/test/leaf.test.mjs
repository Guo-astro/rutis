// guarantee: sdk/leaf-events (shared bus, cleanup, listener errors, internal/* refused, API 2)
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Context } from '@deepseek-ai/cordis'
import { definePlugin } from '../../rutis/src/index.mjs'
import { supported, toCordis } from '../src/leaf.mjs'
import { Bus } from '../src/events.mjs'
import { install } from '../src/cordis-events.mjs'
import { fakePeer } from './fixtures/fake-peer.mjs'

test('leaf events delegate to the shared Cordis bus and leave with their fiber', async () => {
  // The runtime's logical bus on the root, as the runner installs it; its
  // host dispatches every emission and calls the listeners back.
  const root = new Context()
  const reported = []
  const host = fakePeer()
  const bus = new Bus({ runtime: 'node', peer: host, report: line => reported.push(line) })
  host.connect(bus)
  install(root, Context, bus, () => ({ row: null, scope: [], signal: undefined }))
  const received = []
  const seen = []
  let leaf
  root.on('leaf-output', value => { received.push(['root', value]) })
  const spec = definePlugin({
    apply(ctx) {
      leaf = ctx
      ctx.on('root-event', value => { received.push(['leaf', value]) })
      ctx.emit('leaf-output', 1)
      ctx.on('e', () => { seen.push(1); throw new Error('sync failure') })
      ctx.on('e', () => { seen.push(2) })
    },
  })
  const plugin = root.plugin(toCordis(supported(spec, 'events')))
  await plugin.await()
  assert.throws(() => leaf.on('internal/dispatch', () => {}), /is internal to the runtime/)
  assert.throws(() => leaf.emit('internal/service'), /is internal to the runtime/)
  // A non-string name is not Cordis's `this` argument: `emit(null, 'internal/service')` does not dispatch.
  assert.throws(() => leaf.on(null, () => {}), TypeError)
  assert.throws(() => leaf.emit(null, 'internal/service'), TypeError)
  assert.throws(() => leaf.emit({}, 'internal/service'), TypeError)
  // emit never throws: every listener runs, the failure goes to the host,
  // which reports it (§5.6).
  assert.equal(leaf.emit('e'), undefined)
  await host.settled()
  assert.deepEqual(seen, [1, 2])
  assert.deepEqual(host.failures.map(error => error.message), ['sync failure'])
  assert.deepEqual(reported, [])
  await root.parallel('root-event', 2)
  assert.deepEqual(received, [['root', 1], ['leaf', 2]])
  await plugin.dispose()
  await root.parallel('root-event', 3)
  assert.deepEqual(received, [['root', 1], ['leaf', 2]], 'unload removes the leaf listener')
  assert.ok(!host.entries.some(entry => entry.name === 'root-event'), 'and the host forgets it')
})

// guarantee: sdk/leaf-events (failed load and update)
test('a failed apply and an update remove the leaf listeners, as the runner drives the fiber', async () => {
  const root = new Context()
  const host = fakePeer()
  const bus = new Bus({ runtime: 'node', peer: host, report: () => {} })
  host.connect(bus)
  install(root, Context, bus, () => ({ row: null, scope: [], signal: undefined }))
  const seen = []
  const spec = definePlugin({
    apply(ctx, config) {
      ctx.on('tick', () => { seen.push(config.label) })
      if (config.fail) throw new Error('apply failed')
    },
  })
  const plugin = root.plugin(toCordis(supported(spec, 'events')), { label: 'first' })
  await plugin.await()
  // rows.update: the runner restarts the fiber with the new config.
  plugin.update({ label: 'updated' }, true)
  await plugin.await()
  const failed = root.plugin(toCordis(supported(spec, 'events')), { label: 'failed', fail: true })
  await assert.rejects(failed.await(), /apply failed/)
  await root.parallel('tick')
  assert.deepEqual(seen, ['updated'])
  assert.equal(host.entries.filter(entry => entry.name === 'tick').length, 1, 'the host has the updated listener only')
  await failed.dispose()
  await plugin.dispose()
})

// A service neither the plugin nor its row declares in `inject`, and that
// is not there for it: `use` says what to declare, not that the service
// does not exist (#676). One the row's own `inject` declares is missing.
// guarantee: sdk/undeclared-use
test('a leaf plugin using a service it does not inject is told to declare it', async () => {
  const root = new Context()
  const errors = {}
  const spec = definePlugin({
    apply(ctx) {
      for (const name of ['tsvc', 'extra']) {
        try { ctx.use(name) } catch (caught) { errors[name] = caught.message; errors[`${name} code`] = [caught.name, caught.code, caught.data] }
      }
    },
  })
  const plugin = root.plugin(toCordis(supported(spec, 'sneaky'), ['extra']))
  await plugin.await()
  assert.equal(errors.tsvc, "service tsvc is not available to this plugin, which does not declare it in inject: add 'tsvc' to its inject")
  assert.equal(errors.extra, 'service extra is not available')
  // With a code and data for programs (§4.3).
  assert.deepEqual(errors['tsvc code'], ['BindingError', 'not-declared', { service: 'tsvc' }])
  assert.deepEqual(errors['extra code'], ['BindingError', 'unavailable', { service: 'extra' }])
  await plugin.dispose()
})

// An optional service: `get` gives it when there and undefined when not;
// `use` reads only what `inject` lists, as Rust's `require` (an optional
// one: add it to `inject`); `get` of a name the plugin declares nowhere
// says to add it to `optional`.
// guarantee: sdk/optional-deps
test('a leaf plugin reads its optional services with get', async () => {
  const root = new Context()
  root.provide('present')
  root.present = { hello: () => 'hi' }
  const seen = {}
  const spec = definePlugin({
    optional: ['present', 'absent'],
    apply(ctx) {
      seen.present = ctx.get('present')?.hello()
      seen.absent = ctx.get('absent')
      seen.extra = ctx.get('extra')
      for (const [key, run] of [['use present', () => ctx.use('present')], ['use absent', () => ctx.use('absent')], ['get undeclared', () => ctx.get('nope')]]) {
        try { run() } catch (caught) { seen[key] = [caught.message, caught.code, caught.data] }
      }
    },
  })
  const plugin = root.plugin(toCordis(supported(spec, 'optional'), ['extra']))
  await plugin.await()
  assert.equal(seen.present, 'hi')
  assert.equal(seen.absent, undefined)
  assert.equal(seen.extra, undefined, "the row's inject counts as declared")
  assert.deepEqual(seen['use present'], ["service present is not available to this plugin, which does not declare it in inject: add 'present' to its inject", 'not-declared', { service: 'present' }])
  assert.deepEqual(seen['use absent'], ["service absent is not available to this plugin, which does not declare it in inject: add 'absent' to its inject", 'not-declared', { service: 'absent' }])
  assert.deepEqual(seen['get undeclared'], ["service nope is not available to this plugin, which does not declare it: add 'nope' to its optional (or to its inject, if it cannot run without it)", 'not-declared', { service: 'nope' }])
  await plugin.dispose()
})

// A row's own `inject` counts as declared; a name declared nowhere is
// refused even when another plugin here provides it (#676): the answer
// does not depend on who else is in the process.
// guarantee: sdk/optional-deps
test('a leaf plugin uses what it or its row declares, not what a neighbour provides', async () => {
  const root = new Context()
  root.provide('row')
  root.row = { hello: () => 'hi' }
  root.provide('neighbour')
  root.neighbour = { hello: () => 'hi' }
  const seen = {}
  const spec = definePlugin({
    apply(ctx) {
      // The row's `inject`: declared for this plugin.
      seen.use = ctx.use('row').hello()
      seen.get = ctx.get('row').hello()
      for (const [key, call] of [['neighbour use', () => ctx.use('neighbour')], ['neighbour get', () => ctx.get('neighbour')]]) {
        try { seen[key] = call().hello() } catch (caught) { seen[key] = [caught.message, caught.code, caught.data] }
      }
    },
  })
  const plugin = root.plugin(toCordis(supported(spec, 'declared'), ['row']))
  await plugin.await()
  assert.equal(seen.use, 'hi')
  assert.equal(seen.get, 'hi')
  // Declared nowhere, though this process provides it: say what to declare.
  assert.deepEqual(seen['neighbour use'], ["service neighbour is not available to this plugin, which does not declare it in inject: add 'neighbour' to its inject", 'not-declared', { service: 'neighbour' }])
  assert.deepEqual(seen['neighbour get'], ["service neighbour is not available to this plugin, which does not declare it: add 'neighbour' to its optional (or to its inject, if it cannot run without it)", 'not-declared', { service: 'neighbour' }])
  await plugin.dispose()
})

// A plugin reads what it provides itself: declared by providing it, for
// `use` and `get` alike (as in every other runtime). Withdrawn, it is
// declared no more, as a name the plugin never declared.
// guarantee: sdk/optional-deps
test('a leaf plugin may use and get a service it provides itself', async () => {
  const root = new Context()
  const seen = {}
  const spec = definePlugin({
    async apply(ctx) {
      const withdraw = ctx.provide('own', { hello: () => 'hi' })
      seen.use = ctx.use('own').hello()
      seen.get = ctx.get('own').hello()
      // Names Cordis answers itself as a Context property: still the object.
      for (const name of ['_u', 'fiber']) {
        const object = { name }
        ctx.provide(name, object)
        seen[name] = ctx.use(name) === object && ctx.get(name) === object
      }
      // Withdrawn, it is undeclared at once, before Cordis finishes.
      const withdrawn = withdraw()
      for (const [key, call] of [['use withdrawn', () => ctx.use('own')], ['get withdrawn', () => ctx.get('own')]]) {
        try { seen[key] = call() } catch (caught) { seen[key] = [caught.message, caught.code] }
      }
      await withdrawn
      // Withdrawn and provided again in one turn: the first withdrawal,
      // finishing later, leaves the second provide in place.
      ctx.provide('again', { v: 1 })()
      ctx.provide('again', { v: 2 })
      await new Promise(resolve => setTimeout(resolve, 20))
      seen.again = ctx.use('again').v
    },
  })
  const plugin = root.plugin(toCordis(supported(spec, 'own')))
  await plugin.await()
  assert.equal(seen.use, 'hi')
  assert.equal(seen.get, 'hi')
  assert.equal(seen._u, true)
  assert.equal(seen.fiber, true)
  assert.deepEqual(seen['use withdrawn'], ["service own is not available to this plugin, which does not declare it in inject: add 'own' to its inject", 'not-declared'])
  assert.deepEqual(seen['get withdrawn'], ["service own is not available to this plugin, which does not declare it: add 'own' to its optional (or to its inject, if it cannot run without it)", 'not-declared'])
  assert.equal(seen.again, 2)
  await plugin.dispose()
})
