// The session conformance target, served by the Node session: connects to
// `channel` as endpoint `node`, expecting `main`, and serves `conformance`
// until the session ends.
import { Process } from '../../src/client.mjs'

const [channel] = process.argv.slice(2)
let held, started = false, aborted = false
const kept = new Map()
let session
const conformance = {
  echo: value => value,
  apply: (fn, value) => fn(value),
  later: value => new Promise(resolve => setTimeout(() => resolve(value), 5)),
  fail: (name, message) => { const error = new Error(message); error.name = name; throw error },
  hold: fn => { held = fn },
  fire: value => { if (!held) throw new Error('nothing held'); return held(value) },
  drop: () => { if (held) session.release(held); held = undefined },
  abortable: signal => { started = true; return new Promise(resolve => signal.addEventListener('abort', () => { aborted = true; resolve() })) },
  started: () => started,
  aborted: () => aborted,
  reenter: fn => fn(),
  // The release-count property test (risk P4).
  keep: (slot, fn) => { kept.set(slot, fn) },
  free: slot => { if (kept.has(slot)) session.release(kept.get(slot)); kept.delete(slot) },
  make: () => value => value,
  // The value one level deeper, returned or passed to a callback: past the
  // depth limit, this side's call or reply fails (#279).
  wrap: value => [value],
  applyWrapped: (fn, value) => fn([value]),
  tables: () => session.references(),
}
session = await Process.connect(channel, (target, method, args) => {
  if (target !== 'conformance') throw new Error(`no target ${target}`)
  const operation = conformance[method]
  if (!operation) throw new Error(`no method ${method}`)
  return operation(...args)
}, undefined, { local: 'node', expected: 'main' })
await session.closed()
