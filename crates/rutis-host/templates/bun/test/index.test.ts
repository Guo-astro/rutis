import { expect, test } from 'bun:test'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

// Step 6 of the Bun plugin guide: load the plugin, call its service, unload.
test('greets with the configured greeting', async () => {
  const t = await load(plugin, { config: { greeting: 'Hi' } })
  expect(await t.service('greeter').hello('Ada')).toBe('Hi, Ada!')
  await t.unload()
})
