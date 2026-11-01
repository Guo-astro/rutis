// A definition with a config schema whose default the host fills (#1004):
// `load` without a config gives `depth` its default, and a config the schema
// refuses fails the load.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'configured',
  config: { type: 'object', properties: { depth: { type: 'number', default: 1 } } },
  provides: { configured: { depth: 'sync' } },
  apply(ctx, config) {
    ctx.provide('configured', { depth: () => config.depth })
  },
})
