// A plugin with an optional service, for the test tool under Bun: its
// `summary` says what it saw.
import { definePlugin } from '../../../../../node/rutis/src/index.mjs'

const said = (run: () => unknown) => {
  try { run() } catch (caught: any) { return [caught.message, caught.code, caught.data] }
}

export default definePlugin({
  name: 'summary',
  inject: ['notes'],
  optional: ['llm'],
  apply(ctx: any) {
    const llm = ctx.get('llm')
    const seen = {
      llm: llm !== undefined,
      use: said(() => ctx.use('llm')),
      get: said(() => ctx.get('other')),
    }
    ctx.provide('summary', {
      async seen() { return seen },
      async ask(question: string) { return llm ? llm.ask(question) : 'none' },
    })
  },
})
