// guarantee: host/output-prefix — with RUTIS_PREFIX_OUTPUT=1 each line a
// row's code writes starts with the row's name, also after an await and
// when written in pieces; without it, the output is as written.
// guarantee: host/blocking-hint — a row's method holding the runtime's
// thread is said once on stderr (row, method, what to do), and not under a
// debugger (RUTIS_DEBUG=1) (#678).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
import { fakeHost } from './fixtures/fake-host.mjs'
import { eventually, within } from './support.mjs'
import { blockingText, prefixLines } from '../src/output.mjs'

const runner = fileURLToPath(new URL('../src/runner.mjs', import.meta.url))
const plugin = fileURLToPath(new URL('./fixtures/output-rows.mjs', import.meta.url))
const METHODS = { say: 'async', shout: 'async', later: 'async', pieces: 'async', block: 'async', wait: 'async' }

async function started(env) {
  const host = await fakeHost([process.execPath, runner, plugin], { env })
  await within('the handshake', host.ready)
  await host.call('mount', { plugins: [], announce: true, scope: [] })
  const row = async (key, id) => {
    await host.call('rows.load', [key, plugin, { label: key }, [], [], { [`talk-${key}`]: METHODS }, { scope: [], id }])
    return (method, ...args) => host.session.invokeAsync(host.services.get(`talk-${key}`), method, args)
  }
  return { host, row }
}

test('prefixLines starts each line, also across writes and in bytes', () => {
  const state = { atStart: true }
  assert.equal(prefixLines('a\nb', '[w] ', state), '[w] a\n[w] b')
  assert.equal(prefixLines('c\n', '[w] ', state), 'c\n')
  assert.equal(prefixLines('', '[w] ', state), '')
  assert.equal(prefixLines('$&\n', '[w] ', state), '[w] $&\n')
  assert.equal(prefixLines(Buffer.from('d\ne\n'), '[w] ', state).toString(), '[w] d\n[w] e\n')
  assert.match(blockingText('py', 'weather', 'forecast.tomorrow', 5), /^runtime py: forecast\.tomorrow of row weather has held the runtime for 5 s, and the other rows wait meanwhile: /)
})

test("each line a row's code writes starts with the row's name", async () => {
  const { host, row } = await started({ RUTIS_PREFIX_OUTPUT: '1' })
  try {
    const a = await row('a', 'weather')
    const b = await row('b', 'chat')
    await a('say', 'hello')
    await b('shout', 'oops')
    await a('later', 'tick')
    await b('pieces')
    await eventually('the lines', () => host.stdout.includes('[chat] three\n') && host.stderr.includes('oops'), () => JSON.stringify([host.stdout, host.stderr]))
    const lines = host.stdout.split('\n')
    for (const line of ['[weather] applied a', '[chat] applied b', '[weather] hello', '[weather] later tick', '[chat] one', '[chat] two', '[chat] three']) {
      assert.ok(lines.includes(line), `${line} in ${JSON.stringify(host.stdout)}`)
    }
    assert.ok(host.stderr.split('\n').includes('[chat] oops'), host.stderr)
  } finally {
    await host.end()
  }
})

test('without RUTIS_PREFIX_OUTPUT the output is as written', async () => {
  const { host, row } = await started({ RUTIS_PREFIX_OUTPUT: '' })
  try {
    const a = await row('a', 'weather')
    await a('say', 'hello')
    await eventually('the line', () => host.stdout.includes('hello\n'))
    assert.ok(host.stdout.split('\n').includes('hello'), host.stdout)
    assert.doesNotMatch(host.stdout, /\[weather\]/)
  } finally {
    await host.end()
  }
})

test('a method holding the runtime is said once; a short one is not', async () => {
  const { host, row } = await started({ RUTIS_TEST_BLOCKING_MS: '200', RUTIS_PREFIX_OUTPUT: '' })
  try {
    const a = await row('a', 'weather')
    await a('block', 20)
    await a('block', 700)
    await eventually('the hint', () => host.stderr.includes('held the runtime'))
    const hints = host.stderr.split('\n').filter(line => line.includes('held the runtime'))
    assert.deepEqual(hints, [blockingText('node', 'weather', 'talk-a.block', 0.2)])
  } finally {
    await host.end()
  }
})

test('a method awaiting for long does not hold the runtime, and is not said', async () => {
  const { host, row } = await started({ RUTIS_TEST_BLOCKING_MS: '100', RUTIS_PREFIX_OUTPUT: '' })
  try {
    const a = await row('a', 'weather')
    await a('wait', 400)
    await a('say', 'done')
    await eventually('the line', () => host.stdout.includes('done'))
    assert.doesNotMatch(host.stderr, /held the runtime/)
  } finally {
    await host.end()
  }
})

test('under a debugger nothing is said of a method holding the runtime', async () => {
  const { host, row } = await started({ RUTIS_TEST_BLOCKING_MS: '100', RUTIS_DEBUG: '1' })
  try {
    const a = await row('a', 'weather')
    await a('block', 400)
    await a('say', 'done')
    await eventually('the line', () => host.stdout.includes('done'))
    assert.doesNotMatch(host.stderr, /held the runtime/)
  } finally {
    await host.end()
  }
})
