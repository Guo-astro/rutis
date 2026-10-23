// guarantee: events/node-runtime — the Node runtime on the logical bus
// (#464), against a host written in JavaScript (fixtures/fake-host.mjs):
// listeners registered before rows.load answers, the fast path and
// dispatch through the host (every mode, filter selector, terminal and
// next by reference), events.call with internal/dispatch, withdrawal at
// unload with the row's signal, events-v2 required, the removed mount
// fields refused.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
import { fakeHost } from './fixtures/fake-host.mjs'
import { eventually, within } from './support.mjs'

const runner = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
const conformance = fileURLToPath(new URL('./fixtures/conformance-events.mjs', import.meta.url))
const cordis = fileURLToPath(new URL('./fixtures/events-cordis.mjs', import.meta.url))
const scopeFilter = fileURLToPath(new URL('./fixtures/events-scope-filter.mjs', import.meta.url))
const T1 = { id: '17', group: 'tenant' }
const T2 = { id: '18', group: 'tenant' }

async function started() {
  const host = await fakeHost([process.execPath, runner, conformance])
  await within('the handshake', host.ready)
  // The full set first, then mount (§4.5).
  await host.elsewhere({})
  await host.call('mount', { plugins: [], announce: true, scope: [] })
  await host.call('hosts.provide', ['events-log', { add: 'sync' }])
  const rows = {
    listener: (key, participant, scope = []) => host.call('rows.load', [key, conformance, participant, [], [], {}, { scope }]),
    emitter: async (key, scope = []) => {
      await host.call('rows.load', [key, conformance, { emitter: true, label: key }, [], [], { [`emitter-${key}`]: { dispatch: 'async', dispatch_sync: 'sync' } }, { scope }])
      return spec => host.session.invokeAsync(host.services.get(`emitter-${key}`), spec.mode === 'bail' ? 'dispatch_sync' : 'dispatch', [spec])
    },
  }
  return { host, rows }
}

test('listeners register before the row answers; dispatch runs here or through the host', async () => {
  const { host, rows } = await started()
  try {
    await rows.listener('a', { label: 'a', name: 't/x', kind: 'any', does: { returns: 'A' } }, [T1])
    assert.deepEqual(host.entries.map(({ name, row, plugin, kind, prepend, global }) => ({ name, row, plugin, kind, prepend, global })), [
      { name: 't/x', row: 'a', plugin: 'conformance-events', kind: 'any', prepend: false, global: false },
    ])
    const emit = await rows.emitter('e', [T1])
    // Every listener here: the fast path.
    assert.equal(await emit({ mode: 'serial', name: 't/x', args: [] }), 'A')
    assert.equal(host.dispatches.length, 0)
    // Listeners elsewhere too: through the host, which calls this one back.
    await host.elsewhere({ add: ['t/x'] })
    assert.equal(await emit({ mode: 'serial', name: 't/x', args: [] }), 'A')
    assert.deepEqual(host.dispatches.map(({ row, name, mode, args }) => ({ row, name, mode, args })), [{ row: 'e', name: 't/x', mode: 'serial', args: [] }])
    // A filter is a selector the host calls with the descriptors.
    assert.equal(await emit({ mode: 'serial', name: 't/x', args: [], filter: { keep: ['nobody'] } }), undefined)
    assert.ok(host.log.includes('filter:a'))
    // waterfall: next by reference, the terminal called by the host.
    await rows.listener('w', { label: 'w', name: 't/wf', does: { next: 'w' } })
    await host.elsewhere({ addPrefixes: ['t/wf'] })
    assert.equal(await emit({ mode: 'waterfall', name: 't/wf', args: [], terminal: 'end' }), 'wend')
    // bail: a synchronous dispatch through the host.
    await rows.listener('b', { label: 'b', name: 't/bail', kind: 'any', does: { returns: 'B' } })
    await host.elsewhere({ add: ['t/bail'] })
    assert.equal(await emit({ mode: 'bail', name: 't/bail', args: [] }), 'B')
    await rows.listener('c', { label: 'c', name: 't/async', kind: 'async', does: { returns: 'C' } })
    await host.elsewhere({ add: ['t/async'] })
    await assert.rejects(emit({ mode: 'bail', name: 't/async', args: [] }), { name: 'AsyncListener' })
    // parallel: the host aggregates in registration order.
    await rows.listener('p', { label: 'p', name: 't/par', does: { throws: { name: 'PErr', message: 'p' } } })
    await host.elsewhere({ add: ['t/par'] })
    await assert.rejects(emit({ mode: 'parallel', name: 't/par', args: [] }), { name: 'AggregateError' })
    // emit: answered once its calls started; the API returned at once.
    assert.equal(await emit({ mode: 'emit', name: 't/x', args: [] }), null)
    await eventually('the emit through the host', () => host.dispatches.some(dispatch => dispatch.mode === 'emit'))
    // rutis/* is refused before anything is sent.
    await assert.rejects(emit({ mode: 'serial', name: 'rutis/fake', args: [] }), { name: 'ReservedEvent' })
    // Unloaded: withdrawn at once; a call for it fails as withdrawn.
    const [entry] = host.entries.filter(entry => entry.row === 'a')
    await host.call('rows.unload', ['a'])
    await assert.rejects(host.call('events.call', [{ listener: entry.id, dispatch: 'dx', own: false, name: 't/x', mode: 'serial', args: [] }]), {
      name: 'BindingError', message: `listener ${entry.id} of t/x is withdrawn`,
    })
  } finally {
    await host.end()
  }
})

