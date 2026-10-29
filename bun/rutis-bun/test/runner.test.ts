import { expect, test } from 'bun:test'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { FEATURES, handleOf, methodsOf, Runtime, scopedId, undeclaredUse } from '../src/runner.ts'
import type { Context } from '../../../node/rutis/src/index.d.ts'
import { fakeHost } from './fixtures/fake-host.ts'

function project(files: Record<string, string>) {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-'))
  writeFileSync(join(dir, 'package.json'), '{}')
  for (const [name, text] of Object.entries(files)) writeFileSync(join(dir, name), text)
  return dir
}

const SCOPED = `
export const provides = { tools: { title: 'sync' } }
export function apply(ctx, config) { ctx.provide('tools', { title: () => config.title }) }
`
const USER = `
export const inject = ['tools']
export function apply(ctx) { globalThis.seen = (globalThis.seen ?? []).concat(ctx.use('tools').title()) }
`

test('ids and handles keep scopes apart', () => {
  expect(scopedId('tools')).toBe('tools')
  expect(scopedId('tools', 'a')).toBe('tools\0a')
  expect(() => scopedId('tools', '')).toThrow('empty scope label')
  expect(() => scopedId('to\0ols')).toThrow('NUL')
  expect(handleOf('tools', 1)).toBe('tools')
  expect(handleOf('tools', 2)).toBe('tools#2')
  expect(handleOf('tools\0a#b', 2)).toBe('tools\0a#b\x002')
})

test('mount reports the features and what runs the runtime', () => {
  const runtime = new Runtime(project({}))
  const mounted: any = runtime.dispatch('', 'mount', {})
  expect(mounted.features).toEqual(FEATURES)
  expect(mounted.engine).toEqual({ name: 'bun', version: Bun.version })
  expect(mounted.implementation.name).toBe('@arcships/rutis-bun')
})

test('rows isolating a name with different labels provide different services', async () => {
  const dir = project({ 'scoped.ts': SCOPED, 'user.ts': USER })
  const runtime = new Runtime(dir)
  const control = (method: string, args: unknown[]) => runtime.dispatch('', method, args) as Promise<unknown>
  await control('rows.load', ['a', './scoped.ts', { title: 'A' }, [['tools', 'a']], [], { tools: { title: 'sync' } }])
  await control('rows.load', ['b', './scoped.ts', { title: 'B' }, [['tools', 'b']], [], { tools: { title: 'sync' } }])
  await control('rows.load', ['ua', './user.ts', {}, [['tools', 'a']], ['tools'], null])
  await control('rows.load', ['ub', './user.ts', {}, [['tools', 'b']], ['tools'], null])
  expect((globalThis as any).seen).toEqual(['A', 'B'])
  // Exported under each scope's handle.
  expect((runtime.dispatch('tools\0a', 'title', []) as string)).toBe('A')
  expect((runtime.dispatch('tools\0b', 'title', []) as string)).toBe('B')
  // Unloading withdraws: the user of that scope can no longer find it.
  await control('rows.unload', ['a'])
  expect(() => runtime.lookup(scopedId('tools', 'a'))).toThrow('not available')
  expect(runtime.lookup(scopedId('tools', 'b'))).toBeDefined()
  // The retired handle lives until the host releases it.
  expect(runtime.dispatch('tools\0a', 'title', [])).toBe('A')
  runtime.dispatch('', 'release', ['tools\0a'])
  expect(() => runtime.dispatch('tools\0a', 'title', [])).toThrow('unknown or released')
})

test('a failed apply unloads the row and runs its cleanups', async () => {
  const dir = project({ 'failing.ts': `
export function apply(ctx) { ctx.effect(() => { globalThis.cleaned = true }); throw new Error('no') }
` })
  const runtime = new Runtime(dir)
  await expect(runtime.dispatch('', 'rows.load', ['f', './failing.ts', {}, [], [], null]) as Promise<unknown>).rejects.toThrow('no')
  expect((globalThis as any).cleaned).toBe(true)
  // The key is free again.
  writeFileSync(join(dir, 'ok.ts'), 'export function apply() {}')
  await runtime.dispatch('', 'rows.load', ['f', './ok.ts', {}, [], [], null])
})

