import { test } from 'node:test'
import assert from 'node:assert/strict'
import { decode, hasEscapedKey, parseExactly } from '../src/codec.mjs'

const literal = text => (0, eval)(`(${text})`)

// #302: from Node 24 (V8 13), once `JSON.parse` has read an object with key
// K, it reads a key T written with an escape, after the same keys in an
// object of the same shape, as K when T has as many UTF-16 units as K and T's
// JSON text starts with K, as long as at most 8 different keys have followed
// those keys so far. Each pair is [K, T]; K, read first, primes the parser.
const PAIRS = [
  ['\\', '\n'], ['\\', '\t'], ['\\', '\u0000'], ['\\', '\u001f'], ['\\', '"'], ['\\', 'é'],
  ['a\\', 'a\t'], ['\\n', '\nx'], ['\\u0', '\u0001xy'], ['x\\"', 'x"y'],
]

test('keys written with escapes survive after the parser has seen keys with backslashes (#302)', t => {
  let misread = 0
  const rounds = 200
  for (let round = 0; round < rounds; round++) {
    PAIRS.forEach(([seen, key], index) => {
      // Keys not used before lead to K and T: the object shape there is new,
      // so the misread happens on every affected Node, whatever ran before.
      const head = { [`#302 ${round}`]: 0, [`pair ${index}`]: 0 }
      const primer = JSON.stringify({ ...head, [seen]: round })
      assert.deepEqual(decode(primer), { ...head, [seen]: round })
      JSON.parse(primer)
      const text = JSON.stringify({ ...head, [key]: round })
      if (!Object.hasOwn(JSON.parse(text), key)) misread++
      assert.deepEqual(decode(text), { ...head, [key]: round })
      // Two such keys must not merge into one, at any depth.
      assert.deepEqual(decode(JSON.stringify({ ...head, [key]: 1, [seen]: 2 })), { ...head, [key]: 1, [seen]: 2 })
      assert.deepEqual(decode(JSON.stringify([{ x: { ...head, [key]: round } }])), [{ x: { ...head, [key]: round } }])
    })
    // A key spelled with a \u escape it need not use.
    assert.deepEqual(decode(`{"#302 ${round}":0,"\\\\":1}`), { [`#302 ${round}`]: 0, '\\': 1 })
    assert.deepEqual(decode(`{"#302 ${round}":0,"\\u0041":1}`), { [`#302 ${round}`]: 0, A: 1 })
  }
  t.diagnostic(`JSON.parse itself misread ${misread} of ${PAIRS.length * rounds} keys on Node ${process.version}`)
})

test('random objects with escaped keys round-trip through decode, and hasEscapedKey spots them', () => {
  let seed = 1
  const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648
  const pool = ['', 'a', '\n', '\t', '\\', '"', '¡', 'é', '😀', '\u0000', 'a\\', 'a\n', '\\n', '\nb', '__proto__', '0', '1']
  const pick = () => pool[Math.floor(random() * pool.length)]
  const make = depth => {
    const object = {}
    for (let n = Math.floor(random() * 4); n > 0; n--) {
      const key = random() < 0.3 ? pick() + pick() : pick()
      const roll = random()
      const value = depth > 0 && roll < 0.3 ? make(depth - 1) : roll < 0.4 ? [pick(), pick() + pick(), n] : roll < 0.7 ? pick() : n
      Object.defineProperty(object, key, { value, writable: true, enumerable: true, configurable: true })
    }
    return object
  }
  const escapedKey = value => value !== null && typeof value === 'object' && (Array.isArray(value)
    ? value.some(escapedKey)
    : Object.keys(value).some(key => JSON.stringify(key).includes('\\') || escapedKey(value[key])))
  for (let i = 0; i < 30000; i++) {
    const object = make(2)
    const text = JSON.stringify(object)
    assert.equal(hasEscapedKey(text), escapedKey(object), text)
    assert.deepStrictEqual(decode(text), object)
  }
})

test('hasEscapedKey finds escapes in keys only', () => {
  for (const value of [{}, { a: 'x' }, { a: 'x\ny' }, { a: 'x":' }, { a: 'x\\' }, { a: ['\\":', '"'] }, [1, '\\'], 'a\\b', { 'a b': '\u0000 :' }]) {
    assert.equal(hasEscapedKey(JSON.stringify(value)), false, JSON.stringify(value))
  }
  for (const value of [{ '\n': 1 }, { a: '\\', '"': 2 }, [{ x: { 'a\\': 1 } }], { a: 'x\\":', '\\': 1 }, { a: '\\\\"', b: { '"\\': 2 } }, { '\\\\': 1 }]) {
    assert.equal(hasEscapedKey(JSON.stringify(value)), true, JSON.stringify(value))
  }
  assert.equal(hasEscapedKey('{ "\\n" \t\r\n : 1 }'), true)
  assert.equal(hasEscapedKey('{"a":"\\\\" , "b":1}'), false)
})

