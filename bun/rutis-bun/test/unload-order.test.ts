// Unloading runs one stack in reverse: cleanups and the withdrawals of the
// services a row provides, in the order they were registered (Rust's
// drain_effects; docs/design-unload-order-2026-11-01.md).
import { expect, test } from 'bun:test'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Runtime } from '../src/runner.ts'

function project(files: Record<string, string>) {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-order-'))
  writeFileSync(join(dir, 'package.json'), '{}')
  for (const [name, text] of Object.entries(files)) writeFileSync(join(dir, name), text)
  return dir
}

// What reading `name` through `ctx` gives: the count, or the error's code.
const READ = `
export const read = (ctx, name = 'tally') => {
  try { return 'count ' + ctx.use(name).count() } catch (error) { return 'error ' + (error.code ?? error.name) }
}
`

async function withRuntime(files: Record<string, string>, run: (runtime: Runtime) => Promise<void>) {
  const dir = project({ 'read.ts': READ, ...files })
  const runtime = new Runtime(dir)
  runtime.dispatch('', 'mount', { announce: true })
  try {
    await run(runtime)
  } finally {
    await runtime.dispose()
    rmSync(dir, { recursive: true, force: true })
  }
}

test('a cleanup registered after provide reads it; one before does not', () => withRuntime({
  'order.ts': `
import { read } from './read.ts'
export function apply(ctx) {
  ctx.effect(() => { globalThis.orderLog.push('before: ' + read(ctx)) })
  ctx.provide('tally', { count: () => 7 })
  ctx.effect(() => { globalThis.orderLog.push('after: ' + read(ctx)) })
}
`,
}, async runtime => {
  const log: string[] = (globalThis as any).orderLog = []
  await runtime.load('o', './order.ts', {}, null, null)
  await runtime.unload('o')
  expect(log).toEqual(['after: count 7', 'before: error not-declared'])
}))

test('another row loses it when the unload starts; the row itself still reads it', () => withRuntime({
  'provider.ts': `
import { read } from './read.ts'
export function apply(ctx) {
  ctx.provide('tally', { count: () => 2 })
  // Registered after the provide: it runs before the withdrawal.
  ctx.effect(() => { globalThis.loserLog.push('own: ' + read(ctx), 'other: ' + read(globalThis.loserUser)) })
}
`,
  'user.ts': `
export const inject = ['tally']
export function apply(ctx) { globalThis.loserUser = ctx }
`,
}, async runtime => {
  const log: string[] = (globalThis as any).loserLog = []
  await runtime.load('p', './provider.ts', {}, null, null)
  await runtime.load('u', './user.ts', {}, null, null)
  const user = (globalThis as any).loserUser
  expect('count ' + user.use('tally').count()).toBe('count 2')
  await runtime.unload('p')
  expect(log).toEqual(['own: count 2', 'other: error unavailable'])
  let after: any
  try { user.use('tally') } catch (error) { after = error }
  expect(after?.code).toBe('unavailable')
}))

test('a cleanup registering one is refused and says so', () => withRuntime({
  'nested.ts': `
export function apply(ctx) {
  ctx.effect(() => { ctx.effect(() => { globalThis.nestedRan = true }) })
}
`,
}, async runtime => {
  ;(globalThis as any).nestedRan = false
  const written: string[] = []
  const write = process.stderr.write.bind(process.stderr)
  ;(process.stderr as any).write = (chunk: any, ...rest: any[]) => { written.push(String(chunk)); return true }
  try {
    await runtime.load('nested', './nested.ts', {}, null, null)
    await runtime.unload('nested')
  } finally {
    ;(process.stderr as any).write = write
  }
  expect((globalThis as any).nestedRan).toBe(false)
  expect(written.join('')).toContain('rutis: row nested is not active: cleanup is not registered\n')
}))

test('an old withdrawal leaves a new registration of the name alone', () => withRuntime({
  'again.ts': `
import { read } from './read.ts'
export function apply(ctx) {
  const tally = { count: () => 5 }
  const withdraw = ctx.provide('tally', tally)
  withdraw()
  ctx.provide('tally', tally)
  withdraw()
  globalThis.againLog.push('during: ' + read(ctx))
  ctx.effect(() => { globalThis.againLog.push('cleanup: ' + read(ctx)) })
}
`,
}, async runtime => {
  const log: string[] = (globalThis as any).againLog = []
  await runtime.load('a', './again.ts', {}, null, null)
  await runtime.unload('a')
  expect(log).toEqual(['during: count 5', 'cleanup: count 5'])
}))
