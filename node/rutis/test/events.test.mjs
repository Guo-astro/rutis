// guarantee: sdk/leaf-events — the testing tool's events (#464): several
// rows on one bus, scopes, filter with descriptors, prepend and global,
// emit's reported failures, unload signalling and waiting.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '../src/index.mjs'
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
  assert.equal(emitter.waterfall.with(x => `t${x}`)('wrap', 1), '<t1>')
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

// Across processes a listener's error arrives by name, not by class: the
// tool gives a plugin's own emitters the crossed form, so a plugin
// matching by class fails in its tests as it would on a real host.
test('errors reach a plugin\'s own emitters as from another process', async () => {
  class Custom extends Error { name = 'Custom' }
  const h = host()
  let emitter
  await h.load(definePlugin({ apply(ctx) { emitter = ctx } }), { row: 'e' })
  await h.load(listener('fail', () => { throw new Custom('custom') }), { row: 'a' })
  await h.load(listener('type', () => { throw new TypeError('typed') }), { row: 't' })
  const serial = await emitter.serial('fail').catch(error => error)
  assert.ok(!(serial instanceof Custom))
  assert.equal(serial.name, 'Custom')
  assert.equal(serial.message, 'custom')
  assert.throws(() => emitter.bail('fail'), error => !(error instanceof Custom) && error.name === 'Custom')
  const parallel = await emitter.parallel('fail').catch(error => error)
  assert.equal(parallel.name, 'AggregateError')
  assert.ok(!(parallel.errors[0] instanceof Custom))
  assert.equal(parallel.errors[0].name, 'Custom')
  // A built-in class crosses as itself.
  assert.ok(await emitter.serial('type').catch(error => error) instanceof TypeError)
  await h.unload()
})

test('descriptors call the runtime as the language; the deadline is RUTIS_SHUTDOWN_TIMEOUT', async () => {
  const seen = []
  const h = host()
  let emitter
  await h.load(definePlugin({ apply(ctx) { emitter = ctx } }), { row: 'e' })
  await h.load(definePlugin({ apply(ctx) { ctx.on('d', () => {}) } }), { row: 'anonymous' })
  await emitter.filter(descriptor => { seen.push(descriptor); return true }).parallel('d')
  assert.deepEqual(seen.map(({ runtime, plugin, row }) => ({ runtime, plugin, row })), [{ runtime: 'node', plugin: 'anonymous', row: 'anonymous' }])
  await h.unload()
  const named = host({ runtime: 'js' })
  await named.load(definePlugin({ apply(ctx) { ctx.on('d', () => {}) } }))
  const descriptors = []
  await named.filter(descriptor => { descriptors.push(descriptor.runtime); return true }).parallel('d')
  assert.deepEqual(descriptors, ['js'])
  await named.unload()
  const before = process.env.RUTIS_SHUTDOWN_TIMEOUT
  process.env.RUTIS_SHUTDOWN_TIMEOUT = '0.02'
  try {
    const stuck = await load(definePlugin({ apply(ctx) { ctx.on('work', () => new Promise(() => {})) } }))
    stuck.emit('work')
    await stuck.unload()
    assert.deepEqual(stuck.reported.map(report => report.line), [`rutis: listeners of row ${stuck.row} still running after 0.02s; unloading anyway`])
  } finally {
    if (before === undefined) delete process.env.RUTIS_SHUTDOWN_TIMEOUT
    else process.env.RUTIS_SHUTDOWN_TIMEOUT = before
  }
})
