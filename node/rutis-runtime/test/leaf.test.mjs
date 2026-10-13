// guarantee: sdk/leaf-events (shared bus, cleanup, internal/* refused, API 2)
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Context } from '@deepseek-ai/cordis'
import { definePlugin } from '../../rutis/src/index.mjs'
import { supported, toCordis } from '../src/leaf.mjs'

test('leaf events delegate to the shared Cordis bus and leave with their fiber', async () => {
  const root = new Context()
  const received = []
  let leaf
  root.on('leaf-output', value => { received.push(['root', value]) })
  const spec = definePlugin({
    apply(ctx) {
      leaf = ctx
      ctx.on('root-event', value => { received.push(['leaf', value]) })
      ctx.emit('leaf-output', 1)
    },
  })
  const plugin = root.plugin(toCordis(supported(spec, 'events')))
  await plugin.await()
  assert.throws(() => leaf.on('internal/dispatch', () => {}), /is internal to the runtime/)
  assert.throws(() => leaf.emit('internal/service'), /is internal to the runtime/)
  await root.parallel('root-event', 2)
  assert.deepEqual(received, [['root', 1], ['leaf', 2]])
  await plugin.dispose()
  await root.parallel('root-event', 3)
  assert.deepEqual(received, [['root', 1], ['leaf', 2]], 'unload removes the leaf listener')
})

test('the runtime refuses a plugin needing a newer API', () => {
  const spec = definePlugin({ apply() {} })
  assert.throws(() => supported({ ...spec, api: spec.api + 1 }, 'future'), /needs plugin API 3; this runtime supports 2/)
})

// guarantee: sdk/leaf-events (listener errors)
test('a sync listener error stops later listeners and reaches the emitter', async () => {
  const root = new Context()
  const seen = []
  let leaf
  const plugin = root.plugin(toCordis({
    apply(ctx) {
      leaf = ctx
      ctx.on('e', () => { seen.push(1); throw new Error('sync failure') })
      ctx.on('e', () => { seen.push(2) })
    },
  }))
  await plugin.await()
  assert.throws(() => leaf.emit('e'), /sync failure/)
  assert.deepEqual(seen, [1])
  await plugin.dispose()
})
