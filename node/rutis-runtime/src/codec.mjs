// Protocol frames to and from compact JSON text. The channel frames the
// messages; the codec adds no separator, and JSON escapes newlines inside
// strings, so an encoded frame never contains a raw newline.
export function encode(value) {
  return JSON.stringify(value, (_key, value) => {
    if (typeof value === 'number' && !Number.isFinite(value)) throw new TypeError('non-finite number')
    return value === undefined ? null : value
  })
}

// From Node 24 (V8 13), `JSON.parse` may read an object key written with an
// escape as another key it has seen after the same keys before: once
// `{"\\":…}` has been parsed, `{"\n":…}` comes back as `{"\\":…}` (#302).
// Only keys whose text holds an escape are misread, so a frame without one
// goes to `JSON.parse`, and a frame with one to `parseExactly`.
export function decode(text) {
  return hasEscapedKey(text) ? parseExactly(text) : JSON.parse(text)
}

const isSpace = c => c === 32 || c === 10 || c === 13 || c === 9

// The first quote at or after `from` that ends a string: one not escaped,
// that is, after an even number of backslashes; -1 when there is none.
function closingQuote(text, from) {
  for (let quote = text.indexOf('"', from); quote !== -1; quote = text.indexOf('"', quote + 1)) {
    let backslashes = 0
    while (text.charCodeAt(quote - 1 - backslashes) === 92) backslashes++
    if ((backslashes & 1) === 0) return quote
  }
  return -1
}

// Whether an object key in the JSON `text` holds an escape. A backslash
// stands inside a string (outside one it is not JSON, and `JSON.parse`
// rejects the text), and the first one after a string ends starts an escape
// in the next string; the string is a key when a colon follows its closing
// quote. Linear, and text without a backslash costs one `indexOf`.
export function hasEscapedKey(text) {
  let escape = text.indexOf('\\')
  while (escape !== -1) {
    const quote = closingQuote(text, escape + 1)
    if (quote === -1) return false
    let at = quote + 1
    while (isSpace(text.charCodeAt(at))) at++
    if (text.charCodeAt(at) === 58) return true
    escape = text.indexOf('\\', at)
  }
  return false
}

const HEX = /[0-9a-fA-F]/

// The position of the first character JSON does not allow in the string
// body `text[from, to)`: a control character, or what follows a backslash
// when it does not make an escape.
function badInString(text, from, to) {
  for (let at = from; at < to; at++) {
    const c = text.charCodeAt(at)
    if (c < 32) return at
    if (c !== 92) continue
    const next = text[++at]
    if (next === 'u') {
      for (let digit = 1; digit <= 4; digit++) if (!HEX.test(text[at + digit])) return at + digit
      at += 4
    } else if (!'"\\/bfnrt'.includes(next)) return at
  }
  return from
}

const NUMBER = /-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/y

// The value of the JSON `text`, as `JSON.parse` gives it, but with objects
// built here so every key is read on its own: strings still go through
// `JSON.parse` (a string alone is no key, and is read exactly), numbers
// through `Number`. Iterative, so nesting is not limited by the call stack.
export function parseExactly(text) {
  const length = text.length
  let at = 0
  const fail = () => {
    if (at >= length) throw new SyntaxError('Unexpected end of JSON input')
    const char = JSON.stringify(String.fromCodePoint(text.codePointAt(at)))
    throw new SyntaxError(`Unexpected character ${char} in JSON at position ${at}`)
  }
  const space = () => { while (isSpace(text.charCodeAt(at))) at++ }
  const string = () => {
    const quote = closingQuote(text, at + 1)
    if (quote === -1) { at = length; fail() }
    let value
    try {
      value = JSON.parse(text.slice(at, quote + 1))
    } catch {
      // Point at the character in the frame, not in the slice.
      at = badInString(text, at + 1, quote)
      fail()
    }
    at = quote + 1
    return value
  }
  const key = () => {
    space()
    if (text.charCodeAt(at) !== 34) fail()
    const name = string()
    space()
    if (text.charCodeAt(at) !== 58) fail()
    at++
    return name
  }
  // The enclosing containers, as a linked list: an array's `push` would run
  // setters on Array.prototype.
  let up = null
  let container
  let name
  for (;;) {
    space()
    let value
    const c = text.charCodeAt(at)
    if (c === 123 || c === 91) {
      at++
      space()
      const object = c === 123
      value = object ? {} : []
      if (text.charCodeAt(at) === (object ? 125 : 93)) at++
      else {
        up = { container, name, up }
        // Filled without a prototype, so no setter on one runs and
        // `__proto__` is an ordinary key; the prototype is set on closing.
        container = Object.setPrototypeOf(value, null)
        name = object ? key() : undefined
        continue
      }
    } else if (c === 34) value = string()
    else if (text.startsWith('true', at)) { value = true; at += 4 }
    else if (text.startsWith('false', at)) { value = false; at += 5 }
    else if (text.startsWith('null', at)) { value = null; at += 4 }
    else {
      NUMBER.lastIndex = at
      const number = NUMBER.exec(text)
      if (number === null) {
        if (c === 45) at++
        fail()
      }
      value = Number(number[0])
      at = NUMBER.lastIndex
      // `1.` and `1e`: the number is cut short where a digit is missing.
      const after = text.charCodeAt(at)
      if (after === 46 || after === 101 || after === 69) {
        at++
        if (text.charCodeAt(at) === 43 || text.charCodeAt(at) === 45) at++
        fail()
      }
    }
    for (;;) {
      if (container === undefined) {
        space()
        if (at < length) fail()
        return value
      }
      const array = Array.isArray(container)
      container[array ? container.length : name] = value
      space()
      const next = text.charCodeAt(at)
      if (next === 44) {
        at++
        if (!array) name = key()
        break
      }
      if (next !== (array ? 93 : 125)) fail()
      at++
      value = Object.setPrototypeOf(container, array ? Array.prototype : Object.prototype)
      name = up.name
      container = up.container
      up = up.up
    }
  }
}
