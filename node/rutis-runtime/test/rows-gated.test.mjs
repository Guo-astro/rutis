// guarantee: loader/gated-row-failure — a row with a row-level
// `inject` (a gated row) reports its plugin's apply failure as the row's
// load failure, the same error an ungated row reports (#584).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
import { fakeHost } from './fixtures/fake-host.mjs'
import { within } from './support.mjs'

const runner = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
const failing = fileURLToPath(new URL('./fixtures/failing-apply.mjs', import.meta.url))

test('a gated row reports its plugin apply failure like an ungated one', async () => {
  const host = await fakeHost([process.execPath, runner, failing])
  try {
    await within('the handshake', host.ready)
    await host.call('mount', { plugins: [], announce: true, scope: [] })
    await host.call('hosts.provide', ['events-log', { add: 'sync' }])
    const load = (key, inject) => within(`row ${key} answers`, host.call('rows.load', [key, failing, { label: key }, [], inject, {}, { scope: [] }]))
    const ungated = await load('plain', []).then(() => null, error => error)
    const gated = await load('gated', ['events-log']).then(() => null, error => error)
    assert.equal(ungated?.name, 'ApplyFailed')
    assert.equal(ungated.message, 'apply failed: plain')
    assert.equal(gated?.name, 'ApplyFailed')
    assert.equal(gated.message, 'apply failed: gated')
    // The failed gated row is gone: its key loads again (and fails again).
    await assert.rejects(load('gated', ['events-log']), { name: 'ApplyFailed', message: 'apply failed: gated' })
  } finally {
    await host.end()
  }
})
