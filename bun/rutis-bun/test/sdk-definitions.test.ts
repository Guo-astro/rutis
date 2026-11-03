// A plugin definition a test double receives goes straight to `load`
// (#1004) under `bun test` too: it is mounted as a row of the runtime that
// made it, as a plugin module is (node/rutis/test/testing-plugins.test.mjs,
// docs/design-test-tool-definition-load-2026-11-01.md §4). Every case loads
// the handle a double received, never a module the test process imported.
import { expect, test } from 'bun:test'
import { host, load } from '../../../node/rutis/src/testing.mjs'
import handover from '../../../node/rutis/test/fixtures/handover.mjs'
import part from '../../../node/rutis/test/fixtures/part.mjs'
import gatekeeper from '../../../node/rutis/test/fixtures/gatekeeper.mjs'
import sink from '../../../node/rutis/test/fixtures/sink.mjs'

const parts = () => {
  const handed = new Map<string, any>()
  return { handed, service: { async register(name: string, definition: unknown) { handed.set(name, definition); return definition } } }
}

const message = async (promise: Promise<unknown>) => promise.then(() => 'loaded', (error: Error) => error.message)

// What the host delivers arrives a little later (sdk.test.ts).
async function until(check: () => boolean) {
  for (let i = 0; i < 200 && !check(); i++) await new Promise(resolve => setTimeout(resolve, 5))
}

test('a definition a double received is mounted by load, and the plugin gets it back', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  // `handover` checks each came back as itself; all five arrived here.
  expect([...handed.keys()].sort()).toEqual(['configured', 'gated', 'part', 'quiet', 'watched'])
  const t = await load(handed.get('part'))
  expect(t.row).toBe('part')
  expect(t.provided()).toEqual(['echo'])
  expect(t.service('echo').say()).toBe('part')
  await t.unload()
  await owner.unload()
})

test('the same definition loaded twice is two rows, each unloaded on its own', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const first = await load(handed.get('quiet'))
  const second = await load(handed.get('quiet'))
  expect([first.row, second.row]).toEqual(['quiet', 'quiet-2'])
  await first.unload()
  expect(second.provided()).toEqual([])
  await second.unload()
  await owner.unload()
})

test('an explicit id that a row already has fails, naming the row', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(handed.get('quiet'), { id: 'mine' })
  expect(await message(load(handed.get('quiet'), { id: 'mine' }))).toBe('a row mine is loaded already')
  await t.unload()
  await owner.unload()
})

test('a definition gets the config defaults its schema declares', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(handed.get('configured'))
  expect(t.service('configured').depth()).toBe(1)
  await t.unload()
  await owner.unload()
})

test('a config the definition’s schema refuses fails the load, and the definition loads after', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  expect(await message(load(handed.get('configured'), { config: { depth: 'deep' } }))).toBe('configured: config: depth should be a number, got "deep"')
  // The failed load left nothing behind: its registration is released, and
  // the same definition loads again under the same row id.
  const t = await load(handed.get('configured'))
  expect(t.row).toBe('configured')
  await t.unload()
  await owner.unload()
})

test('its listeners go with the row: gone after unloading, one when loaded again', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(handed.get('watched'))
  t.emit('watched/ping')
  await until(() => t.service('watched').hits() === 1)
  expect(t.service('watched').hits()).toBe(1)
  await t.unload()
  const t2 = await load(handed.get('watched'))
  t2.emit('watched/ping')
  await until(() => t2.service('watched').hits() === 1)
  expect(t2.service('watched').hits()).toBe(1)
  await t2.unload()
  await owner.unload()
})

test('the test hears what the row emits, as it hears a row of load', async () => {
  const { handed, service } = parts()
  const h = host({ services: { parts: service } })
  const owner = await h.load(handover)
  const heard: string[] = []
  owner.on('watched/ping', () => { heard.push('ping') })
  const t = await h.load(handed.get('watched'))
  t.emit('watched/ping')
  await until(() => heard.length > 0)
  expect(heard).toEqual(['ping'])
  await t.unload()
  await owner.unload()
  await h.unload()
})

test('the origin row unloading first leaves the row running (design §4)', async () => {
  const { handed, service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(handed.get('part'))
  await owner.unload()
  expect(t.service('echo').say()).toBe('part')
  await t.unload()
})

test('a definition whose injected service is not there waits, and starts when it arrives', async () => {
  const { handed, service } = parts()
  const h = host({ services: { parts: service } })
  const owner = await h.load(handover)
  const t = await h.load(handed.get('gated'))
  // Waiting, not failed: the load answered, the host reported nothing, and
  // the service is not provided yet. The row's line is the host's, the one
  // `dev` prints (#933).
  expect(t.provided()).toEqual([])
  expect(await t.reported()).toEqual([])
  expect(await t.waiting()).toBe('gated: waiting for later (no row provides it)')
  expect(() => t.service('gated')).toThrow('gated: waiting for later (no row provides it)')
  const provider = await h.load(gatekeeper)
  await until(() => t.provided().includes('gated'))
  expect(t.provided()).toEqual(['gated'])
  expect(await t.waiting()).toBeNull()
  expect(t.service('gated').ping()).toBe(7)
  await provider.unload()
  await t.unload()
  await owner.unload()
  await h.unload()
})

test('a value no double received is refused', async () => {
  const { service } = parts()
  const owner = await load(handover, { services: { parts: service } })
  const t = await load(sink)
  expect(await message(t.service('sink').register(part))).toBe('sink.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)')
  expect(await message(load({ [Symbol.for('rutis.plugin-definition')]: true }))).toBe('load needs a plugin definition a test double received')
  await t.unload()
  await owner.unload()
})

// A test that fails before unloading a definition row leaves nothing
// behind (#1004), as the row and service tests do (#933).
const leaked = new Map<string, any>()
const kept = { async register(name: string, definition: unknown) { leaked.set(name, definition); return definition } }

test.failing('a test that fails before unloading a definition row', async () => {
  await load(handover, { services: { parts: kept } })
  const t = await load(leaked.get('part'))
  expect(t.service('echo').say()).toBe('part')
  throw new Error('failed before unload')
})

test('the next test loads the same definition again', async () => {
  const t = await load(leaked.get('part'))
  expect(t.service('echo').say()).toBe('part')
  await t.unload()
})

