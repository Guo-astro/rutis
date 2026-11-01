// A plugin with an optional service: its `summary` says what it saw.
import { definePlugin } from '../../src/index.mjs'

const said = run => {
  try { run() } catch (caught) { return [caught.message, caught.code, caught.data] }
}

export default definePlugin({
  name: 'summary',
  inject: ['notes'],
  optional: ['llm'],
  apply(ctx) {
    const llm = ctx.get('llm')
    const seen = {
      llm: llm !== undefined,
      use: said(() => ctx.use('llm')),
      get: said(() => ctx.get('other')),
    }
    ctx.provide('summary', {
      async seen() { return seen },
      async ask(question) { return llm ? llm.ask(question) : 'none' },
    })
  },
})
