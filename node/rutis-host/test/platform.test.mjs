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

// #267: Ctrl-C reaches rutis-host from the terminal; the launcher forwarding
// it too made one Ctrl-C two, and the second skipped the cleanups. A SIGINT
// to the launcher alone stays there; a SIGTERM (a service manager's, to the
// main process) is forwarded, and the launcher ends as rutis-host does.
test('the launcher forwards SIGTERM to rutis-host, not Ctrl-C', { skip: process.platform === 'win32' && 'Unix signals' }, async () => {
  const { cpSync, mkdirSync } = await import('node:fs')
  const { spawn } = await import('node:child_process')
  const dir = mkdtempSync(join(tmpdir(), 'rutis-host-'))
  const launcher = join(dir, 'node_modules/@arcships/rutis-host/bin/rutis-host.mjs')
  mkdirSync(join(dir, 'node_modules/@arcships/rutis-host/bin'), { recursive: true })
  cpSync(new URL('../bin/rutis-host.mjs', import.meta.url), launcher)
  // A stand-in for the binary: says it is ready, records the signals it
  // gets, and on SIGTERM prints them and exits 0.
  const bin = join(dir, `node_modules/@arcships/rutis-host-${process.platform}-${process.arch}/bin`)
  mkdirSync(bin, { recursive: true })
  writeFileSync(join(bin, 'rutis-host'), `#!${process.execPath}
const got = []
process.on('SIGINT', () => got.push('SIGINT'))
process.on('SIGTERM', () => process.stdout.write('got ' + got.concat('SIGTERM').join(' ') + '\\n', () => process.exit(0)))
console.log('ready')
setInterval(() => {}, 1000)
`, { mode: 0o755 })
  const host = spawn(process.execPath, [launcher], { stdio: ['ignore', 'pipe', 'inherit'] })
  let out = ''
  const ended = new Promise(resolve => host.on('close', (...end) => resolve(end)))
  await new Promise(resolve => host.stdout.on('data', chunk => {
    out += chunk
    if (out.includes('ready')) resolve()
  }))
  host.kill('SIGINT')
  host.kill('SIGTERM')
  const [code, signal] = await ended
  assert.match(out, /got SIGTERM\n/, out)
  assert.deepEqual([code, signal], [0, null])
})
