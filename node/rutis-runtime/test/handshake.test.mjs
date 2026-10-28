import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { WebSocketServer } from 'ws'
import { Process } from '../src/client.mjs'
import { frame } from '../src/channel/unix.mjs'
import { decode } from '../src/codec.mjs'
import { Session } from '../src/session.mjs'
import { fakeHost } from './fixtures/fake-host.mjs'
import { within } from './support.mjs'

const runner = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
const conformance = fileURLToPath(new URL('./fixtures/conformance-events.mjs', import.meta.url))

// A local channel given `--id` speaks the endpoint format and greets as
// that id, declaring `runtime`: an inherited socket (`fd:3`, as rutis
// starts a runtime) and a socket path (the fake host's).
test('a local channel given --id greets in the endpoint format as that id', async () => {
  const child = spawn(process.execPath, [runner, 'fd:3', '--id', 'weather', '--peer', 'host', conformance], { stdio: ['ignore', 'ignore', 'pipe', 'pipe'] })
  let stderr = ''
  child.stderr.on('data', chunk => { stderr += chunk })
  const exited = new Promise(resolve => child.once('exit', resolve))
  const session = new Session({
    send: text => channel.send(text),
    pump: () => { throw new Error('no synchronous calls') },
    dispatch: () => undefined,
    endpoint: { local: 'host', expected: 'weather' },
  })
  const channel = frame(child.stdio[3], {
    message: text => session.receive(decode(text)),
    closed: () => session.close(new Error('the runtime disconnected')),
  })
  try {
    session.start()
    await within(`the handshake (${stderr})`, session.ready)
    assert.equal(session.greeting.endpoint, 'weather')
    assert.ok(session.greeting.capabilities.includes('runtime'))
  } finally {
    channel.end()
    await within('the runtime to stop', exited)
  }

  const host = await fakeHost([process.execPath, runner, conformance])
  try {
    await within('the handshake', host.ready)
    assert.equal(host.session.greeting.endpoint, 'rt')
  } finally {
    await host.end()
  }
})

// The far end greets the moment it accepts, so its hello can arrive in the
// same read as the upgrade: this side still says hello before anything else.
test('a dialer greets first even when the far end greets at once', async () => {
  for (let round = 0; round < 20; round++) {
    const server = new WebSocketServer({ host: '127.0.0.1', port: 0, handleProtocols: protocols => [...protocols][0] })
    let peer
    try {
      await new Promise(resolve => server.once('listening', resolve))
      const first = new Promise(resolve => server.once('connection', socket => {
        socket.send(JSON.stringify({ op: 'hello', version: 3, endpoint: 'far', implementation: { name: 'test', version: '0' }, capabilities: [] }))
        socket.once('message', data => resolve(JSON.parse(data.toString())))
      }))
      peer = await Process.connect(`ws://127.0.0.1:${server.address().port}/rutis`, () => undefined, undefined, { local: 'near' })
      peer.callAsync('x', 'y', []).catch(() => {})
      assert.equal((await within('the first message', first)).op, 'hello', `round ${round}`)
    } finally {
      // The far end goes; the session ends with it.
      for (const client of server.clients) client.terminate()
      await peer?.closed()
      await new Promise(resolve => server.close(resolve))
    }
  }
})
