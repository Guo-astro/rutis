// A far end for the cross-language chunked tests (#1363): the Node runtime's
// own Session on a Unix socket, serving `conformance` as endpoint `node`,
// expecting `main`. It uses no package outside the runtime, so a Kotlin or Bun
// test can start it without installing the Node runtime's dependencies.
import { createConnection } from 'node:net'
import { Session } from '../../../../node/rutis-runtime/src/session.mjs'

const [channel] = process.argv.slice(2)
const at = channel.startsWith('unix:') ? channel.slice('unix:'.length) : channel
const socket = createConnection(at)
let calls = 0
let session
let buffered = ''

const dispatch = (target, method, args) => {
  if (target !== 'conformance') throw new Error(`no target ${target}`)
  switch (method) {
    case 'echo': return args[0]
    // How many calls ran, and how long the value was: a frame over the
    // message limit crosses in chunks and the call runs once, whole.
    case 'tally': calls += 1; return { calls, size: args[0].length }
    case 'big': return 'x'.repeat(args[0])
    default: throw new Error(`no method ${method}`)
  }
}

socket.setNoDelay(true)
socket.on('error', () => {})
socket.on('data', data => {
  buffered += data
  for (let end = buffered.indexOf('\n'); end >= 0; end = buffered.indexOf('\n')) {
    const line = buffered.slice(0, end)
    buffered = buffered.slice(end + 1)
    session.receive(JSON.parse(line))
  }
})
socket.on('close', () => process.exit(0))

session = new Session({
  send: text => socket.write(text + '\n'),
  pump: () => { throw new Error('a synchronous call is not expected here') },
  dispatch,
  abort: error => { process.stderr.write(`session fault: ${error?.message ?? error}\n`); process.exit(1) },
  endpoint: { local: 'node', expected: 'main' },
})
session.start()
session.ready.catch(error => { process.stderr.write(`handshake failed: ${error?.message ?? error}\n`); process.exit(1) })
