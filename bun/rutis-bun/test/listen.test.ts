// `listen:` on port 0 (#455): the runtime binds a port the system picks
// and reports it on stderr as `rutis: listening on <address>`, the line
// every runtime prints, so a script or test never picks a port for it.
import { expect, test } from 'bun:test'
import { connect } from 'node:net'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { within } from './support.ts'

const MAIN = new URL('../src/main.ts', import.meta.url).pathname

test('listening on port 0 reports the port it bound', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-listen-'))
  writeFileSync(join(dir, 'package.json'), '{}')
  const child = Bun.spawn(
    [process.execPath, '--no-install', '--no-env-file', MAIN, 'listen:ws://127.0.0.1:0/rutis', '--id', 'gpu', '--peer', 'main', dir],
    { stdin: 'ignore', stdout: 'ignore', stderr: 'pipe', env: { ...process.env, RUTIS_TOKEN: 'secret' } },
  )
  try {
    const reader = child.stderr.getReader()
    let text = ''
    const line = await within('the listening line', (async () => {
      for (;;) {
        const end = text.indexOf('\n')
        if (end >= 0) return text.slice(0, end)
        const { value, done } = await reader.read()
        if (done) throw new Error(`stderr ended: ${text}`)
        text += new TextDecoder().decode(value)
      }
    })())
    const found = /^rutis: listening on ws:\/\/127\.0\.0\.1:(\d+)\/rutis$/.exec(line)
    expect(found, line).not.toBeNull()
    const port = Number(found![1])
    expect(port).toBeGreaterThan(0)
    // Something listens there.
    await within('a connection to the reported port', new Promise<void>((resolve, reject) => {
      const socket = connect(port, '127.0.0.1', () => { socket.destroy(); resolve() })
      socket.on('error', reject)
    }))
  } finally {
    child.kill()
    await child.exited
    rmSync(dir, { recursive: true, force: true })
  }
})
