// A leaf plugin for test/output.test.mjs (#678): it writes as it applies,
// and a row labelled `<label>` provides `talk-<label>`, whose methods write
// (now, after a timer, in pieces, to stderr) or keep the thread busy.
import { definePlugin } from '../../../rutis/src/index.mjs'

export default definePlugin({
  name: 'output-rows',
  apply(ctx, config) {
    console.log(`applied ${config.label}`)
    ctx.provide(`talk-${config.label}`, {
      say(text) {
        console.log(text)
        return null
      },
      shout(text) {
        console.error(text)
        return null
      },
      async later(text) {
        await new Promise(resolve => setTimeout(resolve, 10))
        console.log(`later ${text}`)
        return null
      },
      pieces() {
        process.stdout.write('one\ntwo\nthr')
        process.stdout.write('ee\n')
        return null
      },
      block(ms) {
        const end = Date.now() + ms
        while (Date.now() < end) {}
        return null
      },
    })
  },
})