// A service the plugin does not declare in `inject` is not given to it,
// running or not: `use` says so and how to fix it (#676); one it declares
// that is missing is still "not available".
// guarantee: sdk/undeclared-use
test('a plugin using a service it does not inject is told to declare it', async () => {
  const dir = project({
    'sneaky.ts': `
export function apply(ctx) {
  try { ctx.use('tsvc') } catch (error) { globalThis.undeclared = error.message; globalThis.undeclaredCode = [error.code, error.data] }
}
`,
    'declared.ts': `
export const inject = ['gone']
export function apply(ctx) {
  try { ctx.use('gone') } catch (error) { globalThis.missing = { message: error.message, code: error.code, data: error.data } }
}
`,
  })
  const runtime = new Runtime(dir)
  await runtime.dispatch('', 'rows.load', ['s', './sneaky.ts', {}, [], [], null])
  expect((globalThis as any).undeclared).toBe(undeclaredUse('tsvc'))
  expect((globalThis as any).undeclaredCode).toEqual(['not-declared', { service: 'tsvc' }])
  expect(undeclaredUse('tsvc')).toBe("service tsvc is not available to this plugin, which does not declare it in inject: add 'tsvc' to its inject")
  await runtime.dispatch('', 'rows.load', ['d', './declared.ts', {}, [], [], null])
  expect((globalThis as any).missing).toEqual({ message: 'service gone is not available', code: 'unavailable', data: { service: 'gone' } })
})

test('host services expose their declared methods only', () => {
  const runtime = new Runtime(project({}))
  runtime.dispatch('', 'hosts.provide', ['llm', { ask: 'sync' }])
  const llm: any = runtime.lookup('llm')
  expect(typeof llm.ask).toBe('function')
  let refused: any
  try { llm.other() } catch (error) { refused = error }
  expect([refused.name, refused.message, refused.code, refused.data]).toEqual(['BindingError', 'llm has no method other (methods: ask)', 'no-method', { service: 'llm', method: 'other', methods: ['ask'] }])
  expect(() => runtime.dispatch('', 'hosts.provide', ['llm', {}])).toThrow('already provided')
  runtime.dispatch('', 'hosts.withdraw', ['llm'])
  expect(() => runtime.lookup('llm')).toThrow('not available')
})

test('rows.schema reports what a module declares', async () => {
  const dir = project({ 'scoped.ts': SCOPED })
  const runtime = new Runtime(dir)
  expect(await runtime.dispatch('', 'rows.schema', ['./scoped.ts'])).toEqual({
    config: null, inject: [], provides: { tools: { title: 'sync' } }, announces: true, version: null,
  })
  // With or without `provides`, what it provides beyond it is announced when
  // it runs (#442).
  const undeclared = project({ 'plain.ts': 'export function apply(ctx) { ctx.provide("tools", { title() {} }) }' })
  expect(await new Runtime(undeclared).dispatch('', 'rows.schema', ['./plain.ts'])).toEqual({
    config: null, inject: [], provides: {}, announces: true, version: null,
  })
})

test('the methods of an undeclared service are its functions, own and inherited', () => {
  class Base { week() {} }
  class Forecast extends Base {
    city = 'Oslo'
    tomorrow() {}
  }
  expect([...methodsOf(new Forecast())].sort()).toEqual(['tomorrow', 'week'])
  expect([...methodsOf({ a: () => 1, b: 2 })]).toEqual(['a'])
  expect([...methodsOf(null)]).toEqual([])
})

// Rows `a` and `b` of one runtime whose host is the fake one: every
// emission goes to it, which calls the listeners back (platform cleanup
// §7.5).
async function eventRows(run: (runtime: Runtime, a: Context, b: Context, host: ReturnType<typeof fakeHost>) => Promise<void>) {
  const dir = project({ 'events.ts': 'export function apply(ctx, name) { ctx.provide(name, ctx) }' })
  const runtime = new Runtime(dir)
  const host = fakeHost().connect(fields => runtime.dispatch('', 'events.call', [fields]))
  runtime.attach(host as any)
  try {
    runtime.dispatch('', 'mount', { announce: true })
    for (const key of ['a', 'b']) await runtime.load(key, './events.ts', key, null, null)
    await run(runtime, runtime.lookup('a') as Context, runtime.lookup('b') as Context, host)
  } finally {
    await runtime.dispose()
    rmSync(dir, { recursive: true, force: true })
  }
}

