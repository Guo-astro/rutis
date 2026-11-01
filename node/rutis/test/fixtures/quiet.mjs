// A definition with nothing of its own, for the tests that load one twice.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'quiet',
  apply() {},
})
