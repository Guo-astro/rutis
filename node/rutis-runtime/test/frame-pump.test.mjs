// The session's frame pump, driven by the test (#630, formal verification
// design §3.3: "Node/Bun: frame pump, synchronous wait, re-entrancy, event
// delivery and cancellation").
//
// `Session` takes its transport as a seam: `send` for frames out, `pump` for
// frames the synchronous wait of a call reads, `receive` for the frames the
// far end sends. So pause, lateness, cancel and close are decided here, never
// by how fast the machine is: no test below waits on a timer, and a pump asked
// past its script throws instead of spinning the thread.
//
// The bus over the session appears here once, to drive a Cordis filter's
// synchronous call; the same-name hold is the session's `holds` seam
// (src/runner.mjs:1200).
//
// What this does not cover: the process's own pump (`receiveMessageOnPort`
// plus `Atomics.wait`, src/client.mjs:30-35) and the runner around it, which
// test/runner-factory.test.mjs drives in process: runner.mjs is a factory
// (#630), so its dispatch, bus and synchronous wait are that test's.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Session, ENDPOINT_PROTOCOL } from '../src/session.mjs'
import { Bus } from '../src/events.mjs'

const data = value => ({ type: 'data', value })
const invoke = (id, method, path = [], args = data([])) => ({ op: 'invoke', id: `rust:${id}`, path, target: 'test', method, args })
const reply = (id, value) => ({ op: 'return', id, value: data(value) })
// A call this side makes: its arguments go out as a list, one item per
// argument, and a synchronous one says so (#932).
const callOut = (id, method, path = []) => ({ op: 'invoke', id, path, target: 'test', method, args: { type: 'list', value: [] }, sync: true })
const greeting = { op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'rust', capabilities: ['objects'] }

// A wire the test drives. `steps` says what the transport does on each ask of
// the pump, in order: `'pause'` finds nothing to read (a paused transport), a
// function runs (the far end's frame arriving between two asks, say), and a
// leftover step delivers the next frame of `incoming`. Nothing is read unless
// the session asks, which is what makes the order below the test's.
function wire(dispatch, { steps = [], holds } = {}) {
  const incoming = [], sent = [], asks = []
  let fault
  const peer = new Session({
    endpoint: { local: 'node' },
    dispatch,
    holds,
    send: line => sent.push(JSON.parse(line)),
    pump: done => {
      // `done()` is false while the wait is unanswered: a session asking
      // after its answer would be asking for nothing.
      asks.push(done())
      const step = steps.length ? steps.shift() : 'deliver'
      if (step === 'pause') return
      if (typeof step === 'function') return step()
      if (!incoming.length) throw new Error('the pump was asked for a frame the script does not have')
      peer.receive(incoming.shift())
    },
    abort: error => { fault = error },
  })
  peer.receive(greeting)
  return { peer, incoming, sent, asks, fault: () => fault }
}

// A synchronous call holds this thread and asks the transport for frames until
// its answer comes. A paused transport hands out nothing, and asking again is
// what finds the reply: the wait neither spins without reading nor reads past
// its answer.
test('a paused frame pump answers the wait only when the reply is read', async () => {
  const { peer, incoming, sent, asks, fault } = wire(() => 'unused', { steps: ['pause', 'pause'] })
  incoming.push(reply('node:1', 42))
  assert.equal(peer.invoke('test', 'slow', []), 42)
  // Two asks found nothing and the third read the reply; none came after it.
  assert.deepEqual(asks, [false, false, false])
  assert.deepEqual(sent, [callOut('node:1', 'slow')], 'the call went out once, and nothing else was written')
  assert.equal(fault(), undefined)
  // The pause ends with the wait: the next call is answered from the queue.
  incoming.push(reply('node:2', 7))
  assert.equal(peer.invoke('test', 'after', []), 7)
  assert.deepEqual(asks, [false, false, false, false])
  await peer.drain()
  peer.close()
})

