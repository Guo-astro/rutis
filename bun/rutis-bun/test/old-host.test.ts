// guarantee: events/old-peer-refused — the Bun runtime's side (#464,
// protocol §2.1, §2.2): a host that does not declare `events-v2` (one of
// rutis 0.12 or older) is refused right after the handshake: the runtime sends nothing after its own hello, says so on stderr
// with the §2.2 line and exits with status 1. A real runtime process
// (src/main.ts), against a host that only greets.
import { expect, test } from 'bun:test'
import { createServer, type Socket } from 'node:net'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { within } from './support.ts'
import { ENDPOINT_PROTOCOL } from '../src/session.ts'

const MAIN = new URL('../src/main.ts', import.meta.url).pathname
const REFUSAL = 'rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping'

// Run the runtime on a Unix socket whose far end greets with `hello`; what
// the runtime sent, its stderr and its exit status.
async function meet(hello: object, flags: string[] = []) {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-old-host-'))
  try {
    writeFileSync(join(dir, 'package.json'), '{}')
    const path = join(dir, 'host.sock')
    const received: any[] = []
    let socket!: Socket
    const server = createServer(connection => {
      socket = connection
      let buffer = ''
      connection.on('data', chunk => {
        buffer += chunk
        for (let end = buffer.indexOf('\n'); end >= 0; end = buffer.indexOf('\n')) {
          received.push(JSON.parse(buffer.slice(0, end)))
          buffer = buffer.slice(end + 1)
        }
      })
      connection.on('error', () => {})
      connection.write(JSON.stringify(hello) + '\n')
    })
    await new Promise<void>(resolve => server.listen(path, resolve))
    const child = Bun.spawn([process.execPath, '--no-install', '--no-env-file', MAIN, path, ...flags, dir], {
      stdin: 'ignore', stdout: 'ignore', stderr: 'pipe',
    })
    const code = await within('the runtime to stop', child.exited)
    const stderr = await new Response(child.stderr).text()
    socket?.destroy()
    server.close()
    return { code, stderr, received }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

test('a host without events-v2 is refused', async () => {
  const { code, stderr, received } = await meet(
    { op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'host', implementation: { name: 'rutis-host', version: '0.12.1' }, capabilities: ['signals', 'sync-wait'] },
    ['--id', 'rt', '--peer', 'host'],
  )
  expect(code).toBe(1)
  expect(stderr.split('\n')).toContain(REFUSAL)
  // Its own hello, which declares events-v2, and nothing after it.
  expect(received.map(frame => frame.op)).toEqual(['hello'])
  expect(received[0].capabilities).toContain('events-v2')
})

test('a host that declares events-v2 is not refused', async () => {
  // The control: the same host with the capability gets a session, which
  // ends normally when the host goes (exit status 0, no refusal).
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-old-host-'))
  try {
    writeFileSync(join(dir, 'package.json'), '{}')
    const path = join(dir, 'host.sock')
    const server = createServer(connection => {
      connection.on('error', () => {})
      connection.write(JSON.stringify({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'main', capabilities: ['signals', 'sync-wait', 'events-v2'] }) + '\n')
      // Once the runtime greeted back, the host ends the session.
      connection.once('data', () => connection.end())
    })
    await new Promise<void>(resolve => server.listen(path, resolve))
    const child = Bun.spawn([process.execPath, '--no-install', '--no-env-file', MAIN, path, '--id', 'rt', dir], {
      stdin: 'ignore', stdout: 'ignore', stderr: 'pipe',
    })
    const code = await within('the runtime to stop', child.exited)
    const stderr = await new Response(child.stderr).text()
    server.close()
    expect(stderr).not.toContain('refused')
    expect(code).toBe(0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
