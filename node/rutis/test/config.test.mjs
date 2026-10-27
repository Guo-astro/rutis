// Config schemas (config schema design, docs/design-config-schema-2026-10-27.en.md):
// the shared case list, run word for word; the testing tool fills defaults
// and validates as a runtime does; definePlugin takes JSON Schema only.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { definePlugin } from '../src/index.mjs'
import { checkConfig, configLines } from '../src/config.mjs'
import { host, load } from '../src/testing.mjs'

const read = path => readFileSync(new URL(path, import.meta.url), 'utf8')
const { cases } = JSON.parse(read('../../../specs/config-schema/cases.json'))

// Every object and array of `value` changed in place.
function scribble(value) {
  if (Array.isArray(value)) { value.forEach(scribble); value.push(true) }
  else if (value !== null && typeof value === 'object') { Object.values(value).forEach(scribble); value.scribbled = true }
}

test('every case of the shared list passes', () => {
  const failures = []
  for (const c of cases) {
    const run = () => checkConfig(c.schema, Object.hasOwn(c, 'config') ? c.config : undefined)
    const checked = run()
    const warnings = checked.warnings.map(w => `config: ${w.message}`)
    try { assert.deepEqual(warnings, c.warnings) } catch { failures.push(`${c.name}: warnings ${JSON.stringify(warnings)}`) }
    if (c.then.errors) {
      const lines = configLines(checked.errors)
      try { assert.deepEqual(lines, c.then.errors) } catch { failures.push(`${c.name}: errors ${JSON.stringify(lines)}`) }
      continue
    }
    if (checked.errors.length) { failures.push(`${c.name}: unexpected errors ${JSON.stringify(checked.errors)}`); continue }
    try { assert.deepEqual(checked.config, c.then.config) } catch { failures.push(`${c.name}: config ${JSON.stringify(checked.config)}`) }
    // What is filled is a copy: changing it changes nothing of the next check.
    scribble(checked.config)
    try { assert.deepEqual(run().config, c.then.config) } catch { failures.push(`${c.name}: a second check differs`) }
  }
  assert.deepEqual(failures, [])
})

test('the config rules are one file in the testing tool, the Node runtime and the Bun runtime', () => {
  const mine = read('../src/config.mjs')
  assert.equal(read('../../rutis-runtime/src/config.mjs'), mine, 'node/rutis-runtime/src/config.mjs')
  assert.equal(read('../../../bun/rutis-bun/src/config.mjs'), mine, 'bun/rutis-bun/src/config.mjs')
})

const weather = seen => definePlugin({
  config: {
    type: 'object',
    required: ['city'],
    properties: { city: { type: 'string' }, units: { enum: ['metric', 'imperial'], default: 'metric' } },
  },
  apply(ctx, config) { seen.push(config) },
})

// What `run` writes on standard error, and its result.
async function stderrOf(run) {
  const written = []
  const write = process.stderr.write
  process.stderr.write = chunk => { written.push(String(chunk)); return true }
  try { return { result: await run(), written } } finally { process.stderr.write = write }
}

test('the testing tool fills defaults, validates and warns as the runtime does', async () => {
  const seen = []
  const t = await load(weather(seen), { config: { city: 'Oslo' } })
  assert.deepEqual(seen, [{ city: 'Oslo', units: 'metric' }])
  await t.unload()

  await assert.rejects(load(weather(seen), { config: { units: 'kelvin' } }), error => {
    assert.equal(error.name, 'BindingError')
    assert.equal(error.code, 'bad-config')
    assert.equal(error.message, 'config: city is required\nconfig: units should be "metric" or "imperial", got "kelvin"')
    assert.deepEqual(error.data.issues, [
      { path: ['city'], message: 'city is required' },
      { path: ['units'], message: 'units should be "metric" or "imperial", got "kelvin"' },
    ])
    return true
  })
  await assert.rejects(load(weather(seen), { config: 'Oslo' }), /^BindingError: config: config should be an object, got "Oslo"$/)
  assert.equal(seen.length, 1, 'apply is not called with a wrong config')

  const h = host()
  const { written } = await stderrOf(() => h.load(weather(seen), { config: { city: 'Oslo', cty: 'x' }, row: 'weather' }))
  await h.unload()
  assert.deepEqual(written, ['weather: config: cty is not a config key (did you mean city?)\n'])
  assert.deepEqual(seen.at(-1), { city: 'Oslo', cty: 'x', units: 'metric' })
})

test('an update with a wrong config fails, and the row is unloaded', async () => {
  const seen = []
  const t = await load(weather(seen), { config: { city: 'Oslo' } })
  await assert.rejects(t.update({ city: 3 }), { code: 'bad-config', message: 'config: city should be a string, got 3' })
  assert.equal(seen.length, 1)
  await t.unload()
})

test('a Standard Schema as a module config is refused at load', async () => {
  const plugin = definePlugin({ apply() {} })
  const smuggled = { ...plugin, config: { '~standard': { version: 1, vendor: 'zod', validate: () => ({}) } } }
  Object.defineProperty(smuggled, Symbol.for('rutis.leaf-plugin'), { value: true })
  await assert.rejects(load(smuggled), { message: 'config of definePlugin should be a JSON Schema object, got a Standard Schema' })
})

test('a plugin without a schema gets what it is given', async () => {
  const seen = []
  const plugin = definePlugin({ apply(ctx, config) { seen.push(config) } })
  await (await load(plugin)).unload()
  await (await load(plugin, { config: 'Oslo' })).unload()
  assert.deepEqual(seen, [{}, 'Oslo'])
})

test('definePlugin takes JSON Schema only', () => {
  const schemastery = { '~standard': { version: 1, vendor: 'schemastery', validate: () => ({ value: {} }) } }
  const zod = { '~standard': { version: 1, vendor: 'zod', validate: () => ({ value: {} }) } }
  assert.throws(() => definePlugin({ config: schemastery, apply() {} }), { name: 'TypeError', message: 'config of definePlugin should be a JSON Schema object, got a schemastery Schema' })
  assert.throws(() => definePlugin({ config: zod, apply() {} }), { message: 'config of definePlugin should be a JSON Schema object, got a Standard Schema' })
})

test("a test's own values that JSON lacks are written by what they are", async () => {
  const plugin = definePlugin({ config: { type: 'object', properties: { n: { type: 'integer' }, at: { type: 'object' } } }, apply() {} })
  await assert.rejects(load(plugin, { config: { n: 5n, at: new Date(0) } }), { message: 'config: at should be an object, got a Date\nconfig: n should be an integer, got 5n' })
})
