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
// The Node runtime's copy of this file is node/rutis-runtime/test/frame-pump.test.mjs.
// What neither covers: the process's own pump (the worker in src/client.ts) and
// the runner around it (runner.ts is a script, not a factory), so the
// runner-level frame pump has no test yet — it is the gap #630 names for Node
// and Bun ("runner.mjs 要先改成工厂").
//
// Bun is re-entrant (src/session.ts, "One session"): it has no `holds` seam,
// and needs none — nothing is put aside, so a call read during a wait starts
// there. The bus over the session appears here once, to drive a filter's
// synchronous call (#630).
import { expect, test } from 'bun:test'
import { Session, ENDPOINT_PROTOCOL } from '../src/session.ts'
import { Bus } from '../src/events.mjs'

const data = (value: any) => ({ type: 'data', value })
const invoke = (id: number, method: string, path: string[] = [], args: any = data([])) => ({ op: 'invoke', id: `rust:${id}`, path, target: 'test', method, args })
const reply = (id: string, value: any) => ({ op: 'return', id, value: data(value) })
// A call this side makes: its arguments go out as a list, one item per
// argument, and a synchronous one says so (#932).
const callOut = (id: string, method: string, path: string[] = []) => ({ op: 'invoke', id, path, target: 'test', method, args: { type: 'list', value: [] }, sync: true })
const greeting = { op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'rust', capabilities: ['objects'] }

// A wire the test drives. `steps` says what the transport does on each ask of
// the pump, in order: `'pause'` finds nothing to read (a paused transport), a
// function runs (the far end's frame arriving between two asks, say), and a
// leftover step delivers the next frame of `incoming`. Nothing is read unless
// the session asks, which is what makes the order below the test's.
function wire(dispatch: (target: string, method: string, args: any[]) => unknown, { steps = [] as any[] } = {}) {
  const incoming: any[] = [], sent: any[] = [], asks: boolean[] = []
  let fault: any
  const peer: Session = new Session({
    endpoint: { local: 'bun' },
    dispatch,
    send: (line: string) => sent.push(JSON.parse(line)),
    pump: (done: () => boolean) => {
      // `done()` is false while the wait is unanswered: a session asking
      // after its answer would be asking for nothing.
      asks.push(done())
      const step = steps.length ? steps.shift() : 'deliver'
      if (step === 'pause') return
      if (typeof step === 'function') return step()
      if (!incoming.length) throw new Error('the pump was asked for a frame the script does not have')
      peer.receive(incoming.shift())
    },
    abort: (error: Error) => { fault = error },
  })
  peer.receive(greeting)
  return { peer, incoming, sent, asks, fault: () => fault }
}

// guarantee: boundary/5.6
// A synchronous call holds this thread and asks the transport for frames until
// its answer comes. A paused transport hands out nothing, and asking again is
// what finds the reply: the wait neither spins without reading nor reads past
// its answer.
test('a paused frame pump answers the wait only when the reply is read', async () => {
  const { peer, incoming, sent, asks, fault } = wire(() => 'unused', { steps: ['pause', 'pause'] })
  incoming.push(reply('bun:1', 42))
  expect(peer.invoke('test', 'slow', [])).toBe(42)
  // Two asks found nothing and the third read the reply; none came after it.
  expect(asks).toEqual([false, false, false])
  expect(sent).toEqual([callOut('bun:1', 'slow')])
  expect(fault()).toBeUndefined()
  // The pause ends with the wait: the next call is answered from the queue.
  incoming.push(reply('bun:2', 7))
  expect(peer.invoke('test', 'after', [])).toBe(7)
  expect(asks).toEqual([false, false, false, false])
  await peer.drain()
  peer.close()
})

