// guarantee: sdk/leaf-events — the testing tool's events (#464): several
// rows on one bus, scopes, filter with descriptors, prepend and global,
// emit's reported failures, unload signalling and waiting.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '../src/index.mjs'
import { Bus } from '../src/events.mjs'
import { host, load } from '../src/testing.mjs'

const listener = (name, reply, options) => definePlugin({
  name: `listens-${name}`,
  apply(ctx) { ctx.on(name, (...args) => reply(ctx, ...args), options) },
})

test('rows share one bus; each hears its own scope and those under it', async () => {
  const h = host({ scopes: { t1: 'tenant', 't1/s1': 'session' } })
  const heard = []
  for (const [row, scope] of [['root', undefined], ['t1', 't1'], ['s1', 't1/s1'], ['t2', 't2']]) {
    await h.load(listener('hello', () => { heard.push(row) }), { row, scope })
  }
  await h.at('t1/s1').parallel('hello')
  assert.deepEqual(heard.splice(0), ['root', 't1', 's1'])
  await h.at('t2').parallel('hello')
  assert.deepEqual(heard.splice(0), ['root', 't2'])
  await h.parallel('hello')
  assert.deepEqual(heard.splice(0), ['root'])
  await h.unload()
})

test('a plugin emits to the others with the five methods and filters on descriptors', async () => {
  const h = host({ scopes: { t1: 'tenant' } })
  let emitter
  await h.load(definePlugin({ apply(ctx) { emitter = ctx } }), { row: 'e', scope: 't1' })
  await h.load(listener('ask', () => null), { row: 'a' })
  await h.load(listener('ask', (_ctx, city) => `sunny in ${city}`), { row: 'b', scope: 't1' })
  await h.load(listener('ask', () => 'global', { global: true }), { row: 'g' })
  assert.equal(await emitter.serial('ask', 'Oslo'), 'sunny in Oslo')
  const seen = []
  const filtered = emitter.filter(descriptor => { seen.push(descriptor); return descriptor.row === 'a' })
  assert.equal(await filtered.serial('ask', 'Oslo'), 'global')
  assert.deepEqual(seen.map(d => [d.row, d.plugin, d.scope]), [['a', 'listens-ask', []], ['b', 'listens-ask', [{ id: 't1', group: 'tenant' }]]])
  assert.equal(emitter.bail('ask', 'Oslo'), 'sunny in Oslo')
  await h.load(listener('wrap', (_ctx, x, next) => `<${next()}>`, { prepend: true }), { row: 'w' })
  assert.equal(emitter.waterfall('wrap', 1, x => `t${x}`), '<t1>')
  await h.unload()
})

test('emit returns at once and reports a failing listener; the others run', async () => {
  const t = await load(definePlugin({
    apply(ctx) {
      ctx.on('tick', () => { throw Object.assign(new Error('boom'), { name: 'Boom' }) })
      ctx.on('tick', () => { ctx.provide('ticked', { yes() { return true } }) })
    },
  }))
  assert.equal(t.emit('tick'), undefined)
  assert.equal(await t.service('ticked').yes(), true)
  assert.deepEqual(t.reported.map(report => [report.event, report.row, report.error.name, report.line]), [
    ['tick', t.row, 'Boom', `rutis: listener of tick in row ${t.row} failed: Boom: boom`],
  ])
  await t.unload()
})

test('unload aborts ctx.signal and waits for running listeners, at most the deadline', async () => {
  const order = []
  const waits = definePlugin({
    apply(ctx) {
      ctx.on('work', () => new Promise(resolve => ctx.signal.addEventListener('abort', () => { order.push('aborted'); setTimeout(resolve, 20) })).then(() => order.push('finished')))
      return () => { order.push('cleanup') }
    },
  })
  const t = await load(waits)
  const running = t.parallel('work')
  await t.unload()
  assert.deepEqual(order, ['aborted', 'finished', 'cleanup'])
  await running
  // A listener that never finishes is given up after the deadline.
  const stuck = await load(definePlugin({ apply(ctx) { ctx.on('work', () => new Promise(() => {})) } }), { deadline: 30 })
  stuck.emit('work')
  await stuck.unload()
  assert.deepEqual(stuck.reported.map(report => report.line), [`rutis: listeners of row ${stuck.row} still running after 0.03s; unloading anyway`])
})

test('next called twice or after the waterfall ended fails with WaterfallNext', async () => {
  let kept
  const t = await load(definePlugin({
    apply(ctx) {
      ctx.on('twice', async next => { await next(); return next() })
      ctx.on('late', next => { kept = next; return 'early' })
    },
  }))
  await assert.rejects(t.waterfall('twice'), { name: 'WaterfallNext', message: 'next of twice was already called' })
  assert.equal(await t.waterfall('late'), 'early')
  assert.throws(() => kept(), { name: 'WaterfallNext', message: 'next of late was called after its waterfall ended' })
  await t.unload()
})

// The testing tool has no host, but its bus is the runtime's (one file):
// the lines of §11 for an emit or a registration the host refuses.
test('the bus reports an emit and a registration the host refuses', async () => {
  const reported = []
  const refusing = {
    callAsync(target, method) { return Promise.reject(Object.assign(new Error(`${method} refused`), { name: 'BindingError' })) },
  }
  const bus = new Bus({ peer: refusing, report: line => reported.push(line) })
  bus.on({ name: 't/on', callback: () => {}, row: 'chat', plugin: 'p' })
  bus.elsewhere({ add: ['t/e'] })
  bus.dispatch('emit', 't/e', [], { row: 'chat' })
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(reported, [
    'rutis: registration of listener of t/on in row chat failed: BindingError: events.on refused',
    'rutis: emit of t/e from row chat failed: BindingError: events.dispatch refused',
  ])
})

test('a row uses another row\'s service as the object itself', async () => {
  const h = host()
  const store = { items: [] }
  await h.load(definePlugin({ apply(ctx) { ctx.provide('store', store) } }))
  let used
  await h.load(definePlugin({ inject: ['store'], apply(ctx) { used = ctx.use('store') } }))
  assert.equal(used, store)
  await assert.rejects(h.load(definePlugin({ inject: ['missing'], apply() {} })), /give the test a service missing/)
  await h.unload()
})
