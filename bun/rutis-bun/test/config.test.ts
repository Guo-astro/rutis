// Config schemas in the Bun runtime (config schema design §2.2): the shared
// case list against this runtime's copy, and a row's config filled and
// validated before `apply`.
import { expect, test } from 'bun:test'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { checkConfig, configLines } from '../src/config.mjs'
import { Runtime } from '../src/runner.ts'

const { cases } = JSON.parse(readFileSync(new URL('../../../specs/config-schema/cases.json', import.meta.url), 'utf8'))

test('this copy passes every case of the shared list', () => {
  for (const c of cases) {
    const checked = checkConfig(c.schema, Object.hasOwn(c, 'config') ? c.config : undefined)
    expect(checked.warnings.map((w: { message: string }) => `config: ${w.message}`)).toEqual(c.warnings)
    if (c.then.errors) expect(configLines(checked.errors)).toEqual(c.then.errors)
    else expect(checked.config).toEqual(c.then.config)
  }
})

const WEATHER = `
export const config = { type: 'object', required: ['city'], properties: { city: { type: 'string' }, units: { enum: ['metric', 'imperial'], default: 'metric' } } }
export function apply(ctx, config) { globalThis.weather = (globalThis.weather ?? []).concat([config]) }
`

test("a row's config is filled, and a wrong one fails the row with bad-config", async () => {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-'))
  writeFileSync(join(dir, 'package.json'), '{}')
  writeFileSync(join(dir, 'weather.ts'), WEATHER)
  const runtime = new Runtime(dir)
  const control = (method: string, args: unknown[]) => runtime.dispatch('', method, args) as Promise<unknown>
  await control('rows.load', ['good', './weather.ts', { city: 'Oslo' }, [], [], null])
  expect((globalThis as any).weather).toEqual([{ city: 'Oslo', units: 'metric' }])
  let failed: any
  try {
    await control('rows.load', ['bad', './weather.ts', { units: 'kelvin' }, [], [], null])
  } catch (error) {
    failed = error
  }
  expect(failed?.name).toBe('BindingError')
  expect(failed?.code).toBe('bad-config')
  expect(failed?.message).toBe('config: city is required\nconfig: units should be "metric" or "imperial", got "kelvin"')
  expect((globalThis as any).weather.length).toBe(1)
})
