// The bus of the Bun runtime (#464, docs/design-events-protocol-2026-10-18.en.md):
// the exact lines and errors of §5.7, §7.1 and §11 nobody else checks here
// (the five modes are run by events-conformance.test.ts).
import { test } from 'bun:test'
import assert from 'node:assert/strict'
import { Bus } from '../src/events.mjs'

const refusal = (message: string) => Object.assign(new Error(message), { name: 'BindingError' })

test('a refused or unsent emit through the host and a refused registration are reported', async () => {
  const reported: string[] = []
  const refusing: Record<string, unknown> = {
    callAsync(_target: string, method: string) { return Promise.reject(refusal(`${method} refused`)) },
  }
  const bus = new Bus({ runtime: 'bun', peer: refusing, report: (line: string) => { reported.push(line) } })
  bus.nameRow('k1', 'chat')
  bus.on({ name: 't/on', callback: () => {}, row: 'k1', plugin: 'p' })
  bus.elsewhere({ add: ['t/e'] })
  assert.equal(bus.dispatch('emit', 't/e', [], { row: 'k1' }), undefined)
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(reported.splice(0), [
    'rutis: registration of listener of t/on in row chat failed: BindingError: events.on refused',
    'rutis: emit of t/e from row chat failed: BindingError: events.dispatch refused',
  ])
  // The session ended under it: the frame cannot be sent.
  refusing.callAsync = () => { throw refusal('the session ended') }
  assert.equal(bus.dispatch('emit', 't/e', [], { row: 'k1' }), undefined)
  assert.deepEqual(reported, ['rutis: emit of t/e from row chat failed: BindingError: the session ended'])
})

test('next called twice or after the waterfall ended fails with WaterfallNext', async () => {
  const bus = new Bus({ report: () => {} })
  bus.on({ name: 'twice', callback: async (next: () => unknown) => { await next(); return next() }, row: 'r' })
  bus.on({ name: 'twice', callback: () => 'b', row: 'r' })
  await assert.rejects(bus.dispatch('waterfall', 'twice', []), { name: 'WaterfallNext', message: 'next of twice was already called' })
  let kept: (() => unknown) | undefined
  bus.on({ name: 'late', callback: (next: () => unknown) => { kept = next; return 'x' }, row: 'r' })
  assert.equal(bus.dispatch('waterfall', 'late', []), 'x')
  assert.throws(() => kept!(), { name: 'WaterfallNext', message: 'next of late was called after its waterfall ended' })
})

test('unload gives up on a listener still running after the deadline, with its line', async () => {
  const reported: string[] = []
  const bus = new Bus({ report: (line: string) => { reported.push(line) } })
  bus.nameRow('k2', 'stuck')
  bus.on({ name: 'v', callback: () => new Promise(() => {}), row: 'k2' })
  bus.dispatch('emit', 'v', [])
  bus.withdraw('k2')
  assert.equal(await bus.settle('k2', 20), false)
  assert.deepEqual(reported, ['rutis: listeners of row stuck still running after 0.02s; unloading anyway'])
})
