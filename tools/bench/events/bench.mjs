// The Node row of the event benchmark (run.sh): the emitter and its
// listeners in one plugin, so in one runtime. Only the public plugin API
// (`ctx.on`, `ctx.emit`, `ctx.serial`, `ctx.bail`): the same file measures
// local dispatch and dispatch through the host.
//
// Config: { out, iterations, warmup }. Writes { mode: { median_us, p99_us } }
// as JSON to `out` when done.
import { writeFileSync } from 'node:fs'
import { definePlugin } from '../../../node/rutis/src/index.mjs'

const summary = samples => {
  samples.sort((a, b) => a - b)
  const at = q => samples[Math.min(samples.length - 1, Math.floor(q * samples.length))]
  return { median_us: at(0.5), p99_us: at(0.99), samples: samples.length }
}

export default definePlugin({
  apply(ctx, config) {
    const { out, iterations = 10_000, warmup = 1_000 } = config
    // emit: the time from `emit` until the listener has run.
    let received
    ctx.on('bench/emit', i => received(i))
    ctx.on('bench/serial', i => i)
    ctx.on('bench/bail', i => i)

    const modes = {
      emit: i => new Promise(resolve => { received = resolve; ctx.emit('bench/emit', i) }),
      serial: i => ctx.serial('bench/serial', i),
      bail: i => ctx.bail('bench/bail', i),
    }

    const run = async () => {
      const report = { runtime: 'node', node: process.version, iterations, warmup }
      for (const [mode, once] of Object.entries(modes)) {
        for (let i = 0; i < warmup; i++) await once(i)
        const samples = new Array(iterations)
        for (let i = 0; i < iterations; i++) {
          const start = process.hrtime.bigint()
          const value = await once(i)
          samples[i] = Number(process.hrtime.bigint() - start) / 1000
          if (value !== i) throw new Error(`${mode} returned ${value}, expected ${i}`)
        }
        report[mode] = summary(samples)
      }
      writeFileSync(out, JSON.stringify(report, null, 2))
    }
    // After apply, so that the row is running when the measurement starts.
    setTimeout(() => run().catch(error => writeFileSync(out, JSON.stringify({ error: String(error?.stack ?? error) }))), 0)
  },
})
