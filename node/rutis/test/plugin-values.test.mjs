// A plugin definition handed to a test double (design-plugin-values R10):
// in strict mode it crosses as itself, as it goes back to the runtime that
// defined it, so the test loads it with `load` as the host would mount it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '../src/index.mjs'
import { load } from '../src/testing.mjs'

test('a definition handed to a test double loads, each copy with its config', async () => {
  const handed = []
  const parts = { register(scope, definition) { handed.push([scope, definition]) } }
  const prepared = 'prepared in apply'
  let part
  const origin = definePlugin({
    inject: ['parts'],
    async apply(ctx) {
      part = definePlugin({
        provides: { tag: { text: 'sync' } },
        apply(ctx, config) { ctx.provide('tag', { text: () => `${config.n} ${prepared}` }) },
      })
      await ctx.use('parts').register('session', part)
    },
  })
  await load(origin, { services: { parts } })
  assert.equal(handed.length, 1)
  const [scope, definition] = handed[0]
  assert.equal(scope, 'session')
  assert.equal(definition, part)
  const one = await load(definition, { config: { n: 1 } })
  const two = await load(definition, { config: { n: 2 } })
  assert.equal(await one.service('tag').text(), `1 ${prepared}`)
  assert.equal(await two.service('tag').text(), `2 ${prepared}`)
})
