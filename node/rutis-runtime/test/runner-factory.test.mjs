// The runtime as a factory (#630): `run` speaks on a session the test gives
// it, in this process, so the runtime's own frame pump and two runtimes in
// one process are driven without a process per test.
//
// The wire is the one frame-pump.test.mjs scripts for a `Session`: the test
// decides which frame the runtime's synchronous wait reads, and nothing below
// waits for a timer to decide an order.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
import { run } from '../src/runner.mjs'
import { ENDPOINT_PROTOCOL, Session } from '../src/session.mjs'
import { within } from './support.mjs'

const FIXTURE = fileURLToPath(new URL('./fixtures/factory-cordis.mjs', import.meta.url))
const greeting = { op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'rust', capabilities: ['objects'] }

const data = value => ({ type: 'data', value })
const list = value => ({ type: 'list', value })
const invoke = (id, target, method, args) => ({ op: 'invoke', id, path: [], target, method, args })

// One runtime in this process, its transport scripted here. `steps` is what
// the pump does on each ask, in order (as in frame-pump.test.mjs): a function
// reads the far end's next frame. `stop()` resolves the runtime's session;
// `run` then disposes its rows and answers with the exit status.
function inProcess(name, { steps = [], env = {} } = {}) {
  const sent = [], asks = [], waiting = []
  let session
  let ready
  let finish
  let failure
  let status
  // The host's call ids, increasing as the session requires (`rust:<n>`); an
  // await of a future is a call of the host too.
  let calls = 0
  const allocated = () => `rust:${++calls}`

  const opened = new Promise(resolve => { ready = resolve })
  const closed = new Promise(resolve => { finish = resolve })

  const deliver = frame => session.receive(frame)
  // The host's answer to a call of the runtime that no test scripts: control
  // calls (events.on, service, rows.ended) are answered with null, a host
  // service with what the test's host provides. A synchronous call waits in
  // this thread, so its answer is a pump step, not a turn of the loop.
  const answer = frame => {
    if (frame.op !== 'invoke' || !String(frame.id).startsWith('node:')) return
    const value = frame.target === 'host:clock' && frame.method === 'now' ? 'tick' : null
    if (frame.sync === true) steps.push(() => deliver({ op: 'return', id: frame.id, value: data(value) }))
    else setImmediate(() => deliver({ op: 'return', id: frame.id, value: data(value) }))
  }
  const send = line => {
    const frame = JSON.parse(line)
    sent.push(frame)
    answer(frame)
    for (const check of waiting) check()
  }

  const connect = async (_channel, dispatch, _settled, endpoint) => {
    session = new Session({
      endpoint,
      dispatch: (target, method, args) => dispatch(target, method, args),
      send,
      pump: done => {
        // `done()` is false while the wait is unanswered: a session asking
        // after its answer would be asking for nothing.
        asks.push(done())
        const step = steps.shift()
        if (!step) throw new Error(`runtime ${name}: the pump was asked for a frame the script does not have`)
        step()
      },
      abort: error => { failure = error },
    })
    session.receive(greeting)
    ready()
    return {
      call: (target, method, args, described) => session.invoke(target, method, args, described),
      callAsync: (target, method, args, described) => session.invokeAsync(target, method, args, described),
      callAnswered: (target, method, args, answered, described) => session.invokeAnswered(target, method, args, answered, described),
      callFunctionAsync: (fn, args) => Promise.resolve().then(() => fn(...args)),
      callFunctionSync: (fn, args) => fn(...args),
      withinSync: run => session.withinSync(run),
      closed: () => closed,
      end: () => finish(),
      drain: () => session.drain(),
      fault: undefined,
    }
  }

  const running = run({ argv: ['factory-test', '--id', 'node', '--peer', 'rust', FIXTURE], env, connect, exit: code => { status = code }, handlers: false, watchdog: false })

  // A frame the runtime sent, taken out of `sent` (frames not matched stay).
  const next = (predicate, what) => within(`runtime ${name}: ${what}`, new Promise(resolve => {
    const look = () => {
      const index = sent.findIndex(predicate)
      return index < 0 ? undefined : sent.splice(index, 1)[0]
    }
    const found = look()
    if (found) return resolve(found)
    waiting.push(() => {
      const frame = look()
      if (frame) resolve(frame)
    })
  }))

  // A promise the runtime answered with is a future reference, as it is to
  // the Rust host: `await` it, when it settles, as `settle` there does.
  const settle = async frame => {
    while (frame.op === 'return' && frame.value?.type === 'reference' && frame.value.value?.kind === 'future') {
      const reference = frame.value.value
      const id = allocated()
      const reply = next(candidate => candidate.id === id, 'the awaited result')
      deliver({ op: 'await', id, path: [...(reference.origin ?? [])], reference: reference.id })
      frame = await reply
    }
    if (frame.op === 'throw') throw new Error(`runtime ${name}: the call failed: ${JSON.stringify(frame.error)}`)
    return frame
  }

  // A control call of the host: `args` as the Rust host encodes it (a single
  // value for mount, a list for the others).
  const ask = async (method, args) => {
    const id = allocated()
    const reply = next(frame => frame.id === id, `the reply to ${method}`)
    deliver(invoke(id, '', method, args))
    return settle(await reply)
  }

  return {
    asks,
    sent,
    started: async () => { await opened; await new Promise(resolve => setImmediate(resolve)) },
    mount: async () => {
      await ask('mount', data({ plugins: [], announce: true, scope: [] }))
      await ask('hosts.provide', data(['clock', { now: 'sync' }]))
    },
    // Load the fixture as row `r`; answers the handle its `factory` service
    // was announced under.
    load: async label => {
      const announced = next(frame => frame.method === 'service' && frame.target === '' && frame.args?.value?.[0]?.value === 'factory', 'the factory service')
      await ask('rows.load', data(['r', FIXTURE, { label }, [], [], {}, { scope: [], id: `node:${name}` }]))
      return (await announced).args.value[1].value
    },
    unload: async () => { await ask('rows.unload', data(['r'])) },
    inspect: async () => (await ask('inspect', data(null))).value.value,
    // Call the row's service: a list of arguments, as `Process::invoke` sends.
    call: (handle, method, args = []) => {
      const id = allocated()
      const reply = next(frame => frame.id === id, `the reply to ${method}`)
      deliver(invoke(id, handle, method, list(args)))
      return reply
    },
    stop: async () => {
      finish()
      await within(`runtime ${name} to end`, running)
      assert.equal(failure, undefined)
      return status
    },
  }
}

