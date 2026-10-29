// guarantee: sdk/leaf-events — the test tool's events (#464), on a real
// host: the five modes from the test, filters with descriptors, emit's
// reported failures, errors as from another process, unload waiting for
// running listeners, rutis/* and internal/* refused.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host, load } from '../src/testing.mjs'
import listener from './fixtures/listener.mjs'

test('the test emits to a plugin with the five methods', async () => {
  const t = await load(listener)
  assert.equal(await t.serial('ask', 'Oslo'), 'sunny in Oslo')
  assert.equal(t.bail('ask', 'Oslo'), 'sunny in Oslo')
  assert.equal(await t.waterfall.with(() => 'x')('wrap'), '<x>')
  assert.equal(t.emit('ask', 'Oslo'), undefined)
  await t.unload()
})

test('a filter sees each listener\'s descriptor', async () => {
  const h = host()
  const t = await h.load(listener)
  const seen = []
  assert.equal(await h.filter(descriptor => { seen.push(descriptor); return false }).serial('ask', 'Oslo'), undefined)
  assert.deepEqual(seen.map(({ plugin, row, runtime }) => ({ plugin, row, runtime })), [{ plugin: 'listener', row: t.row, runtime: 'test' }])
  await h.unload()
})

test('emit returns at once; the host reports a failing listener', async () => {
  const t = await load(listener)
  assert.equal(t.emit('tick'), undefined)
  // The report comes once the host ran the listener.
  let reported = []
  for (let i = 0; i < 100 && !reported.length; i++) {
    await new Promise(resolve => setTimeout(resolve, 10))
    reported = await t.reported()
  }
  assert.deepEqual(reported.map(report => report.line.split('\n')[0]), [`${t.row}: listener of tick failed: Boom: boom`])
  await t.unload()
})

test('errors reach the test as from another process: by name, a built-in class as itself', async () => {
  const t = await load(listener)
  const serial = await t.serial('tick').catch(error => error)
  assert.equal(serial.name, 'Boom')
  assert.equal(serial.message, 'boom')
  assert.ok(await t.serial('fail').catch(error => error) instanceof TypeError)
  const parallel = await t.parallel('tick').catch(error => error)
  assert.equal(parallel.name, 'AggregateError')
  assert.equal(parallel.errors[0].name, 'Boom')
  await t.unload()
})

test('unload aborts ctx.signal and waits for running listeners', async () => {
  const t = await load(listener)
  const running = t.parallel('work')
  await new Promise(resolve => setTimeout(resolve, 20))
  await t.unload()
  await running
})

test('rutis/* is the host\'s to emit; internal/* is the runtime\'s', async () => {
  const t = await load(listener)
  assert.throws(() => t.emit('rutis/row', {}), { name: 'ReservedEvent', message: 'rutis/* events are emitted by rutis only: rutis/row' })
  await assert.rejects(t.serial('rutis/row'), { name: 'ReservedEvent' })
  assert.throws(() => t.emit('internal/status'), /event internal\/status is internal to the runtime/)
  assert.throws(() => t.on('internal/dispatch', () => {}), /event internal\/dispatch is internal to the runtime/)
  await t.unload()
})
