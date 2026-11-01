// A plugin definition a test double receives goes straight to `load`
// (plugin values R10, #1004): the test's service receives it, hands it
// back, and `load` mounts it as a row of the runtime that made it, as `load`
// of a file does. What differs from the copy a plugin mounts itself
// (`ctx.plugin(def.at(..))`) is in
// docs/design-test-tool-definition-load-2026-11-01.md §4.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host, load } from '../src/testing.mjs'
import part from './fixtures/part.mjs'
import quiet from './fixtures/quiet.mjs'
import watched from './fixtures/watched.mjs'
import configured from './fixtures/configured.mjs'
import registrar from './fixtures/registrar.mjs'
import sink from './fixtures/sink.mjs'

// What the host delivers arrives a little later (../testing-on.test.mjs).
async function until(check) {
  for (let i = 0; i < 200 && !check(); i++) await new Promise(resolve => setTimeout(resolve, 5))
}

// The test's `parts` service: what it received, which it hands back unchanged.
const parts = () => {
  const handed = []
  return { handed, service: { async register(definition) { handed.push(definition); return definition } } }
}

test('a definition a double received is mounted by load, and the plugin gets it back', async () => {
  const { handed, service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  assert.equal(handed.length, 1)
  const t = await load(handed[0])
  // A row of the host, like `load` of a file: its id is the definition's
  // name, and what it provides is announced.
  assert.equal(t.row, 'part')
  assert.deepEqual(t.provided(), ['echo'])
  assert.equal(t.service('echo').say(), 'part')
  await t.unload()
  await owner.unload()
})

test('the same definition loaded twice is two rows, each unloaded on its own', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const first = await load(quiet)
  const second = await load(quiet)
  assert.deepEqual([first.row, second.row], ['quiet', 'quiet-2'])
  await first.unload()
  // The other one is untouched.
  assert.deepEqual(second.provided(), [])
  await second.unload()
  await owner.unload()
})

test('an explicit id that a row already has fails, naming the row', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(quiet, { id: 'mine' })
  await assert.rejects(load(quiet, { id: 'mine' }), { name: 'PluginTestError', message: 'a row mine is loaded already' })
  await t.unload()
  await owner.unload()
})

test('a definition gets the config defaults its schema declares', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(configured)
  assert.equal(t.service('configured').depth(), 1)
  await t.unload()
  await owner.unload()
})

test('a config the definition\'s schema refuses fails the load, and the definition loads after', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  await assert.rejects(load(configured, { config: { depth: 'deep' } }), { name: 'PluginTestError', message: /^configured: config: depth should be a number, got "deep"$/ })
  // The failed load left nothing behind: the registration is released, and
  // the same definition loads again under the same name.
  const t = await load(configured)
  assert.equal(t.row, 'configured')
  await t.unload()
  await owner.unload()
})

test('its listeners go with the row: gone after unloading, one when loaded again', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(watched)
  t.emit('watched/ping')
  await until(() => t.service('watched').hits() === 1)
  assert.equal(t.service('watched').hits(), 1)
  await t.unload()
  // Nothing of the unloaded row hears it any more, and nothing of it is
  // left beside the next one.
  const t2 = await load(watched)
  t2.emit('watched/ping')
  await until(() => t2.service('watched').hits() === 1)
  assert.equal(t2.service('watched').hits(), 1)
  await t2.unload()
  await owner.unload()
})

test('the test hears what the row emits, as it hears a row of load', async () => {
  const { service } = parts()
  const h = host({ services: { parts: service } })
  const owner = await h.load(registrar)
  const heard = []
  owner.on('watched/ping', () => { heard.push('ping') })
  const t = await h.load(watched)
  t.emit('watched/ping')
  await until(() => heard.length)
  assert.deepEqual(heard, ['ping'])
  await t.unload()
  await owner.unload()
  await h.unload()
})

test('the origin row unloading first leaves the row running (design §4)', async () => {
  const { handed, service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(handed[0])
  await owner.unload()
  // A row of its own, not a copy of the definition: it runs until the test
  // unloads it.
  assert.equal(t.service('echo').say(), 'part')
  await t.unload()
})

test('a value no double received is refused', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
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