test('a Cordis plugin: internal/dispatch once per dispatch, Context.filter, ctx.rutis', async () => {
  const { host, rows } = await started()
  try {
    await host.call('rows.load', ['cordis', cordis, {}, [], [], { 'cordis-probe': { run: 'async', rutis: 'sync', seen: 'sync' } }, { scope: [] }])
    await rows.listener('x', { label: 'x', name: 't/ask', does: { returns: null } })
    const probe = (method, args = []) => host.session.invokeAsync(host.services.get('cordis-probe'), method, args)
    assert.deepEqual(await probe('rutis'), { name: null, prefix: false, plugin: 'events-cordis', row: 'cordis', runtime: null, node: null, scope: [], global: false })
    // Here: Cordis's own dispatch fires internal/dispatch once.
    assert.equal(await probe('run', ['serial', 't/ask', ['Oslo']]), 'cordis:Oslo')
    assert.deepEqual(await probe('seen'), ['dispatch:serial:t/ask', 'ask:Oslo'])
    // Context.filter: the listener's Context here, `{ rutis }` elsewhere.
    await probe('run', ['parallel', 't/ask', ['Bergen'], 'x'])
    assert.deepEqual(await probe('seen'), ['dispatch:parallel:t/ask', 'filter:cordis', 'filter:x'])
    assert.ok(host.log.includes('x'))
    // From elsewhere (own: false): fired before its first listener here.
    const [entry] = host.entries.filter(entry => entry.row === 'cordis')
    for (let i = 0; i < 2; i++) await host.call('events.call', [{ listener: entry.id, dispatch: 'remote-1', own: false, name: 't/ask', mode: 'serial', args: ['Rome'] }])
    assert.deepEqual(await probe('seen'), ['dispatch:serial:t/ask', 'ask:Rome', 'ask:Rome'])
  } finally {
    await host.end()
  }
})

test('unload signals running listeners and waits for them', async () => {
  const { host, rows } = await started()
  try {
    await rows.listener('b', { label: 'b', name: 't/unload', does: { waits: true } })
    const emit = await rows.emitter('e')
    const running = emit({ mode: 'parallel', name: 't/unload', args: [] })
    await eventually('b started', () => host.log.includes('b'))
    await within('the unload', host.call('rows.unload', ['b']))
    assert.deepEqual(host.log, ['b', 'b:cancelled'])
    assert.equal(await running, null)
  } finally {
    await host.end()
  }
})

test('the removed mount fields are refused, naming the replacement', async () => {
  const host = await fakeHost([process.execPath, runner, conformance])
  try {
    await within('the handshake', host.ready)
    for (const field of ['events', 'emits']) {
      await assert.rejects(host.call('mount', { [field]: ['x'] }), {
        message: `the config field ${field} was removed in rutis 0.13: listeners register themselves; emit with ctx.emit / parallel / serial / waterfall / bail (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)`,
      })
    }
  } finally {
    await host.end()
  }
})

