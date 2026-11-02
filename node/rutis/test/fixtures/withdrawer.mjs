// A plugin that provides `echo`, and withdraws it while it runs (#1030):
// the test calls `control.withdraw`, and another row that provides the same
// name takes it over.
import { definePlugin } from '../../src/index.mjs'

export default definePlugin({
  name: 'withdrawer',
  provides: { control: { withdraw: 'sync' }, echo: { say: 'sync' } },
  apply(ctx) {
    const withdraw = ctx.provide('echo', { say() { return 'A' } })
    ctx.provide('control', { withdraw })
  },
})