// A frame the pump delivers while the thread is held runs inside that wait,
// and its own synchronous call asks the pump again (#932, a filter's
// waterfall in a synchronous chain). A frame read behind it is held to the end
// of the wait and then runs, in arrival order — never dropped, never run early.
test('a frame read inside a wait runs there; one read behind it waits its turn', async () => {
  const log = []
  let inner
  const { peer, incoming, sent, asks, fault } = wire((_, method) => {
    log.push(method)
    if (method === 'nested') { inner = peer.invoke('test', 'inner', []); return 'nested-result' }
    return 'unrelated-result'
  })
  incoming.push(
    invoke(2, 'nested', ['node:1']),
    invoke(3, 'unrelated'),
    reply('node:2', 7),
    reply('node:1', 'outer'),
  )
  assert.equal(peer.invoke('test', 'outer', []), 'outer')
  assert.deepEqual(asks, [false, false, false, false], 'the nested call pumped inside the outer one')
  assert.equal(inner, 7, 'a call made inside a frame the pump delivered is answered from the same pump')
  assert.deepEqual(log, ['nested'], 'the unrelated frame did not start inside the wait')
  // The nested call went out inside the frame that made it, on that frame's
  // chain, and that frame's answer went out before the wait ended.
  assert.deepEqual(sent.slice(0, 3), [
    callOut('node:1', 'outer'),
    callOut('node:2', 'inner', ['node:1', 'rust:2']),
    { op: 'return', id: 'rust:2', value: data('nested-result') },
  ])
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(log, ['nested', 'unrelated'])
  assert.deepEqual(sent.at(-1), { op: 'return', id: 'rust:3', value: data('unrelated-result') })
  assert.equal(fault(), undefined)
  peer.close()
})

// Frames are read in order but the call they carry starts on a later tick, so
// a cancel can arrive before its method runs — the case #630 suspects for
// Python ("a cancel that arrives while its task is still deferred is lost").
// Here it is not lost: the submitted signal is already aborted when the method
// starts, and the call still answers.
test('a cancel read before its deferred frame runs reaches the method', async () => {
  let signal
  const { peer, sent, fault } = wire((_, method, args) => {
    if (method === 'listen') { signal = args[0]; return signal.aborted ? 'already cancelled' : 'listening' }
    return 'later'
  })
  peer.receive(invoke(1, 'listen', [], { type: 'list', value: [{ type: 'signal' }] }))
  // Admitted, not started: the cancel the far end sends next is read first.
  peer.receive({ op: 'cancel', id: 'rust:1' })
  assert.equal(signal, undefined, 'the method had not started when the cancel was read')
  await Promise.resolve()
  assert.equal(signal.aborted, true)
  assert.deepEqual(sent, [{ op: 'return', id: 'rust:1', value: data('already cancelled') }])
  // A cancel for a call that is gone is read and ignored, not a fault
  // (formal-trace §3.4: the protocol's only tolerated dangling id).
  peer.receive({ op: 'cancel', id: 'rust:1' })
  peer.receive({ op: 'cancel', id: 'rust:99' })
  assert.equal(fault(), undefined)
  // The session is still usable afterwards.
  peer.receive(invoke(2, 'later'))
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(sent.at(-1), { op: 'return', id: 'rust:2', value: data('later') })
  assert.equal(fault(), undefined)
  peer.close()
})

// A reply read on the pump's drain answers its wait before the frames behind
// it on that drain are run: those are read on a thread about to be free, and a
// future awaited from one of them settles once it is (#639: a callback of a
// plugin, after an await, awaited while the plugin calls a service of the
// test). It is not a wait cycle.
test('a frame read behind the wait\'s reply is not a wait cycle', async () => {
  let opened
  opened = wire((_, method) => method === 'pending' ? new Promise(() => {}) : 'slow', {
    // The reply and the await behind it in one ask of the pump: one drain.
    steps: [() => {
      opened.peer.receive(opened.incoming.shift())
      opened.peer.receive(opened.incoming.shift())
    }],
  })
  const { peer, incoming, sent, fault } = opened
  // A promise this process made, exported to the far end as a future.
  peer.receive(invoke(1, 'pending'))
  await Promise.resolve()
  const future = sent.find(frame => frame.op === 'return' && frame.id === 'rust:1')?.value?.value?.id
  assert.equal(typeof future, 'number', `a future goes out: ${JSON.stringify(sent)}`)
  incoming.push(
    reply('node:1', 'slow'),
    { op: 'await', id: 'rust:2', path: ['node:1'], reference: future },
  )
  assert.equal(peer.invoke('test', 'slow', []), 'slow')
  // The await is taken, not refused: it waits for the promise, which can
  // settle now that the wait's reply was already read.
  await Promise.resolve()
  assert.deepEqual(sent.filter(frame => frame.op === 'throw'), [], 'no SyncWaitCycle: the wait was answered already')
  assert.equal(fault(), undefined)
  peer.close()
})

