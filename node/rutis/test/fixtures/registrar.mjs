// A plugin that hands a plugin definition to a test's service (#1004).
import { definePlugin } from '../../src/index.mjs'
import part from './part.mjs'

export default definePlugin({
  name: 'registrar',
  inject: ['parts'],
  async apply(ctx) {
    const back = await ctx.use('parts').register(part)
    // The test double hands the definition back: the plugin gets its own.
    if (back !== part) throw new Error('the definition came back as another value')
  },
})
