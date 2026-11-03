// What a test says when the row is not running, and when a call never
// returns (#933): the line is the host's, the one `dev` prints; the codes
// are the tool's own where the failure is its, and the row error's where the
// error is the plugin's.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host, load } from '../src/testing.mjs'
import gated from './fixtures/gated.mjs'
import gatekeeper from './fixtures/gatekeeper.mjs'
import echo from './fixtures/echo.mjs'
import anonymous from './fixtures/anonymous.mjs'
import stuck from './fixtures/stuck.mjs'
import cycler from './fixtures/cycler.mjs'

async function until(check) {
  for (let i = 0; i < 200 && !check(); i++) await new Promise(resolve => setTimeout(resolve, 5))
}

test('a row that waits says what `dev` says, with the code `not-running`', async () => {
  const h = host()
  const t = await h.load(gated)
  const away = 'gated: waiting for later (nothing provides it yet)'
  assert.equal(await t.waiting(), away)
  // The same line the host gives, thrown by the synchronous `service()`.
  assert.throws(() => t.service('gated'), { name: 'PluginTestError', message: away, code: 'not-running' })
  // The provider arrives: the row starts, and neither says otherwise.
  const provider = await h.load(gatekeeper)
  await until(() => t.provided().includes('gated'))
  assert.equal(await t.waiting(), null)
  assert.equal(t.service('gated').ping(), 7)
  await provider.unload()
  await t.unload()
  await h.unload()
})

test('two rows that inject each other are told so, as `dev` tells it', async () => {
  // `gated` waits for `later`, which `cycler` provides, and `cycler` waits
  // for `gated`: neither starts, and the host says why.
  const h = host()
  const t = await h.load(gated)
  const other = await h.load(cycler)
  const away = 'gated: waiting for later (row cycler waits for gated too: they inject each other)'
  assert.equal(await t.waiting(), away)
  assert.throws(() => t.service('gated'), { name: 'PluginTestError', message: away, code: 'not-running' })
  await other.unload()
  await t.unload()
  await h.unload()
})

test('a service the running row has not got is `no-service`, with the nearest name', async () => {
  const t = await load(echo)
  assert.throws(() => t.service('ecoh'), {
    name: 'PluginTestError',
    message: 'echo: no such service ecoh (it provides: echo; did you mean echo?)',
    code: 'no-service',
    data: { row: 'echo', service: 'ecoh', services: ['echo'] },
  })
  assert.throws(() => t.methods('nothing'), {
    name: 'PluginTestError',
    message: 'echo: no such service nothing (it provides: echo)',
    code: 'no-service',
  })
  await t.unload()
})

test('a call that never returns fails at the tool\'s limit', async () => {
  const previous = process.env.RUTIS_TEST_CALLING_MS
  process.env.RUTIS_TEST_CALLING_MS = '50'
  // The limit meets a call while the test is still going, as another test in
  // the file would keep it going; a call that holds nothing by itself is
  // what lets `node --test` exit at the test's own limit (#997, §3.4), so
  // this one keeps the loop alive on its own.
  const going = setInterval(() => {}, 10)
  try {
    const t = await load(stuck)
    await assert.rejects(t.service('stuck').wait(), {
      name: 'PluginTestError',
      message: 'stuck.wait() did not return within 0.05s',
      code: 'timeout',
      data: { service: 'stuck', method: 'wait', limit: 0.05 },
    })
    await t.unload()
  } finally {
    clearInterval(going)
    if (previous === undefined) delete process.env.RUTIS_TEST_CALLING_MS
    else process.env.RUTIS_TEST_CALLING_MS = previous
  }
})

test('a plugin that declares no name takes its package\'s', async () => {
  const t = await load(anonymous)
  // The fixture lives in this package: `@arcships/rutis` without its scope.
  assert.equal(t.row, 'rutis')
  assert.throws(() => t.service('ano'), {
    name: 'PluginTestError',
    message: 'rutis: no such service ano (it provides: anon; did you mean anon?)',
    code: 'no-service',
  })
  await t.unload()
})