// Closing while a frame is being handled, while one is still queued, or while
// a synchronous call holds the thread. In each case the session answers
// nothing, does not report a fault (the far end leaving is not this side's
// mistake) and does not leave the thread waiting for frames that will not come.
test('frames in hand at close: none answers, none runs, and the wait ends', async () => {
  // A frame being handled when the session closes: no reply is written, and
  // the row's work finishes so the runtime can drain.
  {
    const ran = []
    const { peer, sent, fault } = wire((_, method) => { ran.push(method); peer.close(); return 'after close' })
    peer.receive(invoke(1, 'during'))
    await new Promise(resolve => setImmediate(resolve))
    assert.deepEqual(ran, ['during'])
    assert.deepEqual(sent, [], 'answering a closed session would name a call nobody can take')
    assert.equal(fault(), undefined)
    await peer.drain()
  }
  // A frame admitted but still queued: the close drops it, so its method never
  // starts, and nothing is written for it. `drain()` is deliberately not
  // awaited here: with this job dropped, `#active` stays above zero and
  // `drain()` never settles (#630). The only observable is a promise that does
  // not settle, so it is reported with the issue rather than asserted.
  {
    const ran = []
    const { peer, sent } = wire((_, method) => { ran.push(method); return 'queued' })
    peer.receive(invoke(1, 'queued'))
    peer.close()
    await new Promise(resolve => setImmediate(resolve))
    assert.deepEqual(ran, [], 'a frame queued at close never starts')
    assert.deepEqual(sent, [])
  }
  // A synchronous call holding the thread when the close arrives through the
  // pump: the wait ends with the close's error on the first ask, and nothing
  // is asked after it.
  {
    // The step runs during the wait, after `wire` returned, so it closes the
    // session the wait belongs to.
    let opened
    opened = wire(() => 'unused', { steps: [() => opened.peer.close(new Error('the host went away'))] })
    const { peer, sent, asks, fault } = opened
    assert.throws(() => peer.invoke('test', 'wait', []), { message: 'the host went away' })
    assert.deepEqual(asks, [false])
    assert.deepEqual(sent.map(frame => frame.op), ['invoke'], 'the call went out; no answer came back to write')
    assert.equal(fault(), undefined)
    await peer.drain()
  }
})

// §5.5 items 3–4 (EVP-5.5-10, EVP-5.5-11): this runtime holds an emit-mode
// listener call that arrives during a synchronous wait, and a later call of
// the same name waits behind it — also when its path is on the wait's chain,
// where it would otherwise start ahead of the earlier emit. The hold is by
// name (`holds`); a call of another name on the same chain starts there, as
// it always did. Once the wait ends, the held calls run in frame order.
test('a same-name emit call waits behind the one a synchronous wait holds', async () => {
  const log = []
  const holds = (method, args) => method === 'events.call' && args?.[0]?.mode === 'emit' ? args[0].name : undefined
  const { peer, incoming, sent, asks, fault } = wire((_, __, args) => {
    log.push(`${args[0].name}:${args[0].args[0]}`)
    return undefined
  }, { holds })
  const call = (id, name, value, path = []) => ({
    op: 'invoke', id: `rust:${id}`, path, target: '', method: 'events.call',
    args: data([{ listener: '1', mode: 'emit', name, args: [value] }]),
  })
  incoming.push(
    call(2, 'n', 1),
    call(3, 'n', 2, ['node:1']),
    call(4, 'm', 3, ['node:1']),
    reply('node:1', 7),
  )
  assert.equal(peer.invoke('test', 'sync', []), 7)
  // During the wait only the other name ran: it was on the wait's chain, and
  // no earlier call of its name was queued.
  assert.deepEqual(log, ['m:3'])
  assert.deepEqual(sent.filter(frame => frame.op === 'return').map(frame => frame.id), ['rust:4'])
  assert.deepEqual(asks, [false, false, false, false])
  // The wait over, the held calls run in the order their frames arrived: the
  // later `n` did not overtake the earlier one, on the chain or not.
  await new Promise(resolve => setImmediate(resolve))
  assert.deepEqual(log, ['m:3', 'n:1', 'n:2'])
  assert.deepEqual(sent.filter(frame => frame.op === 'return').map(frame => frame.id), ['rust:4', 'rust:2', 'rust:3'])
  assert.equal(fault(), undefined)
  peer.close()
})

