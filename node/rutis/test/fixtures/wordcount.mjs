// A plugin that provides `notes`.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'wordcount',
  apply(ctx) {
    ctx.provide('notes', { async get() { return null } })
  },
})
