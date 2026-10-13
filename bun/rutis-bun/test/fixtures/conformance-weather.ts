// The runtime conformance plugin (rutis_bridge::runtime::testing::runtime),
// for the Bun runtime. What calls on it return is in
// crates/rutis-bridge/src/runtime/testing/weather.expected.json.
export const inject = ['clock']
export const provides = { weather: { today: 'sync', later: 'async', each: 'sync', echo: 'sync', crash: 'sync' } }
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
  })
}
