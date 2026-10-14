import { expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { declared, locate, Modules, versionOf, PLUGIN_API } from '../src/plugin.ts'

function project() {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-'))
  writeFileSync(join(dir, 'package.json'), '{}')
  return dir
}

test('module exports and definePlugin both declare a plugin', () => {
  const apply = () => {}
  expect(declared({ apply, inject: ['a'], provides: { s: { m: 'sync' } } }, 'x').inject).toEqual(['a'])
  const made = { apply, inject: [], provides: {}, api: 1, [Symbol.for('rutis.leaf-plugin')]: true }
  expect(declared({ default: made }, 'x').apply).toBe(apply)
  expect(() => declared({ default: {} }, 'x')).toThrow('no apply')
  expect(() => declared({ apply, provides: { s: { m: 'later' } } }, 'x')).toThrow("kind must be 'sync' or 'async'")
  expect(() => declared({ apply, api: PLUGIN_API + 1 }, 'x')).toThrow('needs plugin API 3; this runtime supports 2')
})

test('entries resolve from the project to real paths, or are not found', () => {
  const dir = project()
  writeFileSync(join(dir, 'a.ts'), 'export function apply() {}')
  const pkg = join(dir, 'node_modules/@acme/p')
  mkdirSync(pkg, { recursive: true })
  writeFileSync(join(pkg, 'package.json'), JSON.stringify({ name: '@acme/p', version: '3.1.0', main: 'index.ts' }))
  writeFileSync(join(pkg, 'index.ts'), 'export function apply() {}')
  expect(locate('./a.ts', dir)).toBe(realpathSync(join(dir, 'a.ts')))
  const file = locate('@acme/p', dir)
  expect(file).toBe(realpathSync(join(pkg, 'index.ts')))
  expect(versionOf('@acme/p', file)).toBe('3.1.0')
  expect(versionOf('./a.ts', locate('./a.ts', dir))).toBeNull()
  for (const missing of ['./nope.ts', '@acme/nope']) {
    let error: any
    try { locate(missing, dir) } catch (thrown) { error = thrown }
    expect(error?.name).toBe('NotFound')
  }
})

test('a file created after the process started can be imported', async () => {
  const dir = project()
  const modules = new Modules()
  writeFileSync(join(dir, 'late.ts'), 'export const value = 1')
  expect((await modules.import(locate('./late.ts', dir))).value).toBe(1)
})

test('an edited module is imported again; an unchanged one is not', async () => {
  const dir = project()
  const modules = new Modules()
  const file = join(dir, 'edited.ts')
  writeFileSync(file, 'export const value = 1')
  const first = await modules.import(locate('./edited.ts', dir))
  expect(first.value).toBe(1)
  expect(await modules.import(locate('./edited.ts', dir))).toBe(first)
  writeFileSync(file, 'export const value = 22')
  expect((await modules.import(locate('./edited.ts', dir))).value).toBe(22)
})

test('a module that does not parse fails with its file and line', async () => {
  const dir = project()
  const file = join(dir, 'broken.ts')
  writeFileSync(file, 'export interface C {\n  a?: string\n}\n\nconst const = 1\n')
  const error: any = await new Modules().import(locate('./broken.ts', dir)).catch(error => error)
  expect(error.name).toBe('BuildMessage')
  expect(error.message).toContain(`${realpathSync(file)}:5:`)
  expect(error.message).toContain('const const = 1')
})
