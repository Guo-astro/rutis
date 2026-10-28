// guarantee: sdk/error-texts — what a developer reads when a row cannot load
// or a call is refused uses only what they wrote: their files, services
// and methods (#677, #680; design-diagnostics §4.5, §8). Paths are shown
// relative to where the host runs.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { fakeHost } from './fixtures/fake-host.mjs'
import { within } from './support.mjs'

const runner = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
const sdk = new URL('../../rutis/src/index.mjs', import.meta.url).href

test('a row that cannot load, or is refused a call, is told in the developer\'s words', async () => {
  const dir = mkdtempSync(join(fileURLToPath(new URL('.', import.meta.url)), 'texts-'))
  const file = name => join(dir, name)
  const shown = name => relative(process.cwd(), file(name))
  writeFileSync(file('anchor.mjs'), 'export function apply() {}\n')
  // A default export that is not a definePlugin result: no apply at all,
  // and one written as a leaf plugin, which Cordis takes as its own.
  writeFileSync(file('plain.mjs'), "export default { inject: ['llm'] }\n")
  writeFileSync(file('leafish.mjs'), "export default { inject: ['llm'], apply(ctx) { ctx.use('llm') } }\n")
  // A submodule that throws as it is imported.
  writeFileSync(file('entry.mjs'), "import { x } from './util.mjs'\nexport function apply() { return x }\n")
  writeFileSync(file('util.mjs'), "export const x = 1\nthrow new RangeError('bad util')\n")
  writeFileSync(file('kept.mjs'), 'export function apply() {}\n')
  // A module that throws a string as it is imported.
  writeFileSync(file('str.mjs'), "throw 'need 5 words'\n")
  // A host service's method it does not have; a service not declared.
  writeFileSync(file('caller.mjs'), `import { definePlugin } from '${sdk}'
export default definePlugin({ inject: ['llm'], apply(ctx) { ctx.use('llm').other() } })
`)
  writeFileSync(file('sneaky.mjs'), `import { definePlugin } from '${sdk}'
export default definePlugin({ apply(ctx) { ctx.use('tools') } })
`)
  // Two providers of one service: a row and another row, or the host.
  writeFileSync(file('forecast.mjs'), `import { definePlugin } from '${sdk}'
export default definePlugin({ apply(ctx) { ctx.provide('forecast', { today() { return 1 } }) } })
`)
  writeFileSync(file('notes.mjs'), `import { definePlugin } from '${sdk}'
export default definePlugin({ apply(ctx) { ctx.provide('llm', { ask() { return 1 } }) } })
`)
  const host = await fakeHost([process.execPath, runner, file('anchor.mjs')])
  try {
    await within('the handshake', host.ready)
    await host.elsewhere({})
    await host.call('mount', { plugins: [], announce: true, scope: [] })
    await host.call('hosts.provide', ['llm', { ask: 'sync' }])
    const load = (key, name) => within(`row ${key} answers`, host.call('rows.load', [key, file(name), {}, [], [], {}, { scope: [] }])).then(() => null, error => error)
    const pick = error => ({ name: error?.name, message: error?.message, code: error?.code, data: error?.data })

    const notDefined = name => ({ name: 'LoadError', message: `the default export of ${shown(name)} is not a plugin made with definePlugin`, code: undefined, data: undefined })
    assert.deepEqual(pick(await load('plain', 'plain.mjs')), notDefined('plain.mjs'))
    assert.deepEqual(pick(await load('leafish', 'leafish.mjs')), notDefined('leafish.mjs'))

    assert.deepEqual(pick(await load('entry', 'entry.mjs')), { name: 'LoadError', message: `cannot load ${shown('util.mjs')}: RangeError: bad util (${shown('util.mjs')}:2:7)`, code: undefined, data: undefined })

    assert.deepEqual(pick(await load('str', 'str.mjs')), { name: 'LoadError', message: `cannot load ${shown('str.mjs')}: "need 5 words"`, code: undefined, data: undefined })
    assert.equal(await load('kept', 'kept.mjs'), null)
    await host.call('rows.unload', ['kept'])
    rmSync(file('kept.mjs'))
    assert.deepEqual(pick(await load('kept', 'kept.mjs')), { name: 'LoadError', message: `the entry file ${shown('kept.mjs')} is gone`, code: undefined, data: undefined })

    assert.deepEqual(pick(await load('caller', 'caller.mjs')), { name: 'BindingError', message: 'llm has no method other (methods: ask)', code: 'no-method', data: { service: 'llm', method: 'other', methods: ['ask'] } })
    assert.deepEqual(pick(await load('sneaky', 'sneaky.mjs')), { name: 'BindingError', message: "service tools is not available to this plugin, which does not declare it in inject: add 'tools' to its inject", code: 'not-declared', data: { service: 'tools' } })
    assert.equal(await load('a', 'forecast.mjs'), null)
    assert.deepEqual(pick(await load('b', 'forecast.mjs')), { name: 'Error', message: 'forecast is already provided by a', code: undefined, data: undefined })
    assert.deepEqual(pick(await load('n', 'notes.mjs')), { name: 'Error', message: 'llm is already provided by the host', code: undefined, data: undefined })
    // Nothing of the platform in any of it.
    assert.doesNotMatch(host.stderr, /Cordis|without inject|rutis host/)
  } finally {
    await host.end()
    rmSync(dir, { recursive: true, force: true })
  }
})

// guarantee: sdk/error-codes
// The host going away during a call: the call fails with what happened, in
// plain words, and the code for programs (§4.3).
test('a call in progress when the host disconnects fails with peer-gone', async () => {
  const { createServer } = await import('node:net')
  const { tmpdir } = await import('node:os')
  const { Process } = await import('../src/client.mjs')
  const { ENDPOINT_PROTOCOL: version } = await import('../src/session.mjs')
  const path = join(mkdtempSync(join(tmpdir(), 'rutis-texts-')), 'far.sock')
  const server = createServer(socket => {
    socket.write(JSON.stringify({ op: 'hello', version, endpoint: 'host', capabilities: [] }) + '\n')
    let seen = ''
    socket.on('data', chunk => { seen += chunk; if (seen.includes('"invoke"')) socket.destroy() })
  })
  await new Promise(resolve => server.listen(path, resolve))
  const peer = await within('the handshake', Process.connect(path, () => 'ok', undefined, { local: 'app' }))
  const failed = await within('the call to fail', peer.callAsync('host:llm', 'ask', [])).catch(error => error)
  server.close()
  await peer.closed()
  assert.deepEqual([failed?.name, failed?.message, failed?.code], ['BindingError', 'the host disconnected', 'peer-gone'])
})
