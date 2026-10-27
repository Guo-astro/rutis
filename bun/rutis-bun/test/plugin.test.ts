import { expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
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
  // guarantee: sdk/error-texts
  let refused: any
  try { declared({ default: {} }, 'x', '/work/src/index.ts') } catch (error) { refused = error }
  expect([refused.name, refused.message]).toEqual(['LoadError', 'the default export of /work/src/index.ts is not a plugin made with definePlugin'])
  expect(() => declared({}, 'x')).toThrow('the default export of x is not a plugin made with definePlugin')
  expect(() => declared({ apply, provides: { s: { m: 'later' } } }, 'x')).toThrow("kind must be 'sync' or 'async'")
  expect(() => declared({ apply, api: PLUGIN_API + 1 }, 'x')).toThrow(`needs plugin API ${PLUGIN_API + 1}; this runtime supports ${PLUGIN_API}`)
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

// Under `rutis-host dev`, a row that loads again for another reason (a
// service it uses came back) runs the module its description recorded, not
// an edit made since that does not import, nor one that declares no plugin;
// a description records the edit once it does (#665). Outside `dev`, a row
// loads the file as it is.
test('under dev a row loads the module last described, not a broken edit', async () => {
  const dir = project()
  const file = locate('./kept.ts', (writeFileSync(join(dir, 'kept.ts'), "export const name = 'v1'\nexport function apply() {}\n"), dir))
  const modules = new Modules(true)
  expect((await modules.plugin(file, './kept.ts')).name).toBe('v1')
  writeFileSync(file, 'export const const = 2\n')
  const error: any = await modules.plugin(file, './kept.ts').catch(error => error)
  expect(error).toBeInstanceOf(Error)
  expect((await modules.described(file, './kept.ts')).name).toBe('v1')
  writeFileSync(file, "export const name = 'v2'\n")
  expect(await modules.plugin(file, './kept.ts').catch(error => error)).toBeInstanceOf(Error)
  expect((await modules.described(file, './kept.ts')).name).toBe('v1')
  writeFileSync(file, "export const name = 'v333'\nexport function apply() {}\n")
  expect((await modules.described(file, './kept.ts')).name).toBe('v1')
  expect((await modules.plugin(file, './kept.ts')).name).toBe('v333')
  expect((await modules.described(file, './kept.ts')).name).toBe('v333')

  // Outside dev, the file as it is now.
  const run = new Modules(false)
  expect((await run.plugin(file, './kept.ts')).name).toBe('v333')
  writeFileSync(file, "export const name = 'v4444'\nexport function apply() {}\n")
  expect((await run.described(file, './kept.ts')).name).toBe('v4444')
})

test('a module that does not parse fails with its file and line', async () => {
  const dir = project()
  const file = join(dir, 'broken.ts')
  writeFileSync(file, 'export interface C {\n  a?: string\n}\n\nconst const = 1\n')
  const error: any = await new Modules().import(locate('./broken.ts', dir)).catch(error => error)
  expect(error.name).toBe('LoadError')
  expect(error.message).toMatch(new RegExp(`^cannot load ${escape(realpathSync(file))}: SyntaxError: .+ \\(${escape(realpathSync(file))}:5:\\d+\\)$`))
  expect(error.cause.name).toBe('BuildMessage')
})

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

// guarantee: sdk/error-texts
// A submodule that throws as it is imported is named, not the entry
// (§8.1 row 5); an entry file that was there and is no more is said so.
test('an import error names the submodule that threw, and a gone entry is said so (#677)', async () => {
  const dir = realpathSync(project())
  writeFileSync(join(dir, 'entry.ts'), "import { x } from './util.ts'\nexport function apply() { return x }\n")
  writeFileSync(join(dir, 'util.ts'), "export const x = 1\nthrow new RangeError('bad util')\n")
  const error: any = await new Modules().import(join(dir, 'entry.ts')).catch(error => error)
  expect(error.name).toBe('LoadError')
  expect(error.message).toMatch(new RegExp(`^cannot load ${escape(join(dir, 'util.ts'))}: RangeError: bad util \\(${escape(join(dir, 'util.ts'))}:2:\\d+\\)$`))
  const modules = new Modules()
  writeFileSync(join(dir, 'kept.ts'), 'export function apply() {}\n')
  await modules.import(join(dir, 'kept.ts'))
  rmSync(join(dir, 'kept.ts'))
  const gone: any = await modules.import(join(dir, 'kept.ts')).catch(error => error)
  expect([gone.name, gone.message]).toEqual(['LoadError', `the entry file ${join(dir, 'kept.ts')} is gone`])
})