// guarantee: sdk/leaf-events (one bus for the rows; disposers; unload; reserved names)
test('rows share the bus; listeners dispose independently and leave with their row', () => eventRows(async (runtime, a, b, host) => {
  const received: number[] = []
  const listener = (value: number) => { received.push(value) }
  a.on('changed', listener)
  const off = b.on('changed', listener)
  b.on('changed', listener)
  await a.parallel('changed', 7)
  expect(received).toEqual([7, 7, 7])
  off()
  off()
  expect(host.sent.filter(([method]) => method === 'events.off').length).toBe(1)
  expect(a.emit('changed', 8)).toBeUndefined()
  await host.settled()
  expect(received).toEqual([7, 7, 7, 8, 8])
  b.effect(async () => {
    await Promise.resolve()
    a.emit('changed', 9)
  })
  // The host withdraws an unloading row's listeners itself.
  host.forget('b')
  await runtime.unload('b')
  a.emit('changed', 10)
  await host.settled()
  expect(received).toEqual([7, 7, 7, 8, 8, 9, 10])
  expect(host.failures).toEqual([])
  // A plugin keeping its context past its row emits nothing (protocol §3),
  // also once a new row took its key.
  expect(() => b.emit('changed', 11)).toThrow('row b is not loaded in this runtime')
  await runtime.load('b', './events.ts', 'b2', null, null)
  expect(() => b.serial('changed')).toThrow('row b is not loaded in this runtime')
  await host.settled()
  expect(received).toEqual([7, 7, 7, 8, 8, 9, 10])
  expect(() => a.on('internal/dispatch', () => {})).toThrow('event internal/dispatch is internal to the runtime')
  expect(() => a.emit('internal/service')).toThrow('event internal/service is internal to the runtime')
  expect(() => (a as any).on(null, () => {})).toThrow(TypeError)
  expect(() => (a as any).emit(null, 'internal/service')).toThrow(TypeError)
  expect(a.on('rutis/row', () => {})).toBeInstanceOf(Function)
  expect(() => a.emit('rutis/row')).toThrow('rutis/* events are emitted by rutis only: rutis/row')
  expect(() => a.serial('rutis/row')).toThrow('rutis/* events are emitted by rutis only: rutis/row')
}))

// guarantee: sdk/leaf-events (the five modes; emit's failures go back to the host)
test('the five modes, prepend, global and filter', () => eventRows(async (_runtime, a, b, host) => {
  const write = process.stderr.write
  const written: string[] = []
  process.stderr.write = ((text: string) => { written.push(String(text)); return true }) as typeof write
  try {
    const seen: number[] = []
    const failure = new Error('sync failure')
    a.on('e', () => { seen.push(1); throw failure })
    b.on('e', () => { seen.push(2) })
    expect(a.emit('e')).toBeUndefined()
    await host.settled()
    expect(seen).toEqual([1, 2])
    // The host reports it, not this runtime.
    expect(host.failures).toEqual([failure])
    expect(written).toEqual([])
  } finally {
    process.stderr.write = write
  }
  const dispatched = host.sent.filter(([method]) => method === 'events.dispatch').map(([, [fields]]) => [fields.row, fields.name, fields.mode, fields.args])
  expect(dispatched).toEqual([['a', 'e', 'emit', []]])
  const errors = [new TypeError('sync failure'), new RangeError('async failure')]
  a.on('p', () => { throw errors[0] })
  b.on('p', async () => { await Bun.sleep(5); throw errors[1] })
  const error = await a.parallel('p').catch(error => error)
  expect(error).toBeInstanceOf(AggregateError)
  expect(error.errors).toEqual(errors)
  a.on('s', () => false)
  b.on('s', async () => 0)
  expect(await a.serial('s')).toBe(0)
  a.on('w', (x: string, next: () => unknown) => `a${next()}${x}`)
  b.on('w', () => 'b', { prepend: true })
  expect(await a.waterfall.with(() => 't')('w', '!')).toBe('b')
  b.on('b', async () => 'x')
  expect(() => a.bail('b')).toThrow('listener of b in row b is asynchronous and cannot run in a synchronous bail')
  b.on('g', () => 'global', { global: true })
  a.on('g', () => 'a')
  const descriptors: any[] = []
  expect(await (a as any).filter((d: any) => { descriptors.push(d); return false }).serial('g')).toBe('global')
  expect(descriptors.map(d => [d.row, d.global, d.prefix, d.node])).toEqual([['a', false, false, null]])
}))

// guarantee: sdk/leaf-events (unload signals and waits)
test('unload aborts the row signal and waits for its running listeners', () => eventRows(async (runtime, a, b, host) => {
  const order: string[] = []
  let started!: () => void
  const listening = new Promise<void>(resolve => { started = resolve })
  b.on('work', () => {
    started()
    return new Promise<void>(resolve => (b as any).signal.addEventListener('abort', () => { order.push('aborted'); setTimeout(resolve, 10) })).then(() => { order.push('finished') })
  })
  b.effect(() => { order.push('cleanup') })
  const running = a.parallel('work')
  await listening
  host.forget('b')
  await runtime.unload('b')
  expect(order).toEqual(['aborted', 'finished', 'cleanup'])
  await running
}))