// §5.5 item 4 (#630): a filter's answer is a decision taken during the
// dispatch, so the runtime asks it synchronously (`#keeps` → `withinSync`). A
// filter that calls a service of the host waits on that call, and the pump
// reads the frames behind it. This runtime is not re-entrant: a later emit's
// listener call that arrives meanwhile is held, so the filter never sees this
// process's state through a listener running in the middle of its wait.
test("a filter's synchronous call holds a later emit until the wait ends", async () => {
  const log = [], sent = [], asks = [], steps = []
  let session
  let bus
  session = new Session({
    endpoint: { local: 'node' },
    dispatch: (_, method, args) => method === 'events.call' ? bus.call(args[0]) : 'checked',
    send: line => sent.push(JSON.parse(line)),
    pump: done => {
      asks.push(done())
      const step = steps.shift()
      if (!step) throw new Error('the pump was asked for a frame the script does not have')
      step()
    },
    abort: error => log.push(`fault: ${error?.message}`),
  })
  session.receive(greeting)
  bus = new Bus({
    runtime: 'node',
    peer: {
      call: (target, method, args, described) => session.invoke(target, method, args, described),
      callAsync: (target, method, args, described) => session.invokeAsync(target, method, args, described),
      callAnswered: (target, method, args, answered, described) => session.invokeAnswered(target, method, args, answered, described),
      callFunctionSync: (fn, args) => session.callFunctionSync(fn, args),
      callFunctionAsync: (fn, args) => session.callFunctionAsync(fn, args),
      withinSync: run => session.withinSync(run),
    },
    report: line => log.push(`reported: ${line}`),
  })
  bus.on({ name: 'n', callback: value => { log.push(`listener ${value}`) }, row: 'r', plugin: 'p' })
  const keep = () => {
    log.push('filter asked')
    log.push(`filter saw ${session.invoke('test', 'check', [])}`)
    return true
  }
  const out = method => sent.find(frame => frame.op === 'invoke' && frame.method === method)
  steps.push(() => session.receive({
    op: 'call', id: 'rust:1', path: [out('events.dispatch').id],
    reference: out('events.dispatch').args.value[0].value.filter.value.id,
    args: data([[{ listener: '1', name: 'n', plugin: 'p', row: 'r', runtime: 'node', node: null, scope: [], global: false }]]),
    sync: true,
  }))
  steps.push(() => session.receive({
    op: 'invoke', id: 'rust:2', path: [], target: '', method: 'events.call',
    args: data([{ listener: '1', dispatch: 'later', own: false, name: 'n', mode: 'emit', args: [2] }]),
  }))
  steps.push(() => session.receive({ op: 'return', id: out('check').id, value: data('checked') }))
  steps.push(() => session.receive({ op: 'return', id: out('events.dispatch').id, value: data(undefined) }))
  bus.dispatch('emit', 'n', [1], { row: 'r', keep, sync: true })
  // The later emit's frame was read inside the filter's wait and still did
  // not start: only the filter's own answer went out.
  assert.deepEqual(log, ['filter asked', 'filter saw checked'])
  assert.deepEqual(asks, [false, false, false, false])
  assert.deepEqual(sent.filter(frame => frame.op === 'return').map(frame => frame.id), ['rust:1'])
  await new Promise(resolve => setImmediate(resolve))
  // The wait over, the held listener runs and answers.
  assert.deepEqual(log, ['filter asked', 'filter saw checked', 'listener 2'])
  assert.deepEqual(sent.filter(frame => frame.op === 'return').map(frame => frame.id), ['rust:1', 'rust:2'])
  assert.equal(session.fault, undefined)
  session.close()
})
