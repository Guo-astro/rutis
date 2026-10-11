// The session conformance target, served by the Bun session: connects to
// `channel` as endpoint `bun`, expecting `main`, and serves `conformance`
// until the session ends.
import { Client } from '../../src/client.ts'

const [channel] = process.argv.slice(2)
let held: ((value: unknown) => unknown) | undefined, started = false, aborted = false
let session: Client
const conformance: Record<string, (...args: any[]) => unknown> = {
  echo: value => value,
  apply: (fn, value) => fn(value),
  later: value => new Promise(resolve => setTimeout(() => resolve(value), 5)),
  fail: (name, message) => { const error = new Error(message); error.name = name; throw error },
  hold: fn => { held = fn },
  fire: value => { if (!held) throw new Error('nothing held'); return held(value) },
  drop: () => { if (held) session.release(held); held = undefined },
  abortable: (signal: AbortSignal) => { started = true; return new Promise<void>(resolve => signal.addEventListener('abort', () => { aborted = true; resolve() })) },
  started: () => started,
  aborted: () => aborted,
  reenter: fn => fn(),
}
session = await Client.connect(channel, {
  dispatch: (target, method, args: any) => {
    if (target !== 'conformance') throw new Error(`no target ${target}`)
    const operation = conformance[method]
    if (!operation) throw new Error(`no method ${method}`)
    return operation(...args)
  },
  endpoint: { local: 'bun', expected: 'main' },
})
await session.closed()
process.exit(0)
