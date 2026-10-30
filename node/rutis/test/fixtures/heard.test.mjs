// The test calls the test tool synchronously while its own listener of an
// emit is being called (#984): the driver, calling the listener, waits for
// the test, and the test waits for the driver. Run by
// ../testing-heard.test.mjs, which stops it if it hangs.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host } from '../../src/testing.mjs'
import teller from './teller.mjs'

test('synchronous calls while the test\'s listener is called', async () => {
  const h = host()
  const t = await h.load(teller)
  const told = []
  t.on('teller/told', name => { told.push(name) })
  assert.equal(await t.service('teller').tell('Ada'), 'Ada')
  // Every synchronous call of the test tool, for 1.5 s without a turn of
  // the event loop: the plugin emits 0.1 s in, while one of them waits.
  const until = Date.now() + 1500
  while (Date.now() < until) {
    assert.equal(typeof t.service('teller').tell, 'function')
    assert.deepEqual(t.methods('teller'), { tell: 'async' })
    assert.deepEqual(t.provided(), ['teller'])
    t.on('teller/other', () => {})()
    h.emit('teller/other')
    h.filter(() => true).emit('teller/other')
    assert.equal(h.bail('teller/other'), undefined)
    assert.equal(h.filter(() => true).bail('teller/other'), undefined)
  }
  for (let i = 0; i < 200 && !told.length; i++) await new Promise(resolve => setTimeout(resolve, 5))
  assert.deepEqual(told, ['Ada'])
  await h.unload()
})
