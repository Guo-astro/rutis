// A local channel speaks the endpoint format, as `--id`: started as a host
// starts it (`<channel> --id <name> <project>`), the runtime greets naming
// that id and declaring `runtime`, takes the host greeting as `main`, and
// the session runs until the host ends it; without `--id` it does not
// start. A real runtime process (src/main.ts) on a Unix socket.
import { expect, test } from 'bun:test'
import { createServer } from 'node:net'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ENDPOINT_PROTOCOL } from '../src/session.ts'
import { within } from './support.ts'

const MAIN = new URL('../src/main.ts', import.meta.url).pathname

test('a local channel greets as its --id', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-local-endpoint-'))
  try {
    writeFileSync(join(dir, 'package.json'), '{}')
    const path = join(dir, 'host.sock')
    let greeted!: (frame: any) => void
    const hello = new Promise<any>(resolve => { greeted = resolve })
    const server = createServer(connection => {
      connection.on('error', () => {})
      let buffer = ''
      connection.on('data', chunk => {
        buffer += chunk
        const end = buffer.indexOf('\n')
        if (end < 0) return
        greeted(JSON.parse(buffer.slice(0, end)))
        // The runtime greeted: the host ends the session.
        connection.end()
      })
      connection.write(JSON.stringify({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'main', implementation: { name: 'rutis-host', version: '0.19.0' }, capabilities: [] }) + '\n')
    })
    await new Promise<void>(resolve => server.listen(path, resolve))
    const child = Bun.spawn([process.execPath, '--no-install', '--no-env-file', MAIN, path, '--id', 'weather', dir], {
      stdin: 'ignore', stdout: 'ignore', stderr: 'pipe',
    })
    const frame = await within('the runtime\'s hello', hello)
    expect(frame.version).toBe(ENDPOINT_PROTOCOL)
    expect(frame.endpoint).toBe('weather')
    expect(frame.capabilities).toContain('runtime')
    const code = await within('the runtime to stop', child.exited)
    const stderr = await new Response(child.stderr).text()
    server.close()
    expect(stderr).not.toContain('refused')
    expect(code).toBe(0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('a local channel without --id is refused', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-local-endpoint-'))
  try {
    const path = join(dir, 'host.sock')
    const child = Bun.spawn([process.execPath, '--no-install', '--no-env-file', MAIN, path, dir], {
      stdin: 'ignore', stdout: 'ignore', stderr: 'pipe',
    })
    const code = await within('the runtime to stop', child.exited)
    const stderr = await new Response(child.stderr).text()
    expect(stderr).toBe(`runtime bun: a local channel needs --id <endpoint>: ${path}; a host older than rutis 0.19 started this runtime: upgrade the host\n`)
    expect(code).toBe(2)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
