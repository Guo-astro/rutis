// The runtime conformance plugin (rutis_bridge::runtime::testing::runtime), for
// the Node runtime. What calls on it return is in
// crates/rutis-bridge/src/runtime/testing/weather.expected.json; the real
// runtime and the testing tool (@arcships/rutis/testing,
// node/rutis/test/conformance.test.mjs) are both checked against it. It
// imports the SDK by path, so it is loaded where it is, not copied.
import * as rutis from '../../../rutis/src/index.mjs'
import { definePlugin } from '../../../rutis/src/index.mjs'

// Values only JavaScript makes, by name (`samples` in the expectations).
const samples = {
  'node/nan': () => NaN,
  'node/undefined-in-data': () => ({ a: undefined, b: [undefined] }),
  'node/error-value': () => new RangeError('out of range'),
  // Tagged values (#308).
  'node/value-instant': () => new Date('2026-10-15T03:22:15.123Z'),
  'node/value-bytes': () => new TextEncoder().encode('hello'),
  'node/value-map': () => new Map([[1, 'a'], ['1', 'b'], [true, undefined]]),
  'node/value-set': () => new Set(['a', 1]),
  'node/value-bigint': () => -123456789012345678901n,
  'node/value-constructed': () => [
    rutis.datetime('2026-10-15T14:30:05.25'), rutis.date('2026-10-15'), rutis.time('14:30:05.25'),
    rutis.decimal('1.10'), rutis.uuid('0f8fad5b-d9cb-469f-a165-70867728950e'),
  ],
  'node/value-date-key': () => new Map([[new Date(0), 'epoch']]),
  'node/value-invalid-date': () => new Date(NaN),
  'node/value-unsafe-key': () => new Map([[2 ** 53, 'a']]),
  'node/value-duplicate-elements': () => new Set([rutis.date('2026-10-15'), rutis.date('2026-10-15')]),
  'node/value-typed-array': () => new Int16Array([1, 2]),
}

export default definePlugin({
  inject: ['clock'],
  provides: {
    weather: { today: 'sync', later: 'async', each: 'sync', echo: 'sync', crash: 'sync', samples: 'sync', sample: 'sync' },
  },
  apply(ctx) {
    const clock = ctx.use('clock')
    let city = 'Oslo'
    // `events` in the expectations: the host changes the city.
    ctx.on('weather/city', name => { city = name })
    ctx.provide('weather', {
      today() { return `${city} at ${clock.now()}` },
      async later() { return 'Oslo later' },
      each(callback) { return ['mon', 'tue'].map(day => callback(day)) },
      echo(value) { return value },
      crash() { process.exit(17) },
      samples() { return Object.keys(samples) },
      sample(name) { return samples[name]() },
    })
  },
})
