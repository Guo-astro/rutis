// guarantee: bridge/plugin-values
// Plugin definitions as values (docs/design-plugin-values-2026-10-25.md §5):
// a definition the runtime handed over comes back to be described
// (`plugins.read`) and loaded as a row's entry, each copy with its config.
import { expect, test } from 'bun:test'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { FEATURES, Runtime } from '../src/runner.ts'
import { definePlugin } from '../../../node/rutis/src/index.mjs'

function runtime() {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-values-'))
  writeFileSync(join(dir, 'package.json'), '{}')
  return new Runtime(dir)
}

test('the runtime takes plugin definitions back', () => {
  expect(FEATURES).toContain('plugin-values')
})

test('plugins.read describes a definition at once', () => {
  const part = definePlugin({ name: 'part', inject: ['history'], config: { type: 'object' }, apply() {} })
  const described = runtime().dispatch('', 'plugins.read', [part])
  expect(described).toEqual({ name: 'part', config: { type: 'object' }, inject: ['history'], provides: {}, announces: true, version: null })
  expect(() => runtime().dispatch('', 'plugins.read', [42])).toThrow('not a plugin definition')
})

test('each copy of a definition loads with its own config and leaves with its row', async () => {
  const seen: string[] = []
  const part = definePlugin({
    apply(ctx: any, config: any) {
      seen.push(`up ${config.depth}`)
      ctx.effect(() => { seen.push(`down ${config.depth}`) })
    },
  })
  const rt = runtime()
  const control = (method: string, args: unknown[]) => rt.dispatch('', method, args) as Promise<unknown>
  await control('rows.load', ['c1', part, { depth: 1 }, [], [], null])
  await control('rows.load', ['c2', part, { depth: 2 }, [], [], null])
  expect(seen).toEqual(['up 1', 'up 2'])
  await control('rows.unload', ['c1'])
  expect(seen).toEqual(['up 1', 'up 2', 'down 1'])
  await control('rows.unload', ['c2'])
  expect(seen).toEqual(['up 1', 'up 2', 'down 1', 'down 2'])
})
