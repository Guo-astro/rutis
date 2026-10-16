// The runtime conformance plugin (rutis_bridge::runtime::testing::runtime),
// for the Bun runtime. What calls on it return is in
// crates/rutis-bridge/src/runtime/testing/weather.expected.json.

// What the SDK constructors (`rutis.date(…)`, ... in @arcships/rutis)
// make: this file is copied out of the repository to run, so it marks its
// values itself, as the SDK does.
const value = (tag: string, text: string) => Object.freeze({ [Symbol.for('rutis.value')]: tag, value: text })

// Values only JavaScript makes, by name (`samples` in the expectations):
// the tagged values (#308), sent as on Node.
const samples: Record<string, () => unknown> = {
  'node/value-instant': () => new Date('2026-10-15T03:22:15.123Z'),
  'node/value-bytes': () => new TextEncoder().encode('hello'),
  'node/value-map': () => new Map<unknown, unknown>([[1, 'a'], ['1', 'b'], [true, undefined]]),
  'node/value-set': () => new Set(['a', 1]),
  'node/value-bigint': () => -123456789012345678901n,
  'node/value-constructed': () => [
    value('datetime', '2026-10-15T14:30:05.25'), value('date', '2026-10-15'), value('time', '14:30:05.25'),
    value('decimal', '1.10'), value('uuid', '0f8fad5b-d9cb-469f-a165-70867728950e'),
  ],
  'node/value-date-key': () => new Map([[new Date(0), 'epoch']]),
  'node/value-invalid-date': () => new Date(NaN),
  'node/value-unsafe-key': () => new Map([[2 ** 53, 'a']]),
  'node/value-duplicate-elements': () => new Set([value('date', '2026-10-15'), value('date', '2026-10-15')]),
  'node/value-typed-array': () => new Int16Array([1, 2]),
}

export const inject = ['clock']
export const provides = { weather: { today: 'sync', later: 'async', each: 'sync', echo: 'sync', crash: 'sync', samples: 'sync', sample: 'sync' } }
export function apply(ctx: { use(name: string): any, provide(name: string, value: unknown): void, on(name: string, listener: (...args: any[]) => unknown): () => void }) {
  const clock = ctx.use('clock')
  let city = 'Oslo'
  // `events` in the expectations: the host changes the city.
  ctx.on('weather/city', (name: string) => { city = name })
  ctx.provide('weather', {
    today() { return `${city} at ${clock.now()}` },
    async later() { return 'Oslo later' },
    each(callback: (day: string) => string) { return ['mon', 'tue'].map(day => callback(day)) },
    echo(value: unknown) { return value },
    crash() { process.exit(17) },
    samples() { return Object.keys(samples) },
    sample(name: string) { return samples[name]() },
  })
}
