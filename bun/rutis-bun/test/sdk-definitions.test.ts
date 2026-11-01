// A plugin definition a test double receives goes straight to `load`
// (#1004) under `bun test` too: it is mounted as a row of the runtime that
// made it, as a plugin module is (node/rutis/test/testing-plugins.test.mjs,
// docs/design-test-tool-definition-load-2026-11-01.md §4).
import { expect, test } from 'bun:test'
import { host, load } from '../../../node/rutis/src/testing.mjs'
import part from '../../../node/rutis/test/fixtures/part.mjs'
import quiet from '../../../node/rutis/test/fixtures/quiet.mjs'
import watched from '../../../node/rutis/test/fixtures/watched.mjs'
import configured from '../../../node/rutis/test/fixtures/configured.mjs'
import registrar from '../../../node/rutis/test/fixtures/registrar.mjs'
import sink from '../../../node/rutis/test/fixtures/sink.mjs'

const parts = () => {
  const handed: any[] = []
  return { handed, service: { async register(definition: unknown) { handed.push(definition); return definition } } }
}

const message = async (promise: Promise<unknown>) => promise.then(() => 'loaded', (error: Error) => error.message)

test('a definition a double received is mounted by load, and the plugin gets it back', async () => {
  const { handed, service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  expect(handed.length).toBe(1)
  const t = await load(handed[0])
  expect(t.row).toBe('part')
  expect(t.provided()).toEqual(['echo'])
  expect(t.service('echo').say()).toBe('part')
  await t.unload()
  await owner.unload()
})

test('the same definition loaded twice is two rows, each unloaded on its own', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const first = await load(quiet)
  const second = await load(quiet)
  expect([first.row, second.row]).toEqual(['quiet', 'quiet-2'])
  await first.unload()
  expect(second.provided()).toEqual([])
  await second.unload()
  await owner.unload()
})

test('an explicit id that a row already has fails, naming the row', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(quiet, { id: 'mine' })
  expect(await message(load(quiet, { id: 'mine' }))).toBe('a row mine is loaded already')
  await t.unload()
  await owner.unload()
})

test('a definition gets the config defaults its schema declares', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(configured)
  expect(t.service('configured').depth()).toBe(1)
  await t.unload()
  await owner.unload()
})

test('a config the definition’s schema refuses fails the load, and the definition loads after', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  expect(await message(load(configured, { config: { depth: 'deep' } }))).toBe('configured: config: depth should be a number, got "deep"')
  const t = await load(configured)
  expect(t.row).toBe('configured')
  await t.unload()
  await owner.unload()
})

test('its listeners go with the row: gone after unloading, one when loaded again', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(watched)
  t.emit('watched/ping')
  await until(() => t.service('watched').hits() === 1)
  expect(t.service('watched').hits()).toBe(1)
  await t.unload()
  const t2 = await load(watched)
  t2.emit('watched/ping')
  await until(() => t2.service('watched').hits() === 1)
  expect(t2.service('watched').hits()).toBe(1)
  await t2.unload()
  await owner.unload()
})

test('the test hears what the row emits, as it hears a row of load', async () => {
  const { service } = parts()
  const h = host({ services: { parts: service } })
  const owner = await h.load(registrar)
  const heard: string[] = []
  owner.on('watched/ping', () => { heard.push('ping') })
  const t = await h.load(watched)
  t.emit('watched/ping')
  await until(() => heard.length > 0)
  expect(heard).toEqual(['ping'])
  await t.unload()
  await owner.unload()
  await h.unload()
})

test('the origin row unloading first leaves the row running (design §4)', async () => {
  const { handed, service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(handed[0])
  await owner.unload()
  expect(t.service('echo').say()).toBe('part')
  await t.unload()
})

test('a value no double received is refused', async () => {
  const { service } = parts()
  const owner = await load(registrar, { services: { parts: service } })
  const t = await load(sink)
  expect(await message(t.service('sink').register(part))).toBe('sink.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)')
  expect(await message(load({ [Symbol.for('rutis.plugin-definition')]: true }))).toBe('load needs a plugin definition a test double received')
  await t.unload()
  await owner.unload()
})

// What the host delivers arrives a little later (sdk.test.ts).
async function until(check: () => boolean) {
  for (let i = 0; i < 200 && !check(); i++) await new Promise(resolve => setTimeout(resolve, 5))
}

// A test that fails before unloading a definition row leaves nothing
// behind (#1004), as the row and service tests above do (#933).
const kept: any[] = []
const parts2 = { async register(definition: unknown) { kept.push(definition); return definition } }

test.failing('a test that fails before unloading a definition row', async () => {
  await load(registrar, { services: { parts: parts2 } })
  const t = await load(kept.at(-1))
  expect(t.service('echo').say()).toBe('part')
  throw new Error('failed before unload')
})

test('the next test loads the same definition again', async () => {
  const t = await load(kept.at(-1))
  expect(t.service('echo').say()).toBe('part')
  await t.unload()
})