// guarantee: sdk/leaf-events (events.call; withdrawn; the host's messages)
test('the host registers, dispatches and calls back through the session', async () => {
  const dir = project({ 'events.ts': 'export function apply(ctx) { ctx.on("t/x", x => `x${x}`); ctx.provide("me", ctx) }' })
  const runtime = new Runtime(dir)
  const sent: any[] = []
  runtime.attach({
    call: (_t: string, method: string, args: any) => { sent.push([method, args]); return 'B' },
    callAsync: (_t: string, method: string, args: any) => { sent.push([method, args]); return Promise.resolve(method === 'events.dispatch' ? 'H' : undefined) },
    callFunctionAsync: () => Promise.resolve(),
  } as any)
  try {
    runtime.dispatch('', 'mount', { announce: true })
    await runtime.dispatch('', 'rows.load', ['r', './events.ts', {}, [], [], {}, { scope: [] }])
    const [[method, [registration]]] = sent.filter(([method]) => method === 'events.on')
    expect([method, registration.name, registration.row, registration.kind, registration.prepend, registration.global]).toEqual(['events.on', 't/x', 'r', 'any', false, false])
    expect(await runtime.dispatch('', 'events.call', [{ listener: registration.id, dispatch: 'd1', own: false, name: 't/x', mode: 'serial', args: [1] }])).toBe('x1')
    // Its only listener is here, and still the host dispatches.
    const me = runtime.lookup('me') as Context
    expect(await me.serial('t/x', 3)).toBe('H')
    expect(me.bail('t/x')).toBe('B')
    const dispatched = sent.filter(([method]) => method === 'events.dispatch').map(([, [fields]]) => [fields.row, fields.name, fields.mode, fields.args])
    expect(dispatched).toEqual([['r', 't/x', 'serial', [3]], ['r', 't/x', 'bail', []]])
    await runtime.dispatch('', 'rows.unload', ['r'])
    expect(() => runtime.dispatch('', 'events.call', [{ listener: registration.id, dispatch: 'd2', name: 't/x', mode: 'serial', args: [] }]))
      .toThrow(`listener ${registration.id} of t/x is withdrawn`)
  } finally {
    await runtime.dispose()
    rmSync(dir, { recursive: true, force: true })
  }
})

// A listener the host refuses while its row loads fails the load and is
// not reported; one registered later is reported (protocol §11).
test('a refused registration fails the load; a later one is reported', async () => {
  const dir = project({ 'events.ts': 'export function apply(ctx, name) { ctx.on(name, () => {}); ctx.provide("me-" + name, ctx) }' })
  const runtime = new Runtime(dir)
  const refused = Object.assign(new Error('refused here'), { name: 'BindingError' })
  runtime.attach({
    call: () => undefined,
    callAsync: (_t: string, method: string, args: any) => method === 'events.on' && args[0].name.startsWith('t/no') ? Promise.reject(refused) : Promise.resolve(undefined),
    callFunctionAsync: () => Promise.resolve(),
  } as any)
  const write = process.stderr.write
  const written: string[] = []
  process.stderr.write = ((text: string) => { written.push(String(text)); return true }) as typeof write
  try {
    runtime.dispatch('', 'mount', { announce: true })
    await expect(runtime.dispatch('', 'rows.load', ['r', './events.ts', 't/no', [], [], {}, { scope: [] }])).rejects.toThrow('refused here')
    await runtime.dispatch('', 'rows.load', ['ok', './events.ts', 't/yes', [], [], {}, { scope: [] }])
    ;(runtime.lookup('me-t/yes') as Context).on('t/nope', () => {})
    await Bun.sleep(1)
    expect(written).toEqual(['rutis: registration of listener of t/nope in row ok failed: BindingError: refused here\n'])
  } finally {
    process.stderr.write = write
    await runtime.dispose()
    rmSync(dir, { recursive: true, force: true })
  }
})

