// The Bun side of the channel contract (crates/rutis-bridge/tests/bun_channel.rs):
// dials the two Unix sockets given as arguments with this runtime's channel
// and relays every message between them, in order, both ways. When either
// ends, the other is closed, and the process exits once both have closed.
import { open, type Channel } from '../../src/channel.ts'

const [first, second] = process.argv.slice(2)
let a: Channel | undefined, b: Channel | undefined
let ended = false
// What reaches `a` before `b` is connected waits for it, in order.
const early: string[] = []
// Each channel reports its close once; `b` is closed as soon as it opens
// when `a` ended first.
let closings = 0
const done = () => {
  ended = true
  a?.close(); b?.close()
  if (++closings === 2) process.exit(0)
}
a = await open(first, { message: text => (b ? b.send(text) : early.push(text)), closed: done })
b = await open(second, { message: text => a!.send(text), closed: done })
for (const text of early.splice(0)) b.send(text)
if (ended) b.close()
