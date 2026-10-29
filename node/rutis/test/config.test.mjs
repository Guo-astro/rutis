// Config schemas (config schema design, docs/design-config-schema-2026-10-27.en.md):
// definePlugin takes JSON Schema only. The host checks a row's config and
// fills its defaults (the kernel's `rutis::config::check`, platform cleanup
// §7.5); test/plugin.test.mjs loads a plugin with a schema on a real host.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '../src/index.mjs'

test('definePlugin takes JSON Schema only', () => {
  const schemastery = { '~standard': { version: 1, vendor: 'schemastery', validate: () => ({ value: {} }) } }
  const zod = { '~standard': { version: 1, vendor: 'zod', validate: () => ({ value: {} }) } }
  assert.throws(() => definePlugin({ config: schemastery, apply() {} }), { name: 'TypeError', message: 'config of definePlugin should be a JSON Schema object, got a schemastery Schema' })
  assert.throws(() => definePlugin({ config: zod, apply() {} }), { message: 'config of definePlugin should be a JSON Schema object, got a Standard Schema' })
})