// A frame the pump delivers while the thread is held runs inside that wait,
// and its own synchronous call asks the pump again (#932, a filter's
// waterfall in a synchronous chain). Bun is reentrant (src/session.ts, "One
// session"): a frame read behind it runs there too, in arrival order — never
// dropped, never run early. Node holds an unrelated frame back instead
// (node/rutis-runtime/test/frame-pump.test.mjs), which is Node's documented
// difference.
test('frames read inside a wait run there, in arrival order (re-entrant)', async () => {
  const log: string[] = []
  let inner: any
  const { peer, incoming, sent, asks, fault } = wire((_, method) => {
    log.push(method)
    if (method === 'nested') { inner = peer.invoke('test', 'inner', []); return 'nested-result' }
    return 'unrelated-result'
  })
  incoming.push(
    invoke(2, 'nested', ['bun:1']),
    invoke(3, 'unrelated'),
    reply('bun:2', 7),
    reply('bun:1', 'outer'),
  )
  expect(peer.invoke('test', 'outer', [])).toBe('outer')
  expect(asks).toEqual([false, false, false, false])
  expect(inner).toBe(7)
  expect(log).toEqual(['nested', 'unrelated'])
  // The nested call went out inside the frame that made it, on that frame's
  // chain. The unrelated frame read behind it ran on the same wait and was
  // answered first (its dispatch returned before the nested call's reply was
  // read); the nested call's answer went out when its reply came.
  expect(sent).toEqual([
    callOut('bun:1', 'outer'),
    callOut('bun:2', 'inner', ['bun:1', 'rust:2']),
    { op: 'return', id: 'rust:3', value: data('unrelated-result') },
    { op: 'return', id: 'rust:2', value: data('nested-result') },
  ])
  await new Promise(resolve => setImmediate(resolve))
  expect(log).toEqual(['nested', 'unrelated'])
  expect(fault()).toBeUndefined()
  peer.close()
})

// guarantee: bridge/cancel-reaches-callee
// Frames are read in order but the call they carry starts on a later tick, so
// a cancel can arrive before its method runs — the case #630 suspects for
// Python ("a cancel that arrives while its task is still deferred is lost").
// Here it is not lost: the submitted signal is already aborted when the method
// starts, and the call still answers.
test('a cancel read before its deferred frame runs reaches the method', async () => {
  let signal: any
  const { peer, sent, fault } = wire((_, method, args: any[]) => {
    if (method === 'listen') { signal = args[0]; return signal.aborted ? 'already cancelled' : 'listening' }
    return 'later'
  })
  peer.receive(invoke(1, 'listen', [], { type: 'list', value: [{ type: 'signal' }] }))
  // Admitted, not started: the cancel the far end sends next is read first.
  peer.receive({ op: 'cancel', id: 'rust:1' })
  expect(signal).toBeUndefined()
  await Promise.resolve()
  expect(signal.aborted).toBe(true)
  expect(sent).toEqual([{ op: 'return', id: 'rust:1', value: data('already cancelled') }])
  // A cancel for a call that is gone is read and ignored, not a fault
  // (formal-trace §3.4: the protocol's only tolerated dangling id).
  peer.receive({ op: 'cancel', id: 'rust:1' })
  peer.receive({ op: 'cancel', id: 'rust:99' })
  expect(fault()).toBeUndefined()
  // The session is still usable afterwards.
  peer.receive(invoke(2, 'later'))
  await new Promise(resolve => setImmediate(resolve))
  expect(sent.at(-1)).toEqual({ op: 'return', id: 'rust:2', value: data('later') })
  expect(fault()).toBeUndefined()
  peer.close()
})

