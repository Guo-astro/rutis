import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

// Load the plugin without a host, call its service, unload it (step 6 of
// the TypeScript plugin guide).
test('greets with the configured greeting', async () => {
  const t = await load(plugin, { config: { greeting: 'Hi' } })
  assert.equal(await t.service('greeter').hello('Ada'), 'Hi, Ada!')
  await t.unload()
})
