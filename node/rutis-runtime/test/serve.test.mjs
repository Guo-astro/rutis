import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { connect } from 'node:net'
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createInterface } from 'node:readline'
import { loopback } from '../src/serve.mjs'
import { open } from '../src/channel/websocket.mjs'
import { ENDPOINT_PROTOCOL } from '../src/session.mjs'

const dial = address => {
  const [host, port] = address.slice('tcp:'.length).split(':')
  const socket = connect({ host, port: Number(port) })
  return new Promise((resolve, reject) => { socket.once('connect', () => resolve(socket)); socket.once('error', reject) })
}

test('a loopback connection that fails before its token is dropped alone', async () => {
  const listener = await loopback('the-token')
  // Reset before authenticating: its error must not end this process.
  const failing = await dial(listener.address)
  failing.write('the-to')
  failing.resetAndDestroy()
  await new Promise(resolve => setTimeout(resolve, 100))
  // A wrong token is turned away; the right one is the session, with what
  // followed its token.
  const wrong = await dial(listener.address)
  wrong.write('not-it\n')
  const right = await dial(listener.address)
  right.write('the-token\nfirst\n')
  const accepted = await listener.accepted
  const line = await new Promise(resolve => createInterface({ input: accepted }).once('line', resolve))
  assert.equal(line, 'first')
  accepted.destroy()
  wrong.destroy()
  right.destroy()
})

test('a session whose process ends before connecting ends at once', async () => {
  // The session's runner fails before it connects: the Cordis its anchor
  // resolves does not load. The listening runtime itself never loads it.
  const project = mkdtempSync(join(tmpdir(), 'rutis-serve-'))
  const cordis = join(project, 'node_modules/@deepseek-ai/cordis')
  mkdirSync(cordis, { recursive: true })
  writeFileSync(join(cordis, 'package.json'), JSON.stringify({ name: '@deepseek-ai/cordis', main: 'index.js' }))
  writeFileSync(join(cordis, 'index.js'), "throw new Error('broken on purpose')\n")
  writeFileSync(join(project, 'package.json'), '{}')
  const runner = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
  const runtime = spawn(process.execPath, [runner, 'listen:ws://127.0.0.1:0/rutis', '--id', 'edge', '--peer', 'main', join(project, 'package.json')], {
    env: { ...process.env, RUTIS_TOKEN: 'main-token', RUTIS_LOCAL_HANDOVER: 'loopback' },
    stdio: ['ignore', 'inherit', 'pipe'],
  })
  try {
    const url = await new Promise((resolve, reject) => {
      createInterface({ input: runtime.stderr }).on('line', line => {
        const found = /^rutis: listening on (.*)$/.exec(line)
        if (found) resolve(found[1])
      })
      runtime.once('exit', code => reject(new Error(`the runtime exited (${code})`)))
    })
    let closed
    const ended = new Promise(resolve => { closed = resolve })
    await open(url, { message() {}, closed: reason => closed(reason) }, { protocol: `rutis.${ENDPOINT_PROTOCOL}`, token: 'main-token' })
    const reason = await Promise.race([ended, new Promise(resolve => setTimeout(() => resolve('still open'), 10000))])
    assert.notEqual(reason, 'still open')
    // The runtime keeps listening.
    assert.equal(runtime.exitCode, null)
  } finally {
    runtime.kill()
    rmSync(project, { recursive: true, force: true })
  }
})
