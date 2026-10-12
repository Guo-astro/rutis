// The runtime conformance plugin (rutis_bridge::runtime::testing::runtime), for
// the Node runtime. What calls on it return is in
// crates/rutis-bridge/src/runtime/testing/weather.expected.json; the real
// runtime and the testing tool (@arcships/rutis/testing,
// node/rutis/test/conformance.test.mjs) are both checked against it. It
// imports the SDK by path, so it is loaded where it is, not copied.
import { definePlugin } from '../../../rutis/src/index.mjs'

// Values only JavaScript makes, by name (`samples` in the expectations).
const samples = {
  'node/date': () => new Date(0),
  'node/map': () => new Map([['a', 1]]),
  'node/bytes': () => new Uint8Array([1, 2]),
  'node/nan': () => NaN,
  'node/undefined-in-data': () => ({ a: undefined, b: [undefined] }),
  'node/error-value': () => new RangeError('out of range'),
}

export default definePlugin({
  inject: ['clock'],
  provides: {
    weather: { today: 'sync', later: 'async', each: 'sync', echo: 'sync', crash: 'sync', samples: 'sync', sample: 'sync' },
  },
  apply(ctx) {
    const clock = ctx.use('clock')
    ctx.provide('weather', {
      today() { return `Oslo at ${clock.now()}` },
      async later() { return 'Oslo later' },
      each(callback) { return ['mon', 'tue'].map(day => callback(day)) },
      echo(value) { return value },
      crash() { process.exit(17) },
      samples() { return Object.keys(samples) },
      sample(name) { return samples[name]() },
    })
  },
})
