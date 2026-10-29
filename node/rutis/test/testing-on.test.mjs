// The test tool's `on` (#670 C4): the test listens as a plugin next to the
// one under test does.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host, load } from '../src/testing.mjs'
import talker from './fixtures/talker.mjs'

// Wait until `check()` holds: what the host delivers arrives a little later.
async function until(check) {
  for (let i = 0; i < 200 && !check(); i++) await new Promise(resolve => setTimeout(resolve, 5))
}

test('t.on hears the plugin\'s emissions of the name', async () => {
  const t = await load(talker, { config: { city: 'Oslo' } })
  const asked = []
  const off = t.on('talker/asked', city => { asked.push(city) })
  await t.service('talker').today()
  await until(() => asked.length)
  assert.deepEqual(asked, ['Oslo'])
  off()
  await t.unload()
})

test('t.on hears the test\'s own emissions', async () => {
  const t = await load(talker, { config: { city: 'Oslo' } })
  const heard = []
  t.on('talker/asked', (city, n) => { heard.push([city, n]) })
  t.emit('talker/asked', 'Bergen', 2)
  assert.equal(await t.parallel('talker/asked', 'Tromsø', 3), undefined)
  await until(() => heard.length === 2)
  assert.deepEqual(heard, [['Bergen', 2], ['Tromsø', 3]])
  await t.unload()
})

test('the listener gets the arguments as from another process', async () => {
  const t = await load(talker, { config: { city: 'Oslo' } })
  const entry = { city: 'Oslo', at: undefined }
  let got
  t.on('talker/told', received => { got = received })
  await t.service('talker').tell(entry)
  await until(() => got)
  // A copy, with undefined inside a value as null, as the runtimes send it.
  assert.notEqual(got, entry)
  assert.deepEqual(got, { city: 'Oslo', at: null })
  await t.unload()
})

test('the returned function stops it, and so does unloading the row', async () => {
  const h = host()
  const t = await h.load(talker, { config: { city: 'Oslo' } })
  const asked = []
  const off = t.on('talker/asked', city => { asked.push(city) })
  off()
  off()
  await t.service('talker').today()
  t.on('talker/asked', city => { asked.push(city) })
  await t.unload()
  h.emit('talker/asked', 'Bergen')
  await new Promise(resolve => setTimeout(resolve, 50))
  assert.deepEqual(asked, [])
  assert.throws(() => t.on('talker/asked', () => {}), /the plugin is unloaded/)
  await h.unload()
})

test('in serial, its result counts; prepend puts it first', async () => {
  const t = await load(talker, { config: { city: 'Oslo' } })
  t.on('talker/answer', async place => `rain in ${place}`)
  assert.equal(await t.service('talker').ask('Oslo'), 'rain in Oslo')
  t.on('talker/answer', place => `snow in ${place}`, { prepend: true })
  assert.equal(await t.service('talker').ask('Oslo'), 'snow in Oslo')
  await t.unload()
})
