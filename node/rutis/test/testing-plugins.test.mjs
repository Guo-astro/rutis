// A plugin definition a test double receives goes straight to `load`
// (plugin values R10, #1004): the plugin under test makes the definition and
// hands it to the test's service, the test hands that value to `load`, and
// `load` mounts it as a row of the runtime that made it, as `load` of a file
// does. Every case here loads the handle a double received, never a module
// the test process imported: that is the path this is about. What differs
// from the copy a plugin mounts itself (`ctx.plugin(def.at(..))`) is in
// docs/design-test-tool-definition-load-2026-11-01.md §4.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host, load } from '../src/testing.mjs'
import handover from './fixtures/handover.mjs'
import part from './fixtures/part.mjs'
import gatekeeper from './fixtures/gatekeeper.mjs'
import sink from './fixtures/sink.mjs'

// The test's `parts` service: what the plugin handed over, by the
// definition's name; it hands each back unchanged.
const parts = () => {
  const handed = new Map()
  return { handed, service: { async register(name, definition) { handed.set(name, definition); return definition } } }
}

// What the host delivers arrives a little later (../testing-on.test.mjs).
async function until(check) {
  for (let i = 0; i < 200 && !check(); i++) await new Promise(resolve => setTimeout(resolve, 5))
}

test('a definition a double received is mounted by load, and the plugin gets it back', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  // `handover` checks each came back as itself; all five arrived here.
  assert.deepEqual([...handed.keys()].sort(), ['configured', 'gated', 'part', 'quiet', 'watched'])
  const t = await load(handed.get('part'))
  // A row of the host, like `load` of a file: its id is the definition's
  // name, and what it provides is announced.
  assert.equal(t.row, 'part')
  assert.deepEqual(t.provided(), ['echo'])
  assert.equal(t.service('echo').say(), 'part')
  await t.unload()
  await owner.unload()
})

test('the same definition loaded twice is two rows, each unloaded on its own', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const first = await load(handed.get('quiet'))
  const second = await load(handed.get('quiet'))
  assert.deepEqual([first.row, second.row], ['quiet', 'quiet-2'])
  await first.unload()
  // The other one is untouched.
  assert.deepEqual(second.provided(), [])
  await second.unload()
  await owner.unload()
})

test('an explicit id that a row already has fails, naming the row', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(handed.get('quiet'), { id: 'mine' })
  await assert.rejects(load(handed.get('quiet'), { id: 'mine' }), { name: 'PluginTestError', message: 'a row mine is loaded already' })
  await t.unload()
  await owner.unload()
})

test('a definition gets the config defaults its schema declares', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(handed.get('configured'))
  assert.equal(t.service('configured').depth(), 1)
  await t.unload()
  await owner.unload()
})

test('a config the definition\'s schema refuses fails the load, and the definition loads after', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  await assert.rejects(load(handed.get('configured'), { config: { depth: 'deep' } }), { name: 'PluginTestError', message: /^configured: config: depth should be a number, got "deep"$/ })
  // The failed load left nothing behind: its registration is released, and
  // the same definition loads again under the same row id.
  const t = await load(handed.get('configured'))
  assert.equal(t.row, 'configured')
  await t.unload()
  await owner.unload()
})

test('its listeners go with the row: gone after unloading, one when loaded again', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(handed.get('watched'))
  t.emit('watched/ping')
  await until(() => t.service('watched').hits() === 1)
  assert.equal(t.service('watched').hits(), 1)
  await t.unload()
  // Nothing of the unloaded row hears it any more, and nothing of it is
  // left beside the next one.
  const t2 = await load(handed.get('watched'))
  t2.emit('watched/ping')
  await until(() => t2.service('watched').hits() === 1)
  assert.equal(t2.service('watched').hits(), 1)
  await t2.unload()
  await owner.unload()
})

test('the test hears what the row emits, as it hears a row of load', async () => {
  const { handed, service } = parts()
  const h = host({ services: { parts: service } })
  const owner = await h.load(handover)
  const heard = []
  owner.on('watched/ping', () => { heard.push('ping') })
  const t = await h.load(handed.get('watched'))
  t.emit('watched/ping')
  await until(() => heard.length)
  assert.deepEqual(heard, ['ping'])
  await t.unload()
  await owner.unload()
  await h.unload()
})

test('the origin row unloading first leaves the row running (design §4)', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(handed.get('part'))
  await owner.unload()
  // A row of its own, not a copy of the definition: it runs until the test
  // unloads it.
  assert.equal(t.service('echo').say(), 'part')
  await t.unload()
})

test('a definition whose injected service is not there waits, and starts when it arrives', async () => {
  const { handed, service } = parts()
  const h = host({ services: { parts: service } })
  const owner = await h.load(handover)
  const t = await h.load(handed.get('gated'))
  // Waiting, not failed: the load answered, the host reported nothing, and
  // the service is not provided yet. The row's state is the host's line, the
  // one `dev` prints (`gated: waiting for later (no row provides it)`), and
  // `waiting()` hands it over.
  assert.deepEqual(t.provided(), [])
  assert.deepEqual(await t.reported(), [])
  const away = 'gated: waiting for later (no row provides it)'
  assert.equal(await t.waiting(), away)
  assert.throws(() => t.service('gated'), { name: 'PluginTestError', message: away, code: 'not-running' })
  // The service arrives, as a row of its own: the waiting row starts.
  const provider = await h.load(gatekeeper)
  await until(() => t.provided().includes('gated'))
  assert.deepEqual(t.provided(), ['gated'])
  assert.equal(await t.waiting(), null)
  assert.equal(t.service('gated').ping(), 7)
  await provider.unload()
  await t.unload()
  await owner.unload()
  await h.unload()
})

test('a value no double received is refused', async () => {
  const { service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  // A definition made in the test's own process, handed to a plugin's
  // service: a far end that declared no `plugins` takes no definition from
  // another process (plugin values §5.3).
  const t = await load(sink)
  await assert.rejects(t.service('sink').register(part), {
    message: /^sink\.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions \(it did not declare the capability plugins\)$/,
  })
  // A forged handle through `load`: the marker is public, a real handle is
  // the session's.
  await assert.rejects(load({ [Symbol.for('rutis.plugin-definition')]: true }), {
    name: 'PluginTestError',
    message: 'load needs a plugin definition a test double received',
  })
  await t.unload()
  await owner.unload()
})