test('parseExactly reads what JSON.parse reads', () => {
  const texts = [
    'null', 'true', 'false', '0', '-0', '1e400', '-1.5E-3', '12345678901234567890', '"\\ud83d\\ude00 \\/ \\b"',
    ' [ ] ', '{ }', '[1,[2,[3,{}]],{"a":[]}]', '{"a":1,"a":2}', '{"__proto__":{"x":1},"constructor":2}',
    '{"2":1,"1":2,"b":3,"a":4}', ' {\n "a" : [ true , false , null ] \r\t} ', '"\\u0000"',
    JSON.stringify({ text: 'x'.repeat(100000) + '\\"'.repeat(1000), '\n': ['\t', { '"': '😀' }] }),
  ]
  // JSON.parse may misread the escaped keys here (#302); the same text as a
  // JS literal means the same value, except for a `__proto__` key.
  for (const text of texts) assert.deepStrictEqual(parseExactly(text), text.includes('__proto__') ? JSON.parse(text) : literal(text), text.slice(0, 80))
  const proto = parseExactly('{"__proto__":{"x":1}}')
  assert.equal(Object.getPrototypeOf(proto), Object.prototype)
  assert.deepEqual(Object.getOwnPropertyNames(proto), ['__proto__'])
  const deep = '['.repeat(100000) + ']'.repeat(100000)
  let value = parseExactly(deep)
  for (let depth = 1; depth < 100000; depth++) value = value[0]
  assert.deepEqual(value, [])
})

test('parseExactly rejects what JSON.parse rejects', () => {
  const texts = [
    '', ' ', '{', '}', '[1,]', '{"a":1,}', '{"a" 1}', '{"\\n":}', '{a:1}', '"abc', '"\\x"', '"\u0001"',
    '01', '1.', '-', '+1', '.5', '1e', 'tru', 'nul', 'NaN', '[1] 2', '{"a":1}}', "'a'", '[1 2]', '{"\\n":1 "b":2}',
  ]
  for (const text of texts) {
    assert.throws(() => JSON.parse(text), SyntaxError, text)
    assert.throws(() => parseExactly(text), SyntaxError, text)
  }
})

test('parseExactly agrees with JSON on mutated input, and names the position of an error', () => {
  // Seeds hold escaped keys and values, nesting and every kind of token;
  // mutations insert, delete or replace characters that JSON gives meaning to.
  const seeds = [
    '{"\\n":[1,-2.5e3,true,false,null],"a\\\\":{"\\"":"x\\\\\\"y"},"b":[]}',
    '[{"\\u0041\\t":"\\ud83d\\ude00"},{"k":{"\\/":0}},"\\\\",{}]',
    ' { "a" : [ { "\\r" : "v\\n" } , 1e-7 ] , "\\b" : "" } ',
  ]
  const alphabet = ['{', '}', '[', ']', ',', ':', '"', '\\', 'n', 'u', '0', '1', '-', '.', 'e', ' ', '\n', 't', 'a']
  let seed = 7
  const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648
  const below = n => Math.floor(random() * n)
  let valid = 0
  for (let i = 0; i < 20000; i++) {
    let text = seeds[below(seeds.length)]
    for (let edits = 1 + below(3); edits > 0; edits--) {
      const at = below(text.length + 1)
      const roll = random()
      const char = alphabet[below(alphabet.length)]
      text = text.slice(0, at) + (roll < 0.3 ? '' : char) + text.slice(roll < 0.6 ? at : at + 1)
    }
    let json = true
    try { JSON.parse(text) } catch { json = false }
    if (json) {
      valid++
      // Valid JSON as a JS literal means the same value, read without JSON.parse.
      assert.deepStrictEqual(parseExactly(text), literal(text), text)
    } else {
      assert.throws(() => parseExactly(text), error => error instanceof SyntaxError && /position \d+|end of JSON/.test(error.message), text)
    }
  }
  assert.ok(valid > 1000, `only ${valid} mutated texts are JSON`)
  assert.throws(() => parseExactly('{"\\n":1,"b" 2}'), { name: 'SyntaxError', message: /position 12/ })
})
