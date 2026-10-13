import { expect, test } from 'bun:test'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { FEATURES, handleOf, Runtime, scopedId } from '../src/runner.ts'
import type { Context } from '../../../node/rutis/src/index.d.ts'

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

test('host services expose their declared methods only', () => {
  const runtime = new Runtime(project({}))
  runtime.dispatch('', 'hosts.provide', ['llm', { ask: 'sync' }])
  const llm: any = runtime.lookup('llm')
  expect(typeof llm.ask).toBe('function')
  expect(() => llm.other()).toThrow('llm.other is not provided by the rutis host')
  expect(() => runtime.dispatch('', 'hosts.provide', ['llm', {}])).toThrow('already provided')
  runtime.dispatch('', 'hosts.withdraw', ['llm'])
  expect(() => runtime.lookup('llm')).toThrow('not available')
})

test('rows.schema reports what a module declares', async () => {
  const dir = project({ 'scoped.ts': SCOPED })
  const runtime = new Runtime(dir)
  expect(await runtime.dispatch('', 'rows.schema', ['./scoped.ts'])).toEqual({
    config: null, inject: [], provides: { tools: { title: 'sync' } }, version: null,
  })
})

async function eventRows(run: (runtime: Runtime, a: Context, b: Context) => Promise<void>) {
  const dir = project({ 'events.ts': 'export function apply(ctx, name) { ctx.provide(name, ctx) }' })
  const runtime = new Runtime(dir)
  try {
    runtime.dispatch('', 'mount', { emits: ['changed'] })
    for (const key of ['a', 'b']) await runtime.load(key, './events.ts', key, null, null)
    await run(runtime, runtime.lookup('a') as Context, runtime.lookup('b') as Context)
  } finally {
    await runtime.dispose()
    rmSync(dir, { recursive: true, force: true })
  }
}

test('host events require declared names; shared listeners dispose independently and unload', () => eventRows(async (runtime, a, b) => {
  const received: number[] = []
  const listener = (value: number) => { received.push(value) }
  a.on('changed', listener)
  const off = b.on('changed', listener)
  b.on('changed', listener)
  await runtime.dispatch('', 'emit', ['changed', [7]])
  expect(received).toEqual([7, 7, 7])
  off()
  expect(a.emit('changed', 8)).toBeUndefined()
  expect(received).toEqual([7, 7, 7, 8, 8])
  b.effect(async () => {
    await Promise.resolve()
    a.emit('changed', 9)
  })
  await runtime.unload('b')
  a.emit('changed', 10)
  expect(received).toEqual([7, 7, 7, 8, 8, 9, 10])
  const seen: number[] = []
  a.on('e', () => { seen.push(1); throw new Error('sync failure') })
  a.on('e', () => { seen.push(2) })
  expect(() => a.emit('e')).toThrow('sync failure')
  expect(seen).toEqual([1])
  expect(() => a.on('internal/dispatch', () => {})).toThrow('event internal/dispatch is internal to the runtime')
  expect(() => a.emit('internal/service')).toThrow('event internal/service is internal to the runtime')
  expect(() => runtime.dispatch('', 'mount', { emits: ['internal/dispatch'] })).toThrow('is internal to the runtime and cannot be emitted from rutis')
}))

test('host emission awaits every listener and aggregates sync and async errors', () => eventRows(async (runtime, a, b) => {
  const errors = [new TypeError('sync failure'), new RangeError('async failure')]
  let release!: () => void
  const pending = new Promise<void>(resolve => { release = resolve })
  let waiting = false
  a.on('changed', () => { throw errors[0] })
  b.on('changed', async () => { throw errors[1] })
  a.on('changed', async () => { waiting = true; await pending })
  let settled = false
  const outcome = (runtime.dispatch('', 'emit', ['changed', []]) as Promise<void>).then(
    () => { settled = true; return null },
    error => { settled = true; return error },
  )
  expect(waiting).toBe(true)
  await Bun.sleep(0)
  expect(settled).toBe(false)
  release()
  const error = await outcome
  expect(error).toBeInstanceOf(AggregateError)
  expect(error.errors).toEqual(errors)
}))

// guarantee: sdk/leaf-events (failed load and update)
test('a failed apply and an update remove the row listeners', async () => {
  const dir = project({
    'events.ts': `export function apply(ctx, config) {
      ctx.on('changed', () => { globalThis.heard.push(config.label) })
      if (config.fail) throw new Error('apply failed')
    }`,
  })
  const runtime = new Runtime(dir)
  const heard: string[] = (globalThis as any).heard = []
  try {
    runtime.dispatch('', 'mount', { emits: ['changed'] })
    await runtime.load('a', './events.ts', { label: 'first' }, null, null)
    await runtime.update('a', { label: 'updated' })
    await expect(runtime.load('f', './events.ts', { label: 'failed', fail: true }, null, null)).rejects.toThrow('apply failed')
    await runtime.dispatch('', 'emit', ['changed', []])
    expect(heard).toEqual(['updated'])
  } finally {
    await runtime.dispose()
    delete (globalThis as any).heard
    rmSync(dir, { recursive: true, force: true })
  }
})
