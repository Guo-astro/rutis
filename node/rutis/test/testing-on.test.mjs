// The testing tool's `on` (#670 C4): the test listens as the host does.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '../src/index.mjs'
import { host, load } from '../src/testing.mjs'

const weather = definePlugin({
  name: 'weather',
  inject: [],
  apply(ctx, config) {
    ctx.provide('weather', {
      async today() {
        ctx.emit('weather/asked', config.city)
        return 'sunny'
      },
      async ask(place) {
        return ctx.serial('weather/answer', place)
      },
      async tell(entry) {
        ctx.emit('weather/told', entry)
        return entry.city
      },
    })
  },
})

test('t.on hears the plugin\'s emissions of the name', async () => {
  const t = await load(weather, { config: { city: 'Oslo' } })
  const asked = []
  const off = t.on('weather/asked', city => { asked.push(city) })
  await t.service('weather').today()
  assert.deepEqual(asked, ['Oslo'])
  off()
  await t.unload()
})

test('t.on hears the test\'s own emissions', async () => {
  const t = await load(weather, { config: { city: 'Oslo' } })
  const heard = []
  t.on('weather/asked', (city, n) => { heard.push([city, n]) })
  t.emit('weather/asked', 'Bergen', 2)
  assert.equal(await t.parallel('weather/asked', 'Tromsø', 3), undefined)
  assert.deepEqual(heard, [['Bergen', 2], ['Tromsø', 3]])
  await t.unload()
})

test('strict, the listener gets the arguments as another process would', async () => {
  const t = await load(weather, { config: { city: 'Oslo' } })
  const entry = { city: 'Oslo', at: undefined }
  let got
  t.on('weather/told', received => { got = received })
  await t.service('weather').tell(entry)
  // A copy, with undefined inside a value as null, as the runtimes send it.
  assert.notEqual(got, entry)
  assert.deepEqual(got, { city: 'Oslo', at: null })
  // Not strict: the object itself.
  const loose = await load(weather, { config: { city: 'Oslo' }, strict: false })
  let same
  loose.on('weather/told', received => { same = received })
  const value = { city: 'Oslo' }
  await loose.service('weather').tell(value)
  assert.equal(same, value)
  await t.unload()
  await loose.unload()
})

test('the returned function stops it', async () => {
  const t = await load(weather, { config: { city: 'Oslo' } })
  const asked = []
  const off = t.on('weather/asked', city => { asked.push(city) })
  off()
  off()
  await t.service('weather').today()
  assert.deepEqual(asked, [])
  await t.unload()
})

test('it stops when the row unloads', async () => {
  const h = host()
  const t = await h.load(weather, { config: { city: 'Oslo' } })
  const other = await h.load(weather, { config: { city: 'Bergen' }, row: 'second' })
  const asked = []
  t.on('weather/asked', city => { asked.push(city) })
  await t.unload()
  await other.service('weather').today()
  assert.deepEqual(asked, [])
  assert.throws(() => t.on('weather/asked', () => {}), /the plugin is unloaded/)
  await h.unload()
})

test('in serial, its result counts', async () => {
  const t = await load(weather, { config: { city: 'Oslo' } })
  t.on('weather/answer', async place => `rain in ${place}`)
  assert.equal(await t.service('weather').ask('Oslo'), 'rain in Oslo')
  // Options as ctx.on: a prepended listener is asked first.
  t.on('weather/answer', place => `snow in ${place}`, { prepend: true })
  assert.equal(await t.service('weather').ask('Oslo'), 'snow in Oslo')
  await t.unload()
})