// A reply read on the pump's drain answers its wait before the frames behind
// it on that drain are run: those are read on a thread about to be free, and a
// future awaited from one of them settles once it is (#639). It is not a wait
// cycle.
test('a frame read behind the wait\'s reply is not a wait cycle', async () => {
  let opened: any
  opened = wire((_: string, method: string) => method === 'pending' ? new Promise(() => {}) : 'slow', {
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
  const future = sent.find((frame: any) => frame.op === 'return' && frame.id === 'rust:1')?.value?.value?.id
  expect(typeof future).toBe('number')
  incoming.push(
    reply('bun:1', 'slow'),
    { op: 'await', id: 'rust:2', path: ['bun:1'], reference: future },
  )
  expect(peer.invoke('test', 'slow', [])).toBe('slow')
  // The await is taken, not refused: it waits for the promise, which can
  // settle now that the wait's reply was read already.
  await Promise.resolve()
  expect(sent.filter((frame: any) => frame.op === 'throw')).toEqual([])
  expect(fault()).toBeUndefined()
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
    const ran: string[] = []
    const { peer, sent, fault } = wire((_, method) => { ran.push(method); peer.close(); return 'after close' })
    peer.receive(invoke(1, 'during'))
    await new Promise(resolve => setImmediate(resolve))
    expect(ran).toEqual(['during'])
    expect(sent).toEqual([])
    expect(fault()).toBeUndefined()
    await peer.drain()
  }
  // A frame admitted but still queued: the close drops it, so its method never
  // starts, and nothing is written for it. `drain()` is deliberately not
  // awaited here: with this job dropped, `#active` stays above zero and
  // `drain()` never settles (#630). The only observable is a promise that does
  // not settle, so it is reported with the issue rather than asserted.
  {
    const ran: string[] = []
    const { peer, sent } = wire((_, method) => { ran.push(method); return 'queued' })
    peer.receive(invoke(1, 'queued'))
    peer.close()
    await new Promise(resolve => setImmediate(resolve))
    expect(ran).toEqual([])
    expect(sent).toEqual([])
  }
  // A synchronous call holding the thread when the close arrives through the
  // pump: the wait ends with the close's error on the first ask, and nothing
  // is asked after it.
  {
    // The step runs during the wait, after `wire` returned, so it closes the
    // session the wait belongs to.
    let opened: any
    opened = wire(() => 'unused', { steps: [() => opened.peer.close(new Error('the host went away'))] })
    const { peer, sent, asks, fault } = opened
    expect(() => peer.invoke('test', 'wait', [])).toThrow('the host went away')
    expect(asks).toEqual([false])
    expect(sent.map((frame: any) => frame.op)).toEqual(['invoke'])
    expect(fault()).toBeUndefined()
    await peer.drain()
  }
})

// §5.5 item 3 (EVP-5.5-9): a runtime must start emit-mode calls in arrival
// order. Bun is re-entrant, so two calls of the same name read during the
// wait run there, in frame order — the later one nested in the wait, never
// ahead of the earlier one. Node holds them back instead, which is Node's
// documented difference (node/rutis-runtime/test/frame-pump.test.mjs); Bun has
// no `holds` seam and needs none.
test('same-name emit calls read during a wait run there, in arrival order', async () => {
  const log: string[] = []
  const { peer, incoming, sent, asks, fault } = wire((_, __, args: any[]) => {
    log.push(`${args[0].name}:${args[0].args[0]}`)
    return undefined
  })
  const call = (id: number, name: string, value: any, path: string[] = []) => ({
    op: 'invoke', id: `rust:${id}`, path, target: '', method: 'events.call',
    args: data([{ listener: '1', mode: 'emit', name, args: [value] }]),
  })
  incoming.push(call(2, 'n', 1), call(3, 'n', 2, ['bun:1']), reply('bun:1', 7))
  expect(peer.invoke('test', 'sync', [])).toBe(7)
  expect(log).toEqual(['n:1', 'n:2'])
  expect(sent.filter(frame => frame.op === 'return').map(frame => frame.id)).toEqual(['rust:2', 'rust:3'])
  expect(asks).toEqual([false, false, false])
  expect(fault()).toBeUndefined()
  peer.close()
})

// §5.5 item 4 (#630): a filter's answer is a decision taken during the
// dispatch, so the runtime asks it synchronously (`#keeps` → `withinSync`). A
// filter that calls a service of the host waits on that call, and the pump
// reads the frames behind it. Bun runs them there: a later emit's listener
// call that arrives inside the filter's wait starts nested, before the filter
// answers — two emissions of one name from different processes are
// concurrent, and §5.5 gives them no order, so the host does not hold one
// behind the other. Node holds the call until the wait ends.
test("a filter's synchronous call runs a later emit nested (re-entrant)", async () => {
  const log: string[] = [], sent: any[] = [], asks: boolean[] = []
  const steps: Array<() => void> = []
  let fault: any
  let session: Session
  let bus: any
  session = new Session({
    endpoint: { local: 'bun' },
    dispatch: (_: string, method: string, args: any[]) => method === 'events.call' ? bus.call(args[0]) : 'checked',
    send: (line: string) => sent.push(JSON.parse(line)),
    pump: (done: () => boolean) => {
      asks.push(done())
      const step = steps.shift()
      if (!step) throw new Error('the pump was asked for a frame the script does not have')
      step()
    },
    abort: (error: Error) => { fault = error },
  })
  session.receive(greeting)
  bus = new Bus({
    runtime: 'bun',
    peer: {
      call: (target: string, method: string, args: any, described?: any) => session.invoke(target, method, args, described),
      callAsync: (target: string, method: string, args: any, described?: any) => session.invokeAsync(target, method, args, described),
      callAnswered: (target: string, method: string, args: any, answered: any, described?: any) => session.invokeAnswered(target, method, args, answered, described),
      callFunctionSync: (fn: any, args: any[]) => session.callFunctionSync(fn, args),
      callFunctionAsync: (fn: any, args: any[]) => session.callFunctionAsync(fn, args),
      withinSync: (run: () => any) => session.withinSync(run),
    },
    report: (line: string) => log.push(`reported: ${line}`),
  })
  bus.on({ name: 'n', callback: (value: any) => { log.push(`listener ${value}`) }, row: 'r', plugin: 'p' })
  const keep = () => {
    log.push('filter asked')
    log.push(`filter saw ${session.invoke('test', 'check', [])}`)
    return true
  }
  const out = (method: string) => sent.find(frame => frame.op === 'invoke' && frame.method === method)
  steps.push(() => session.receive({
    op: 'call', id: 'rust:1', path: [out('events.dispatch').id],
    reference: out('events.dispatch').args.value[0].value.filter.value.id,
    args: data([[{ listener: '1', name: 'n', plugin: 'p', row: 'r', runtime: 'bun', node: null, scope: [], global: false }]]),
    sync: true,
  }))
  steps.push(() => session.receive({
    op: 'invoke', id: 'rust:2', path: [], target: '', method: 'events.call',
    args: data([{ listener: '1', dispatch: 'later', own: false, name: 'n', mode: 'emit', args: [2] }]),
  }))
  steps.push(() => session.receive({ op: 'return', id: out('check').id, value: data('checked') }))
  steps.push(() => session.receive({ op: 'return', id: out('events.dispatch').id, value: data(undefined) }))
  bus.dispatch('emit', 'n', [1], { row: 'r', keep, sync: true })
  // The later emit's listener ran inside the filter's wait and answered
  // before the filter did: the nested call started first.
  expect(log).toEqual(['filter asked', 'listener 2', 'filter saw checked'])
  expect(asks).toEqual([false, false, false, false])
  expect(sent.filter(frame => frame.op === 'return').map(frame => frame.id)).toEqual(['rust:2', 'rust:1'])
  await new Promise(resolve => setImmediate(resolve))
  // Nothing else ran or answered after the wait: each of the two calls was
  // answered once, in the order it finished.
  expect(log).toEqual(['filter asked', 'listener 2', 'filter saw checked'])
  expect(sent.filter(frame => frame.op === 'return').map(frame => frame.id)).toEqual(['rust:2', 'rust:1'])
  expect(fault).toBeUndefined()
  session.close()
})
