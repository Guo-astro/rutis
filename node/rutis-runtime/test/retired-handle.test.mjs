// guarantee: sdk/optional-deps — a handle on a service whose row unloaded
// says the service is not available, to a call and to a property read,
// rather than reaching what is no longer there: a row holding it (an
// optional service, leased at start) is told it is gone. One its row
// replaced while running keeps its object until the row unloads, as a
// snapshot of a service does in Rust (boundary/5.1).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
import { fakeHost } from './fixtures/fake-host.mjs'
import { eventually, within } from './support.mjs'

const runner = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
const plugin = fileURLToPath(new URL('./fixtures/flip.mjs', import.meta.url))
const gone = { name: 'BindingError', message: 'service flip is not available' }

async function started() {
  const host = await fakeHost([process.execPath, runner, plugin])
  await within('the handshake', host.ready)
  await host.call('mount', { plugins: [], announce: true, scope: [] })
  await within('the row loads', host.call('rows.load', ['flip', plugin, {}, [], [], { flip: { toss: 'async', swap: 'async' } }, { scope: [] }]))
  return host
}

test('a handle on a service its row replaced keeps its object until the row unloads', async () => {
  const host = await started()
  try {
    const old = host.services.get('flip')
    assert.equal(await host.session.invokeAsync(old, 'toss', []), 'heads')
    await host.session.invokeAsync(old, 'swap', [])
    // The handle rutis is told of last answers with the new object.
    const toss = () => host.session.invokeAsync(host.services.get('flip'), 'toss', []).catch(() => null)
    await eventually('the new flip answers', async () => await toss() === 'tails', () => host.services.get('flip'))
    assert.notEqual(host.services.get('flip'), old)
    assert.equal(await host.session.invokeAsync(old, 'toss', []), 'heads')
    await within('the row unloads', host.call('rows.unload', ['flip']))
    await assert.rejects(host.session.invokeAsync(old, 'toss', []), gone)
    await assert.rejects(host.call('get', [old, 'toss']), gone)
  } finally {
    await host.end()
  }
})

test('a handle on a service whose row unloaded is not available', async () => {
  const host = await started()
  try {
    const old = host.services.get('flip')
    assert.equal(await host.session.invokeAsync(old, 'toss', []), 'heads')
    await within('the row unloads', host.call('rows.unload', ['flip']))
    await assert.rejects(host.session.invokeAsync(old, 'toss', []), gone)
    await assert.rejects(host.call('get', [old, 'toss']), gone)
  } finally {
    await host.end()
  }
})

test('a handle on a service whose row was loaded again is not available', async () => {
  const host = await started()
  try {
    const old = host.services.get('flip')
    assert.equal(await host.session.invokeAsync(old, 'toss', []), 'heads')
    await within('the row unloads', host.call('rows.unload', ['flip']))
    await within('the row loads again', host.call('rows.load', ['flip', plugin, {}, [], [], { flip: { toss: 'async', swap: 'async' } }, { scope: [] }]))
    // The new object has a handle of its own: the old one stays retired.
    const now = await eventually('the new flip', () => host.services.get('flip') !== old && host.services.get('flip'))
    assert.equal(await host.session.invokeAsync(now, 'toss', []), 'heads')
    await assert.rejects(host.session.invokeAsync(old, 'toss', []), gone)
  } finally {
    await host.end()
  }
})

// A service no declaration describes (announced) offers the methods of its
// object: a handle its row replaced keeps the old object's, and is told an
// old-object method it lacks is not there, rather than reaching nothing.
test('a handle its row replaced answers with its own object\'s methods', async () => {
  const reshape = fileURLToPath(new URL('./fixtures/reshape.mjs', import.meta.url))
  const host = await fakeHost([process.execPath, runner, reshape])
  try {
    await within('the handshake', host.ready)
    await host.call('mount', { plugins: [], announce: true, scope: [] })
    await within('the row loads', host.call('rows.load', ['coin', reshape, {}, [], [], null, { scope: [] }]))
    const old = await eventually('coin announced', () => host.services.get('coin'))
    assert.equal(await host.session.invokeAsync(old, 'toss', []), 'heads')
    await host.session.invokeAsync(old, 'swap', [])
    const now = await eventually('the new coin', () => host.services.get('coin') !== old && host.services.get('coin'))
    assert.equal(await host.session.invokeAsync(now, 'spin', []), 'spun')
    assert.equal(await host.session.invokeAsync(old, 'toss', []), 'heads')
    await assert.rejects(host.session.invokeAsync(old, 'spin', []), { message: /coin has no method spin/ })
  } finally {
    await host.end()
  }
})

// rows.update restarting the row (its config cannot change in place) is an
// unload and a load: the handles on its objects say they are gone.
test('a handle on a service whose row restarted on update is not available', async () => {
  const host = await started()
  try {
    const old = host.services.get('flip')
    assert.equal(await host.session.invokeAsync(old, 'toss', []), 'heads')
    await within('the row restarts', host.call('rows.update', ['flip', { round: 2 }]))
    await assert.rejects(host.session.invokeAsync(old, 'toss', []), gone)
    const now = await eventually('the new flip', () => host.services.get('flip') !== old && host.services.get('flip'))
    assert.equal(await host.session.invokeAsync(now, 'toss', []), 'heads')
  } finally {
    await host.end()
  }
})
