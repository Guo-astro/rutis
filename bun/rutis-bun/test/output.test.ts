// guarantee: host/output-prefix — the Bun runtime's side (#678): with
// RUTIS_PREFIX_OUTPUT=1 each line a row's code writes (console, the
// streams) starts with the row's name, also after an await and when
// written in pieces; without it, the output is as written.
// guarantee: host/blocking-hint — a row's method holding the runtime's
// thread is said once on stderr, and not under a debugger (RUTIS_DEBUG=1).
import { expect, test } from 'bun:test'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { within } from './support.ts'
import { blockingText } from '../src/output.mjs'

const DRIVER = new URL('./fixtures/output-driver.ts', import.meta.url).pathname
const TALK = `
export function apply(ctx, config) {
  console.log('applied ' + config.label)
  ctx.provide('talk-' + config.label, {
    say(text) { console.log(text); return null },
    shout(text) { console.error(text); return null },
    async later(text) { await new Promise(resolve => setTimeout(resolve, 10)); console.log('later ' + text); return null },
    pieces() { process.stdout.write('one\\ntwo\\nthr'); process.stdout.write('ee\\n'); return null },
    async wait(ms) { await new Promise(resolve => setTimeout(resolve, ms)); return null },
    block(ms) { const end = Date.now() + ms; while (Date.now() < end) {} return null },
  })
}
`

async function run(steps: string[], env: Record<string, string>) {
  const dir = mkdtempSync(join(tmpdir(), 'rutis-bun-output-'))
  try {
    writeFileSync(join(dir, 'package.json'), '{}')
    writeFileSync(join(dir, 'talk.ts'), TALK)
    const child = Bun.spawn([process.execPath, DRIVER, dir, ...steps], {
      stdin: 'ignore', stdout: 'pipe', stderr: 'pipe', env: { ...process.env, RUTIS_PREFIX_OUTPUT: '', RUTIS_DEBUG: '', ...env },
    })
    const code = await within('the runtime to end', child.exited)
    const [stdout, stderr] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text()])
    expect(code, stderr).toBe(0)
    return { stdout: stdout.split('\n'), stderr: stderr.split('\n') }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

test("each line a row's code writes starts with the row's name", async () => {
  const { stdout, stderr } = await run(['say', 'shout', 'later', 'pieces'], { RUTIS_PREFIX_OUTPUT: '1' })
  for (const line of ['[weather] applied a', '[chat] applied b', '[weather] say', '[weather] later later', '[chat] one', '[chat] two', '[chat] three']) {
    expect(stdout).toContain(line)
  }
  expect(stderr).toContain('[chat] shout')
})

test('without RUTIS_PREFIX_OUTPUT the output is as written', async () => {
  const { stdout } = await run(['say'], {})
  expect(stdout).toContain('say')
  expect(stdout.join('\n')).not.toContain('[weather]')
})

test('a method holding the runtime is said once; a short one is not', async () => {
  const { stderr } = await run(['block:20', 'block:700'], { RUTIS_TEST_BLOCKING_MS: '200' })
  expect(stderr.filter(line => line.includes('held the runtime'))).toEqual([blockingText('bun', 'weather', 'talk-a.block', 0.2)])
})

test('a method awaiting for long does not hold the runtime, and is not said', async () => {
  const { stderr } = await run(['wait:400'], { RUTIS_TEST_BLOCKING_MS: '100' })
  expect(stderr.join('\n')).not.toContain('held the runtime')
})

test('under a debugger nothing is said of a method holding the runtime', async () => {
  const { stderr } = await run(['block:400'], { RUTIS_TEST_BLOCKING_MS: '100', RUTIS_DEBUG: '1' })
  expect(stderr.join('\n')).not.toContain('held the runtime')
})
