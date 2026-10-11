// Dials `address` and makes one long synchronous call: the main thread
// waits in it while the I/O worker must keep answering heartbeats.
import { Process } from '../../src/client.mjs'

const [address] = process.argv.slice(2)
const session = await Process.connect(address, () => { throw new Error('nothing here') }, undefined, { local: 'node', expected: 'main' })
// More than twice the heartbeat timeout the test sets (1 s).
const slept = session.call('svc', 'sleep', [2500])
process.stdout.write(`survived ${slept}\n`)
await session.dispose().catch(() => {})
process.exit(0)
