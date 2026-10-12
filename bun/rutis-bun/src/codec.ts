// Protocol frames to and from compact JSON text. The channel frames the
// messages; the codec adds no separator, and JSON escapes newlines inside
// strings, so an encoded frame never contains a raw newline.
export function encode(value) {
  return JSON.stringify(value, (_key, value) => {
    if (typeof value === 'number' && !Number.isFinite(value)) throw new TypeError('non-finite number')
    return value === undefined ? null : value
  })
}

// The Node runtime's codec works around a V8 `JSON.parse` defect that
// misreads escaped object keys (#302); JavaScriptCore reads them correctly.
export function decode(text) {
  return JSON.parse(text)
}