// A synchronous call of a row to the host holds this thread; the reply is
// read by the runtime's own pump, and the call answers with it in place.
// This is the runner-level frame pump #630 names ("runner.mjs 要先改成工厂"):
// Session's pump alone is frame-pump.test.mjs's, the runner's dispatch and
// bus are not.
test("a row's synchronous call to the host is answered by the runtime's own pump", async () => {
  const a = inProcess('a', { env: { RUTIS_TEST_INSPECT: '1' } })
  await a.started()
  await a.mount()
  const handle = await a.load('pump')
  assert.equal(handle, 'factory')
  const reply = await a.call(handle, 'now')
  assert.deepEqual(reply.value, data('tick'))
  // The wait was unanswered at its one ask: the reply came from the step the
  // test queued, as a paused transport's does, not from a frame in hand.
  assert.deepEqual(a.asks, [false])
  const out = a.sent.find(frame => frame.target === 'host:clock' && frame.method === 'now')
  assert.equal(out?.sync, true, 'the call to the host went out as a synchronous one')
  assert.deepEqual((await a.inspect()).rows, ['r'])
  assert.equal(await a.stop(), 0)
})

// Two runtimes in one process are two sessions: the same row key and service
// name in each are that runtime's own, an unload in one leaves the other's
// row serving, and a session ending takes only its own with it.
test('two runtimes in one process keep their rows and services apart', async () => {
  const env = { RUTIS_TEST_INSPECT: '1' }
  const a = inProcess('a', { env }), b = inProcess('b', { env })
  await Promise.all([a.started(), b.started()])
  await Promise.all([a.mount(), b.mount()])
  const [inA, inB] = await Promise.all([a.load('a'), b.load('b')])
  assert.equal(inA, 'factory')
  assert.equal(inB, 'factory', 'each runtime announced its own service under the same name')
  assert.deepEqual((await a.call(inA, 'label')).value, data('a'))
  assert.deepEqual((await b.call(inB, 'label')).value, data('b'))
  await a.unload()
  assert.deepEqual((await a.inspect()).rows, [], 'the row is gone from the runtime that unloaded it')
  assert.deepEqual((await b.inspect()).rows, ['r'], 'the other runtime still runs its row')
  assert.deepEqual((await b.call(inB, 'label')).value, data('b'))
  assert.equal(await a.stop(), 0)
  // A's session ended, its runtime disposed; B is still serving.
  assert.deepEqual((await b.call(inB, 'label')).value, data('b'))
  assert.equal(await b.stop(), 0)
})
