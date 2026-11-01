// A plugin that makes plugin definitions and hands them to the test's
// service (#1004): what the tests load are these, as this runtime made
// them, not the test process's own copies. Each is handed back by the
// double unchanged, and the plugin checks it got its own back.
import { definePlugin } from '../../src/index.mjs'
import part from './part.mjs'
import quiet from './quiet.mjs'
import watched from './watched.mjs'
import configured from './configured.mjs'
import gated from './gated.mjs'

export default definePlugin({
  name: 'handover',
  inject: ['parts'],
  async apply(ctx) {
    const parts = ctx.use('parts')
    for (const definition of [part, quiet, watched, configured, gated]) {
      const back = await parts.register(definition.name, definition)
      if (back !== definition) throw new Error(`the test handed back another definition for ${definition.name}`)
    }
  },
})
