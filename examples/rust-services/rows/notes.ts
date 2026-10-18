// A TypeScript row inside each `session` instance: `notes` is that
// instance's own Rust `Notes`.
import { definePlugin } from '@arcships/rutis'
import type { Services } from './services.js'

export default definePlugin({
  inject: ['notes', 'probe'],
  async apply(ctx) {
    const notes = ctx.use<Services['notes']>('notes')
    const probe = ctx.use<{ record(line: string): void }>('probe')
    const owner = await notes.owner()
    const count = await notes.write(`hello from ${owner}`)
    probe.record(`notes of ${owner}: ${count} ${(await notes.read()).join('; ')}`)
  },
})