test('a host that does not declare events-v2 is refused', async () => {
  const host = await fakeHost([process.execPath, runner, conformance], {
    hello: { op: 'hello', version: 3, endpoint: 'host', implementation: { name: 'rutis-host', version: '0.12.1' }, capabilities: ['signals', 'sync-wait'] },
  })
  const code = await within('the runtime to stop', host.exited)
  assert.equal(code, 1)
  assert.match(host.stderr, /^rutis: refused: incompatible \(this host does not declare events-v2 \(the event protocol of rutis 0\.13\): upgrade rutis-host to 0\.13 or later\); stopping$/m)
  await host.end()
})

// Two copies of one row (a group's two instances) in one process share the
// row's id and plugin: the filter is asked about the copy the host means,
// by the id the host gives, and sees that copy's descriptor (its scope).
test('through the host, the filter judges the very copy the host asks about', async () => {
  const { host } = await started()
  try {
    const load = (key, config, scope, exports = {}) => host.call('rows.load', [key, scopeFilter, config, [], [], exports, { scope, id: key.split('#')[0] }])
    await load('c#t2', { listen: 't/copies' }, [T2])
    await load('c#t1', { listen: 't/copies' }, [T1])
    await load('e', { probe: 'e' }, [T1], { 'probe-e': { filtered: 'async', emitKept: 'async' } })
    const probe = (method, args) => host.session.invokeAsync(host.services.get('probe-e'), method, args)
    // The filter is asked about both copies; it keeps the t1 copy.
    assert.deepEqual(await probe('filtered', ['t/copies', '17']), ['18', '17'])
    // Through the host: the same.
    await host.elsewhere({ add: ['t/copies'] })
    assert.deepEqual(await probe('filtered', ['t/copies', '17']), ['18', '17'])
    assert.equal(host.dispatches.length, 1)
  } finally {
    await host.end()
  }
})

test('a plugin keeping its context past its row emits nothing', async () => {
  const { host } = await started()
  try {
    await host.call('rows.load', ['old', scopeFilter, { keep: true }, [], [], {}, { scope: [] }])
    await host.call('rows.load', ['p', scopeFilter, { probe: 'p' }, [], [], { 'probe-p': { filtered: 'async', emitKept: 'async' } }, { scope: [] }])
    await host.call('rows.unload', ['old'])
    const probe = method => host.session.invokeAsync(host.services.get('probe-p'), method, ['t/late'])
    assert.equal(await probe('emitKept'), 'BindingError: row old is not loaded in this runtime')
  } finally {
    await host.end()
  }
})

test('a listener the host refuses fails the row\'s load, and is not reported', async () => {
  const { host, rows } = await started()
  try {
    host.refused.add('t/refused')
    await assert.rejects(rows.listener('r', { label: 'r', name: 't/refused', does: { returns: null } }), { message: 'listening to t/refused is refused here' })
    await assert.rejects(host.call('events.call', [{ listener: '1', dispatch: 'd', own: false, name: 't/refused', mode: 'serial', args: [] }]), { name: 'BindingError' })
  } finally {
    await host.end()
  }
  assert.doesNotMatch(host.stderr, /registration of listener/)
})

// rows.update restarts the row as an unload and a load: its running
// listeners are signalled and waited for, its listeners leave and register
// again, and the update is answered once they are registered.
test('rows.update unloads (signalled, waited for) and loads the row again', async () => {
  const { host, rows } = await started()
  try {
    await rows.listener('b', { label: 'b', name: 't/update', does: { waits: true } })
    const [before] = host.entries.filter(entry => entry.row === 'b')
    const emit = await rows.emitter('e')
    await host.elsewhere({ add: ['t/update'] })
    const running = emit({ mode: 'parallel', name: 't/update', args: [] })
    await eventually('b started', () => host.log.includes('b'))
    await within('the update', host.call('rows.update', ['b', { label: 'b', name: 't/update', does: { returns: 'again' } }]))
    assert.deepEqual(host.log, ['b', 'b:cancelled'])
    assert.equal(await running, null)
    const after = host.entries.filter(entry => entry.row === 'b')
    assert.equal(after.length, 1)
    assert.notEqual(after[0].id, before.id)
    assert.equal(await emit({ mode: 'serial', name: 't/update', args: [] }), 'again')
  } finally {
    await host.end()
  }
})
