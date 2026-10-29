// A plugin with no name, which uses a service it does not declare.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  apply(ctx) { ctx.use('llm') },
})
