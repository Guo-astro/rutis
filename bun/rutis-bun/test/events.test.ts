// The bus of the Bun runtime (#464, docs/design-events-protocol-2026-10-18.en.md):
// the exact lines and errors of §5.7, §7.1 and §11 nobody else checks here.
// Every emission goes to the host (platform cleanup §7.5); the five modes
// across processes are run by the Rust session matrix on this runtime.
import { test } from 'bun:test'
import assert from 'node:assert/strict'
import { Bus } from '../src/events.mjs'
import { fakeHost } from './fixtures/fake-host.ts'

const refusal = (message: string) => Object.assign(new Error(message), { name: 'BindingError' })

// A bus whose host is the fake one, calling it back.
function hosted(report: (line: string) => void = () => {}) {
  const host = fakeHost()
  const bus = new Bus({ runtime: 'bun', peer: host, report })
  host.connect(fields => bus.call(fields))
  return { host, bus }
}

test('a refused or unsent emit through the host and a refused registration are reported', async () => {
  const reported: string[] = []
  const refusing: Record<string, unknown> = {
    callAsync(_target: string, method: string) { return Promise.reject(refusal(`${method} refused`)) },
  }
  const bus = new Bus({ runtime: 'bun', peer: refusing, report: (line: string) => { reported.push(line) } })
  bus.nameRow('k1', 'chat')
  bus.on({ name: 't/on', callback: () => {}, row: 'k1', plugin: 'p' })
  assert.equal(bus.dispatch('emit', 't/e', [], { row: 'k1' }), undefined)
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(reported.splice(0), [
    'rutis: registration of listener of t/on in row chat failed: BindingError: events.on refused',
    'rutis: emit of t/e from row chat failed: BindingError: events.dispatch refused',
  ])
  // The session ended under it: the frame cannot be sent.
  refusing.callAsync = () => { throw refusal('the session ended') }
  assert.equal(bus.dispatch('emit', 't/e', [], { row: 'k1' }), undefined)
  assert.deepEqual(reported.splice(0), ['rutis: emit of t/e from row chat failed: BindingError: the session ended'])
  // No host any more: an emit is reported, the other modes fail.
  bus.ended()
  assert.equal(bus.dispatch('emit', 't/e', [], { row: 'k1' }), undefined)
  assert.deepEqual(reported, ['rutis: emit of t/e from row chat failed: BindingError: the host disconnected'])
  for (const mode of ['parallel', 'serial', 'waterfall']) {
    await assert.rejects(bus.dispatch(mode, 't/e', []) as Promise<unknown>, { name: 'BindingError', message: 'the host disconnected', code: 'peer-gone' })
  }
  assert.throws(() => bus.dispatch('bail', 't/e', []), { name: 'BindingError', message: 'the host disconnected', code: 'peer-gone' })
})

test('next called twice fails with WaterfallNext', async () => {
  const { bus } = hosted()
  bus.on({ name: 'twice', callback: async (next: () => unknown) => { await next(); return next() }, row: 'r' })
  bus.on({ name: 'twice', callback: () => 'b', row: 'r' })
  await assert.rejects(bus.dispatch('waterfall', 'twice', []) as Promise<unknown>, { name: 'WaterfallNext', message: 'next of twice was already called' })
})

test('a failing emit listener fails back to the host; one its unload stopped does not', async () => {
  const reported: string[] = []
  const { host, bus } = hosted(line => { reported.push(line) })
  const failure = new Error('no')
  bus.on({ name: 'f', callback: () => { throw failure }, row: 'r' })
  bus.on({ name: 'f', callback: async () => { throw failure }, row: 'r' })
  bus.dispatch('emit', 'f', [])
  await host.settled()
  assert.deepEqual(host.failures, [failure, failure])
  assert.deepEqual(reported, [])
  // Stopped by its row's unload: its signal's AbortError is not a failure.
  let abort!: (error: Error) => void
  let started!: () => void
  const running = new Promise<void>(resolve => { started = resolve })
  bus.on({ name: 'a', callback: () => { started(); return new Promise((_, reject) => { abort = reject }) }, row: 's' })
  bus.dispatch('emit', 'a', [])
  await running
  bus.withdraw('s')
  abort(Object.assign(new Error('aborted'), { name: 'AbortError' }))
  await host.settled()
  assert.deepEqual(host.failures, [failure, failure])
})

test('unload gives up on a listener still running after the deadline, with its line', async () => {
  const reported: string[] = []
  const { bus } = hosted(line => { reported.push(line) })
  bus.nameRow('k2', 'stuck')
  let started!: () => void
  const running = new Promise<void>(resolve => { started = resolve })
  bus.on({ name: 'v', callback: () => { started(); return new Promise(() => {}) }, row: 'k2' })
  bus.dispatch('emit', 'v', [])
  await running
  bus.withdraw('k2')
  assert.equal(await bus.settle('k2', 20), false)
  assert.deepEqual(reported, ['rutis: listeners of row stuck still running after 0.02s; unloading anyway'])
})
