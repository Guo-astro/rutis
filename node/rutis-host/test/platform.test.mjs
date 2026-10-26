import { test } from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, writeFileSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

test('a platform package carries the binary, its os and cpu, and the host version', () => {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-host-'))
  const binary = join(dir, 'rutis-host')
  writeFileSync(binary, '#!/bin/sh\n')
  const out = join(dir, 'pkg')
  execFileSync(process.execPath, [new URL('../scripts/platform-package.mjs', import.meta.url).pathname, 'linux', 'x64', binary, out])
  const manifest = JSON.parse(readFileSync(join(out, 'package.json'), 'utf8'))
  const host = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'))
  assert.equal(manifest.name, '@arcships/rutis-host-linux-x64')
  assert.equal(manifest.version, host.version)
  assert.deepEqual([manifest.os, manifest.cpu], [['linux'], ['x64']])
  assert.ok(statSync(join(out, 'bin', 'rutis-host')).mode & 0o111)
  assert.equal(host.optionalDependencies[manifest.name], host.version, 'the host depends on it at its version')
})

test('a Windows platform package carries rutis-host.exe', () => {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-host-'))
  const binary = join(dir, 'rutis-host.exe')
  writeFileSync(binary, 'MZ')
  const out = join(dir, 'pkg')
  execFileSync(process.execPath, [new URL('../scripts/platform-package.mjs', import.meta.url).pathname, 'win32', 'x64', binary, out])
  const manifest = JSON.parse(readFileSync(join(out, 'package.json'), 'utf8'))
  const host = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'))
  assert.equal(manifest.name, '@arcships/rutis-host-win32-x64')
  assert.deepEqual([manifest.os, manifest.cpu], [['win32'], ['x64']])
  assert.ok(statSync(join(out, 'bin', 'rutis-host.exe')).isFile())
  assert.equal(host.optionalDependencies[manifest.name], host.version, 'the host depends on it at its version')
})

// The launcher installed as npm installs it, with `standIn` (a script) as
// this platform's binary, started; its output read until the stand-in says
// it is ready.
async function launched(standIn) {
  const { cpSync, mkdirSync } = await import('node:fs')
  const { spawn } = await import('node:child_process')
  const dir = mkdtempSync(join(tmpdir(), 'rutis-host-'))
  const launcher = join(dir, 'node_modules/@arcships/rutis-host/bin/rutis-host.mjs')
  mkdirSync(join(dir, 'node_modules/@arcships/rutis-host/bin'), { recursive: true })
  cpSync(new URL('../bin/rutis-host.mjs', import.meta.url), launcher)
  const bin = join(dir, `node_modules/@arcships/rutis-host-${process.platform}-${process.arch}/bin`)
  mkdirSync(bin, { recursive: true })
  writeFileSync(join(bin, 'rutis-host'), standIn, { mode: 0o755 })
  const host = spawn(process.execPath, [launcher], { stdio: ['ignore', 'pipe', 'pipe'] })
  const seen = { out: '', err: '' }
  const waits = []
  host.stdout.on('data', chunk => {
    seen.out += chunk
    for (const wait of waits) wait()
  })
  host.stderr.on('data', chunk => { seen.err += chunk })
  const said = text => new Promise(resolve => {
    const look = () => { if (seen.out.includes(text)) resolve() }
    waits.push(look)
    look()
  })
  const ended = new Promise(resolve => host.on('close', (...end) => resolve(end)))
  await said('ready')
  return { host, seen, said, ended }
}

// #267: Ctrl-C reaches rutis-host from the terminal; the launcher forwarding
// it too made one Ctrl-C two, and the second skipped the cleanups. A SIGINT
// to the launcher alone stays there. What a service manager sends to the
// main process (the launcher) is forwarded: SIGTERM, and SIGUSR1 (#664),
// for which Node opens no debugger. The launcher ends as rutis-host does.
test('the launcher forwards SIGTERM and SIGUSR1 to rutis-host, not Ctrl-C', { skip: process.platform === 'win32' && 'Unix signals' }, async () => {
  // A stand-in for the binary: says it is ready, records the signals it
  // gets, and on SIGTERM prints them and exits 0.
  const { host, seen, said, ended } = await launched(`#!${process.execPath}
const got = []
process.on('SIGINT', () => got.push('SIGINT'))
process.on('SIGUSR1', () => { got.push('SIGUSR1'); console.log('usr1') })
process.on('SIGTERM', () => process.stdout.write('got ' + got.concat('SIGTERM').join(' ') + '\\n', () => process.exit(0)))
console.log('ready')
setInterval(() => {}, 1000)
`)
  host.kill('SIGINT')
  host.kill('SIGUSR1')
  await said('usr1')
  host.kill('SIGTERM')
  const [code, signal] = await ended
  assert.match(seen.out, /got SIGUSR1 SIGTERM\n/, seen.out)
  assert.deepEqual([code, signal], [0, null])
  assert.doesNotMatch(seen.err, /Debugger listening/, seen.err)
})

// A command that does not read SIGUSR1 ends on it: so does the launcher, as
// a shell reports such an end, instead of opening Node's debugger and
// staying.
test('a rutis-host ended by SIGUSR1 ends the launcher', { skip: process.platform === 'win32' && 'Unix signals' }, async () => {
  const { constants } = await import('node:os')
  const { host, seen, ended } = await launched('#!/bin/sh\necho ready\nexec sleep 1000\n')
  host.kill('SIGUSR1')
  const [code, signal] = await ended
  assert.deepEqual([code, signal], [128 + constants.signals.SIGUSR1, null])
  assert.doesNotMatch(seen.err, /Debugger listening/, seen.err)
})
