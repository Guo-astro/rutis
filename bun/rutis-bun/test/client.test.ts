import { expect, test } from 'bun:test'
import { Client } from '../src/client.ts'
import { within } from './support.ts'

test('a worker that crashes ends the session, saying so', async () => {
  const client = await Client.connect('unused', {
    dispatch: () => null,
    worker: new URL('./fixtures/crashing-worker.ts', import.meta.url),
  })
  await within('the session to end', client.closed())
  expect(client.reason).toContain('communication worker exited')
  expect(() => client.call('t', 'm', [])).toThrow()
})

test('a frame the session refuses ends the channel, saying why', async () => {
  const { createServer } = await import('node:net')
  const { mkdtempSync } = await import('node:fs')
  const { tmpdir } = await import('node:os')
  const { join } = await import('node:path')
  const path = join(mkdtempSync(join(tmpdir(), 'rutis-bun-')), 'far.sock')
  let farEnded!: () => void
  const ended = new Promise<void>(resolve => { farEnded = resolve })
  const server = createServer(socket => {
    // The host's side: greet, then, once the session has greeted too, send
    // one call twice (a repeated id).
    const call = JSON.stringify({ op: 'invoke', id: 'rust:1', path: [], target: 't', method: 'm', args: { type: 'list', value: [] } })
    socket.write(JSON.stringify({ op: 'hello', version: 2 }) + '\n')
    socket.once('data', () => socket.write(call + '\n' + call + '\n'))
    socket.on('data', () => {})
    socket.on('close', farEnded)
  })
  await new Promise<void>(resolve => server.listen(path, resolve))
  const client = await Client.connect(path, { dispatch: () => 'ok' })
  await within('the session to end', client.closed())
  await within('the far end to see the channel end', ended)
  server.close()
  expect(client.reason).toContain('session fault')
  expect(client.reason).toContain('repeated invocation identity')
})

// guarantee: sdk/error-codes
// The host going away during a call: the call fails with what happened, in
// plain words, and the code for programs (§4.3, #677).
test('a call in progress when the host disconnects fails with peer-gone', async () => {
  const { createServer } = await import('node:net')
  const { mkdtempSync } = await import('node:fs')
  const { tmpdir } = await import('node:os')
  const { join } = await import('node:path')
  const path = join(mkdtempSync(join(tmpdir(), 'rutis-bun-')), 'far.sock')
  const server = createServer(socket => {
    socket.write(JSON.stringify({ op: 'hello', version: 2 }) + '\n')
    let seen = ''
    // The call arrives: the host goes.
    socket.on('data', chunk => { seen += chunk; if (seen.includes('"invoke"')) socket.destroy() })
  })
  await new Promise<void>(resolve => server.listen(path, resolve))
  const client = await Client.connect(path, { dispatch: () => 'ok' })
  const failed: any = await within('the call to fail', client.callAsync('host:llm', 'ask', [])).catch(error => error)
  server.close()
  expect([failed?.name, failed?.message, failed?.code]).toEqual(['BindingError', 'the host disconnected', 'peer-gone'])
})
