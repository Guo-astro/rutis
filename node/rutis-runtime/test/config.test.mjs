// Config schemas in the Node runtime (config schema design §2.2, §2.3): a
// leaf plugin's config is filled and validated before `apply`; a Cordis
// plugin's `Config` issues become the same `bad-config` error.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { Context } from '@deepseek-ai/cordis'
import { definePlugin } from '../../rutis/src/index.mjs'
import { supported, toCordis } from '../src/leaf.mjs'
import { checkConfig, configLines } from '../src/config.mjs'
import { cordisConfig } from '../src/cordis-config.mjs'

const { cases } = JSON.parse(readFileSync(new URL('../../../specs/config-schema/cases.json', import.meta.url), 'utf8'))

test('this copy passes every case of the shared list', () => {
  for (const c of cases) {
    const checked = checkConfig(c.schema, Object.hasOwn(c, 'config') ? c.config : undefined)
    assert.deepEqual(checked.warnings.map(w => `config: ${w.message}`), c.warnings, c.name)
    if (c.then.errors) assert.deepEqual(configLines(checked.errors), c.then.errors, c.name)
    else assert.deepEqual(checked.config, c.then.config, c.name)
  }
})

const schema = {
  type: 'object',
  properties: { city: { type: 'string', default: 'Tromso' }, units: { enum: ['metric', 'imperial'] } },
}

test('a leaf plugin gets its config filled, and fails its row on a wrong one', async () => {
  const seen = []
  const spec = definePlugin({ config: schema, apply(ctx, config) { seen.push(config) } })
  const root = new Context()
  const good = root.plugin(toCordis(supported(spec, 'weather')), { units: 'metric' })
  await good.await()
  assert.deepEqual(seen, [{ units: 'metric', city: 'Tromso' }])
  const bad = root.plugin(toCordis(supported(spec, 'weather')), { units: 'kelvin' })
  await assert.rejects(bad.await(), error => {
    assert.equal(error.name, 'BindingError')
    assert.equal(error.code, 'bad-config')
    assert.equal(error.message, 'config: units should be "metric" or "imperial", got "kelvin"')
    assert.deepEqual(error.data, { issues: [{ path: ['units'], message: 'units should be "metric" or "imperial", got "kelvin"' }] })
    return true
  })
  assert.equal(seen.length, 1, 'apply is not called with a wrong config')
})

test("a Cordis plugin's Config issues are a bad-config error, written as every runtime writes them", () => {
  const plugin = {
    apply() {},
    Config: {
      '~standard': {
        version: 1,
        vendor: 'schemastery',
        validate: config => (typeof config?.port === 'number'
          ? { value: config }
          : { issues: [{ message: 'expected number but got string', path: [{ key: 'server' }, 'port'] }, { message: 'expected object' }] }),
      },
    },
  }
  assert.doesNotThrow(() => cordisConfig(plugin, { port: 1 }))
  assert.throws(() => cordisConfig(plugin, { port: 'x' }), error => {
    assert.equal(error.code, 'bad-config')
    assert.equal(error.message, 'config: server.port expected number but got string\nconfig: config expected object')
    return true
  })
  // Not a Standard Schema: Cordis's own rules, nothing here.
  assert.doesNotThrow(() => cordisConfig({ apply() {}, Config: { type: 'object' } }, 'x'))
})
