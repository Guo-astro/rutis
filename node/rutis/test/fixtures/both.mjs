// A plugin declaring one name both required and optional.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'both',
  inject: ['llm'],
  optional: ['llm'],
  apply() {},
})