// guarantee: sdk/leaf-events (failed load and update)
test('a failed apply and an update remove the row listeners', async () => {
  const dir = project({
    'events.ts': `export function apply(ctx, config) {
      ctx.on('changed', () => { globalThis.heard.push(config.label) })
      ctx.provide('emitter-' + config.label, ctx)
      if (config.fail) throw new Error('apply failed')
    }`,
  })
  const runtime = new Runtime(dir)
  const host = fakeHost().connect(fields => runtime.dispatch('', 'events.call', [fields]))
  runtime.attach(host as any)
  const heard: string[] = (globalThis as any).heard = []
  const listenerOf = (row: string) => host.sent.filter(([method, [fields]]) => method === 'events.on' && fields.row === row).map(([, [fields]]) => fields.id)
  try {
    runtime.dispatch('', 'mount', { announce: true })
    await runtime.load('a', './events.ts', { label: 'first' }, null, null)
    const [first] = listenerOf('a')
    // The host withdraws the listeners of a row it updates, and of one
    // whose load failed, itself; here they are withdrawn too.
    const withdrawn = (listener: string) => expect(() => runtime.dispatch('', 'events.call', [{ listener, dispatch: 'd0', own: true, name: 'changed', mode: 'parallel', args: [] }]))
      .toThrow(`listener ${listener} of changed is withdrawn`)
    host.forget('a')
    await runtime.update('a', { label: 'updated' })
    withdrawn(first)
    await expect(runtime.load('f', './events.ts', { label: 'failed', fail: true }, null, null)).rejects.toThrow('apply failed')
    host.forget('f')
    withdrawn(listenerOf('f')[0])
    await (runtime.lookup('emitter-updated') as Context).parallel('changed')
    expect(heard).toEqual(['updated'])
  } finally {
    await runtime.dispose()
    delete (globalThis as any).heard
    rmSync(dir, { recursive: true, force: true })
  }
})

// guarantee: sdk/error-texts
// What a row is told when it cannot load, or provides what another row
// does, in the developer's words: its files, services and rows (#677).
test('a row that cannot load or provides a taken service is told so plainly (#677)', async () => {
  const dir = project({
    'plain.ts': "export default { inject: ['llm'] }\n",
    'entry.ts': "import { x } from './util.ts'\nexport function apply() { return x }\n",
    'util.ts': "export const x = 1\nthrow new RangeError('bad util')\n",
    'forecast.ts': "export function apply(ctx) { ctx.provide('forecast', { today: () => 1 }) }\n",
    'caller.ts': "export const inject = ['llm']\nexport function apply(ctx) { ctx.use('llm').other() }\n",
  })
  const runtime = new Runtime(dir)
  runtime.dispatch('', 'mount', {})
  runtime.dispatch('', 'hosts.provide', ['llm', { ask: 'sync' }])
  const load = (key: string, entry: string) => Promise.resolve().then(() => runtime.dispatch('', 'rows.load', [key, entry, {}, [], [], null])).then(() => null, error => error)
  const pick = (error: any) => [error?.name, error?.message, error?.code, error?.data]
  const real = (name: string) => require('node:fs').realpathSync(join(dir, name))
  // Outside where the host runs: shown in full.
  const shown = real
  expect(pick(await load('plain', './plain.ts'))).toEqual(['LoadError', `the default export of ${shown('plain.ts')} is not a plugin made with definePlugin`, undefined, undefined])
  expect((await load('entry', './entry.ts'))?.message).toMatch(new RegExp(`^cannot load ${shown('util.ts').replace(/[.\\]/g, '\\$&')}: RangeError: bad util \\(.*util\\.ts:2:\\d+\\)$`))
  expect(await load('a', './forecast.ts')).toBeNull()
  expect(pick(await load('b', './forecast.ts'))).toEqual(['Error', 'forecast is already provided by a', undefined, undefined])
  expect(pick(await load('c', './caller.ts'))).toEqual(['BindingError', 'llm has no method other (methods: ask)', 'no-method', { service: 'llm', method: 'other', methods: ['ask'] }])
})

// guarantee: sdk/error-texts
// An apply failure crosses with the plugin's frames, none of the runtime's.
test('an apply failure crosses with the plugin\'s frames only (#677)', async () => {
  const { encodeError } = await import('../src/errors.ts')
  const dir = project({ 'weather.ts': "export function apply() {\n  throw new Error('no region')\n}\n" })
  const runtime = new Runtime(dir)
  runtime.dispatch('', 'mount', {})
  const failed: any = await Promise.resolve().then(() => runtime.dispatch('', 'rows.load', ['w', './weather.ts', {}, [], [], null])).catch(error => error)
  const stack = encodeError(failed).graph.nodes[0].stack
  expect(stack).toMatch(/^Error: no region\n\s+at .*weather\.ts:2/)
  expect(stack).not.toMatch(/rutis-bun[\\/]src|runner\.ts/)
})
