// A TypeScript row using the application's Rust `Calendar` by name. Its
// methods and types come from rows/services.d.ts, generated from the Rust
// definitions; the application wrote no binding for it.
import { definePlugin } from '@arcships/rutis'
import type { CalendarErrorThrown, Change, Services } from './services.js'

interface Probe {
  record(line: string): void
}

const delay = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms))

export default definePlugin({
  inject: ['calendar', 'probe'],
  async apply(ctx) {
    const calendar = ctx.use<Services['calendar']>('calendar')
    const probe = ctx.use<Probe>('probe')

    // A serde struct in, a serde struct out.
    const added = await calendar.add({ title: 'Standup', startsAt: 1_760_000_000_000, tags: ['team'], kind: 'meeting' })
    probe.record(`added ${added.title} ${added.kind} ${added.tags?.join(',')}`)
    // An async method.
    const got = await calendar.get(added.id)
    probe.record(`got ${got.title} at ${got.startsAt}`)
    // `Err(e)` is thrown: code, message and data.
    try {
      await calendar.get(99_999)
    } catch (error) {
      const thrown = error as CalendarErrorThrown
      probe.record(`missing ${thrown.name} ${thrown.code} ${thrown.message} ${JSON.stringify(thrown.data)}`)
    }
    try {
      await calendar.add({ title: ' ', startsAt: 0 })
    } catch (error) {
      const thrown = error as CalendarErrorThrown
      probe.record(`refused ${thrown.code} ${thrown.message}`)
    }
    // An optional parameter left out; a `&str` parameter.
    const listed = await calendar.list()
    probe.record(`listed ${listed.some(entry => entry.id === added.id)}`)
    probe.record(`tagged ${(await calendar.list('team')).length > 0}`)
    probe.record(`renamed ${(await calendar.rename(added.id, 'Retro')).title}`)
    // Beyond 2^53 an integer arrives as a bigint.
    const last = await calendar.lastId()
    probe.record(`last ${typeof last} ${last}`)
    // A function passed by reference: each visit settles before the next.
    const visited: string[] = []
    await calendar.each(async entry => {
      await delay(5)
      visited.push(entry.title)
    })
    probe.record(`visited ${visited.includes('Retro')}`)

    // A subscriber to the calendar's changes: the Rust side's publish is
    // done only once this handler settled, and fails when it throws.
    await calendar.subscribe(async (change: Change) => {
      await delay(50)
      if (change.type === 'removed' && change.id === 13) throw new Error('handler refused')
      await probe.record(change.type === 'added' ? `handled added ${change.entry.title}` : `handled removed ${change.id}`)
    })
    probe.record('subscribed')
  },
})
