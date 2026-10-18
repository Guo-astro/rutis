// guarantee: sdk/leaf-events (shared bus, cleanup, listener errors, internal/* refused, API 2)
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Context } from '@deepseek-ai/cordis'
import { definePlugin } from '../../rutis/src/index.mjs'
import { oncePerName, supported, toCordis } from '../src/leaf.mjs'
import { Bus } from '../src/events.mjs'
import { install } from '../src/cordis-events.mjs'

test('leaf events delegate to the shared Cordis bus and leave with their fiber', async () => {
  // The runtime's logical bus on the root, as the runner installs it.
  const root = new Context()
  const reported = []
  install(root, Context, new Bus({ runtime: 'node', report: line => reported.push(line) }), () => ({ row: null, scope: [], signal: undefined }))
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
  // emit never throws: every listener runs, the failure is reported (§5.6).
  leaf.emit('e')
  assert.deepEqual(seen, [1, 2])
  assert.match(reported.join('\n'), /rutis: listener of e in .* failed: Error: sync failure/)
  await root.parallel('root-event', 2)
  assert.deepEqual(received, [['root', 1], ['leaf', 2]])
  await plugin.dispose()
  await root.parallel('root-event', 3)
  assert.deepEqual(received, [['root', 1], ['leaf', 2]], 'unload removes the leaf listener')
})

// guarantee: sdk/leaf-events (failed load and update)
test('a failed apply and an update remove the leaf listeners, as the runner drives the fiber', async () => {
  const root = new Context()
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
  await failed.dispose()
  await plugin.dispose()
})

// An undeclared service under a host older than 0.12: told once per name,
// a declared or keyed one not at all.
test('a leaf plugin providing an undeclared service is told once per name', async () => {
  const root = new Context()
  const written = []
  const spec = definePlugin({
    provides: { declared: { get: 'sync' } },
    apply(ctx) {
      ctx.provide('declared', { get() {} })
      ctx.provide('weather', { today() {} })
      ctx.provide('weather#1', { today() {} })
    },
  })
  const told = oncePerName(text => written.push(text))
  const first = root.plugin(toCordis(supported(spec, 'old-host'), told))
  await first.await()
  await first.dispose()
  const again = root.plugin(toCordis(supported(spec, 'old-host'), told))
  await again.await()
  await again.dispose()
  assert.deepEqual(written, [
    'rutis: service weather is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host\n',
  ])
})
