// A TypeScript row using the application's Rust `Calendar` by name. Its
// methods and types come from rows/services.d.ts, which `rutis-host types`
// writes from the Rust definitions.
import { definePlugin } from '@arcships/rutis'
import type { CalendarErrorThrown, Services } from './services.js'

interface Probe {
  record(line: string): void
}

export default definePlugin({
  inject: ['calendar', 'probe'],
  async apply(ctx) {
    const calendar = ctx.use<Services['calendar']>('calendar')
    const probe = ctx.use<Probe>('probe')
    const id = await calendar.add({ title: 'Standup', startsAt: 1_760_000_000_000 })
    const entry = await calendar.get(id)
    const titles: string[] = []
    const count = await calendar.each(async (entry) => {
      titles.push(entry.title)
    })
    probe.record(`ts: ${entry.title} at ${entry.startsAt}, ${count} ${titles.join(',')}`)
    try {
      await calendar.get(99)
    } catch (error) {
      const thrown = error as CalendarErrorThrown
      probe.record(`ts: ${thrown.name} ${thrown.code} ${thrown.message}`)
    }
  },
})
