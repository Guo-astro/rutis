// Plugin definitions as values (docs/design-plugin-values-2026-10-25.md §5):
// a leaf plugin crosses as a `plugin` reference, only to a far end that
// declared `plugins`, and comes back home as itself. Cordis plugins cross as
// functions and objects, as before; the receiver decides they are plugins.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Session, ENDPOINT_PROTOCOL } from '../src/session.mjs'
import { VALUE_CAPABILITIES } from '../src/values.mjs'
import { definePlugin } from '../../rutis/src/index.mjs'

const data = value => ({ type: 'data', value })

// A session (as `node`) whose far end (`rust`) declared `capabilities`.
function harness(capabilities, dispatch = () => {}) {
  const sent = [], incoming = []
  let fault
  const peer = new Session({
    endpoint: { local: 'node' },
    dispatch,
    send: line => sent.push(JSON.parse(line)),
    pump: () => { assert.ok(incoming.length, 'sync caller must not strand'); peer.receive(incoming.shift()) },
    abort: error => { fault = error },
  })
  peer.start()
  peer.receive({ op: 'hello', version: ENDPOINT_PROTOCOL, endpoint: 'rust', capabilities: [...VALUE_CAPABILITIES, ...capabilities] })
  return { peer, sent, incoming, fault: () => fault }
}

const part = definePlugin({ inject: ['history'], apply() {} })

test('a leaf plugin crosses as a plugin reference to a far end that takes them', () => {
  const { peer, sent, incoming, fault } = harness(['plugins'])
  incoming.push({ op: 'return', id: 'node:1', value: data(null) })
  peer.invoke('', 'register', [part, part])
  const [first, second] = sent.find(frame => frame.op === 'invoke').args.value
  assert.equal(first.type, 'reference')
  assert.deepEqual({ kind: first.value.kind, home: first.value.home }, { kind: 'plugin', home: false })
  // The same value is one reference.
  assert.equal(second.value.id, first.value.id)
  assert.equal(fault(), undefined)
})

test('a far end that does not take plugin definitions fails only that call', () => {
  const { peer, sent, fault } = harness([])
  assert.throws(() => peer.invoke('', 'register', [part]), /does not accept plugin definitions/)
  assert.equal(sent.filter(frame => frame.op === 'invoke').length, 0)
  assert.equal(fault(), undefined)
})

test('a plugin definition sent home is the original value', () => {
  let seen
  const { peer, sent, incoming, fault } = harness(['plugins'], (target, method, args) => { seen = args[0]; return null })
  incoming.push({ op: 'return', id: 'node:1', value: data(null) })
  peer.invoke('', 'register', [part])
  const { id } = sent.find(frame => frame.op === 'invoke').args.value[0].value
  peer.receive({ op: 'invoke', id: 'rust:1', path: [], target: '', method: 'plugins.read', args: { type: 'list', value: [{ type: 'reference', value: { id, kind: 'plugin', home: true, origin: [] } }] } })
  return new Promise(resolve => setImmediate(resolve)).then(() => {
    assert.equal(seen, part)
    assert.equal(fault(), undefined)
  })
})

test('Cordis plugins cross as functions and objects, as before', () => {
  const { peer, sent, incoming } = harness(['plugins'])
  incoming.push({ op: 'return', id: 'node:1', value: data(null) })
  function plugin() {}
  peer.invoke('', 'register', [plugin, { inject: [], apply() {} }])
  const [fn, object] = sent.find(frame => frame.op === 'invoke').args.value
  assert.equal(fn.value.kind, 'function')
  assert.equal(object.value.kind, 'object')
})
