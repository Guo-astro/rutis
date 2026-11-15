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
// What this does not cover: the process's own pump (`receiveMessageOnPort`
// plus `Atomics.wait`, src/client.mjs:30-35) and the runner around it.
// runner.mjs is a script, not a factory, so the runner-level frame pump has no
// test yet — it is the gap #630 names for Node and Bun ("runner.mjs 要先改成
// 工厂").
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Session, ENDPOINT_PROTOCOL } from '../src/session.mjs'

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
function wire(dispatch, { steps = [] } = {}) {
  const incoming = [], sent = [], asks = []
  let fault
  const peer = new Session({
    endpoint: { local: 'node' },
    dispatch,
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
