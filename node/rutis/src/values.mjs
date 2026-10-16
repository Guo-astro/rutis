// The tagged values beyond JSON data (#308, #355, #360): a point in time,
// a local date and time, a date, a time of day, binary data, a map, a set,
// a big integer, a decimal and a UUID. Each crosses a session under its own
// tag, as text in the formats below, and only to a far end that declared
// the capability `value-<tag>`.
//
// | tag        | JavaScript sends        | receives     | text on the wire                     |
// | ---------- | ----------------------- | ------------ | ------------------------------------ |
// | `instant`  | `Date`                  | `Date`       | `2026-10-15T03:22:15.123Z`, UTC      |
// | `datetime` | `rutis.datetime(text)`  | string       | `2026-10-15T14:30:05.25`, no zone    |
// | `date`     | `rutis.date(text)`      | string       | `2026-10-15`                         |
// | `time`     | `rutis.time(text)`      | string       | `14:30:05.25`, 24-hour, no zone      |
// | `bytes`    | `Uint8Array` (`Buffer`), `ArrayBuffer`, `DataView` | `Uint8Array` | base64, padded |
// | `map`      | `Map`                   | `Map`        | `[[key, value], …]`, in order        |
// | `set`      | `Set`                   | `Set`        | `[element, …]`, in order             |
// | `bigint`   | `bigint`                | `bigint`     | decimal integer, at most 4300 digits |
// | `decimal`  | `rutis.decimal(text)`   | string       | `-12.3400`, trailing zeros kept      |
// | `uuid`     | `rutis.uuid(text)`      | string       | lowercase `8-4-4-4-12` hex           |
//
// Years are 0001–9999; fractions of a second have 0–9 digits; an instant is
// truncated to the millisecond toward negative infinity when received. The
// constructors at the end make objects marked with
// `Symbol.for('rutis.value')`, so the runtimes know them without importing
// this package; the testing tool crosses values by the same rules. This is
// the SDK's own copy of the runtimes' rules (node/rutis-runtime/src/
// values.mjs). Design: docs/design-value-types-2026-10-15.md; the
// normative wire text is in docs/design-protocol-plugins-2026-09-25.md
// §6.1; the Rust reference is crates/rutis-bridge/src/session/values.rs.

// The mark on what the SDK constructors make: `{ [MARK]: tag, value: text }`.
export const MARK = Symbol.for('rutis.value')
// The tags the SDK constructors make; the rest are JavaScript values.
const CONSTRUCTED = ['datetime', 'date', 'time', 'decimal', 'uuid']
// The most digits a bigint or decimal may have: Python's default limit on
// integer text.
export const MAX_DIGITS = 4300

// A value this side cannot send or take: only the call carrying it fails,
// the session goes on (unlike a frame not in the protocol's shape).
export class ValueError extends TypeError {}

// A text a value error quotes, shortened to 40 characters.
const quoted = text => text.length > 40 ? `${JSON.stringify(text.slice(0, 40))}…` : JSON.stringify(text)
const invalid = (what, text, expected) => new ValueError(`invalid ${what} ${quoted(text)}: expected ${expected}`)

// ── The text formats ──────────────────────────────────────────────────

const DATE = /^(\d{4})-(\d{2})-(\d{2})$/
const TIME = /^(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?$/

const leap = year => year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0)
const daysInMonth = (year, month) => month === 2 ? (leap(year) ? 29 : 28) : [4, 6, 9, 11].includes(month) ? 30 : 31

// `[year, month, day]` of `YYYY-MM-DD`, a day of years 0001–9999.
function readDate(text) {
  const match = DATE.exec(text)
  if (!match) return undefined
  const [year, month, day] = match.slice(1).map(Number)
  if (year < 1 || month < 1 || month > 12 || day < 1 || day > daysInMonth(year, month)) return undefined
  return [year, month, day]
}
// `[hour, minute, second, fraction digits]` of `HH:MM:SS[.f…]`.
function readTime(text) {
  const match = TIME.exec(text)
  if (!match) return undefined
  const [hour, minute, second] = match.slice(1, 4).map(Number)
  if (hour > 23 || minute > 59 || second > 59) return undefined
  return [hour, minute, second, match[4] ?? '']
}
// `[date, time]` of `YYYY-MM-DDTHH:MM:SS[.f…]`.
function readDateTime(text) {
  const at = text.indexOf('T')
  if (at !== 10) return undefined
  const date = readDate(text.slice(0, 10)), time = readTime(text.slice(11))
  return date && time ? [date, time] : undefined
}

// Days from 1970-01-01 to `year-month-day` (H. Hinnant's `days_from_civil`).
function daysFromCivil(year, month, day) {
  year -= month <= 2 ? 1 : 0
  const era = Math.floor(year / 400)
  const yearOfEra = year - era * 400
  const dayOfYear = Math.floor((153 * ((month + 9) % 12) + 2) / 5) + day - 1
  const dayOfEra = yearOfEra * 365 + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100) + dayOfYear
  return era * 146097 + dayOfEra - 719468
}

// 0001-01-01T00:00:00Z and 9999-12-31T23:59:59.999Z, in milliseconds.
const FIRST_MS = -62135596800000
const LAST_MS = 253402300799999

// Checks of each text, as the wire takes it.
const formats = {
  instant: text => text.endsWith('Z') && readDateTime(text.slice(0, -1)) ? undefined : 'RFC 3339 in UTC, YYYY-MM-DDTHH:MM:SS[.fraction]Z, years 0001–9999',
  datetime: text => readDateTime(text) ? undefined : 'YYYY-MM-DDTHH:MM:SS[.fraction] without a time zone, years 0001–9999',
  date: text => readDate(text) ? undefined : 'YYYY-MM-DD, years 0001–9999',
  time: text => readTime(text) ? undefined : 'HH:MM:SS[.fraction], 24-hour, 0–9 fraction digits',
  bigint: text => /^-?(0|[1-9]\d*)$/.test(text) && text !== '-0' && text.replace('-', '').length <= MAX_DIGITS ? undefined : `a decimal integer without leading zeros, at most ${MAX_DIGITS} digits`,
  decimal: text => /^-?(0|[1-9]\d*)(\.\d+)?$/.test(text) && text.replace(/[-.]/g, '').length <= MAX_DIGITS ? undefined : `decimal digits with an optional fraction, no exponent, at most ${MAX_DIGITS} digits`,
  uuid: text => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(text) ? undefined : 'lowercase hex 8-4-4-4-12',
}

// `text`, if it is a valid `tag` value; else a ValueError.
export function checkText(tag, text) {
  if (typeof text !== 'string') throw new ValueError(`invalid ${tag}: expected its text, not ${describe(text)}`)
  const expected = formats[tag](text)
  if (expected) throw invalid(tag, text, expected)
  return text
}

// The Date an instant's text names, truncated to the millisecond toward
// negative infinity (design V7).
export function instantFromText(text) {
  checkText('instant', text)
  const [[year, month, day], [hour, minute, second, fraction]] = readDateTime(text.slice(0, -1))
  const seconds = daysFromCivil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second
  return new Date(seconds * 1000 + Number(fraction.padEnd(3, '0').slice(0, 3)))
}

// A Date's text as an instant; a ValueError for an invalid Date or one
// outside years 0001–9999.
export function instantText(date) {
  const time = date.getTime()
  if (Number.isNaN(time)) throw new ValueError('cannot send an invalid Date')
  if (time < FIRST_MS || time > LAST_MS) throw new ValueError(`cannot send the Date ${date.toISOString()}: an instant is within years 0001–9999`)
  // `YYYY-MM-DDTHH:MM:SS.sssZ`, without trailing zeros in the fraction.
  return date.toISOString().replace(/\.?0*Z$/, 'Z')
}

// ── Binary data ──────────────────────────────────────────────────────

// The bytes of a Uint8Array, ArrayBuffer or DataView (a Buffer is a
// Uint8Array); undefined for anything else.
export function bytesOf(value) {
  if (value instanceof Uint8Array) return value
  if (value instanceof ArrayBuffer) return new Uint8Array(value)
  if (value instanceof DataView) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
  return undefined
}

// ── What crosses as a tagged value ───────────────────────────────────

// The tag of an SDK constructor's value, if `value` is one.
export const constructedTag = value => value !== null && typeof value === 'object' && CONSTRUCTED.includes(value[MARK]) ? value[MARK] : undefined

// The tag `value` crosses under, if it crosses as a tagged value. Other
// typed arrays (Int16Array, ...) are not bytes: their element type would
// be lost.
export function tagOf(value) {
  if (typeof value === 'bigint') return 'bigint'
  if (value === null || typeof value !== 'object') return undefined
  if (value instanceof Date) return 'instant'
  if (value instanceof Map) return 'map'
  if (value instanceof Set) return 'set'
  if (bytesOf(value)) return 'bytes'
  return constructedTag(value)
}

// What `value` is, for errors.
export function describe(value) {
  if (value === null) return 'null'
  if (value === undefined) return 'undefined'
  if (Array.isArray(value)) return 'an array'
  const tag = tagOf(value)
  if (tag) return { instant: 'a Date', bytes: 'binary data', map: 'a Map', set: 'a Set', bigint: 'a bigint' }[tag] ?? `a ${tag}`
  if (ArrayBuffer.isView(value)) return `a ${value.constructor.name}`
  if (typeof value === 'object') return 'an object'
  return `a ${typeof value}`
}

// ── Map keys and set elements (design §5) ────────────────────────────

// The fraction of a time without trailing zeros: `14:30:05.250` and
// `14:30:05.25` are one time.
const shortTime = text => text.replace(/(\.\d*?)0+$/, '$1').replace(/\.$/, '')

const unsafeKey = (what, number) => new ValueError(`a ${what} cannot be the integer ${number}, beyond ±(2^53-1): send it as a bigint`)
const refusedKey = (what, kind) => new ValueError(`a ${what} cannot be ${kind}: only strings, finite numbers, booleans, bigint, datetime, date, time, decimal and uuid values can (an instant, bytes, null, an array, an object, a map, a set or a reference cannot)`)

// How a key compares with the others of its map (`what` "map key") or set
// ("set element"), as `tag` and its JavaScript value: numbers by value (0
// and -0 are one), the rest by kind and text. A ValueError for a value
// that is not a key in every language.
export function keyIdentity(tag, value, what) {
  switch (tag) {
    case 'data':
      if (typeof value === 'string') return `s${value}`
      if (typeof value === 'boolean') return `b${value}`
      if (typeof value === 'number') {
        if (!Number.isFinite(value)) throw refusedKey(what, String(value))
        // Beyond ±(2^53−1) two integers may be one double: send a bigint.
        if (Number.isInteger(value) && !Number.isSafeInteger(value)) throw unsafeKey(what, value)
        return `n${value === 0 ? 0 : value}`
      }
      throw refusedKey(what, describe(value))
    case 'bigint': return `bigint${value}`
    case 'datetime': case 'time': return `${tag}${shortTime(value)}`
    case 'date': case 'decimal': case 'uuid': return `${tag}${value}`
    default: throw refusedKey(what, tag === 'instant' ? 'an instant' : describe(value))
  }
}

// A key or element this side sends, as `[tag, value]` for keyIdentity:
// an SDK constructor's value as its tag and checked text.
export function keyOf(value) {
  const constructed = constructedTag(value)
  if (constructed) return [constructed, checkText(constructed, value.value)]
  if (value === null || typeof value === 'object' || typeof value === 'function' || value === undefined) return [tagOf(value) ?? 'other', value]
  return [typeof value === 'bigint' ? 'bigint' : 'data', value]
}

// Each `[tag, value]` of `keys` a key, no two equal (a language that merged
// them would lose an entry); then none equal once received in JavaScript
// (`received`, the Map or Set made), where a date, decimal or ... is its
// text and may meet a string key.
export function checkKeys(keys, what, received) {
  const seen = new Set()
  for (const [tag, value] of keys) {
    const identity = keyIdentity(tag, value, what)
    if (seen.has(identity)) throw new ValueError(`duplicate ${what} ${shownKey(value)}: a map or set whose keys collide would lose entries`)
    seen.add(identity)
  }
  if (received && received.size !== seen.size) {
    throw new ValueError(`duplicate ${what} once received: two ${what}s are one key in JavaScript (a datetime, date, time, decimal or uuid arrives as its text), so an entry would be lost`)
  }
}

function shownKey(value) {
  const text = typeof value === 'string' ? JSON.stringify(value) : String(value)
  return text.length > 40 ? `${text.slice(0, 40)}…` : text
}

// ── The constructors ─────────────────────────────────────────────────

// A local date and time, date, time of day, decimal or UUID: JavaScript
// has no type for them, so a plugin receives them as strings and sends
// them as one of these. A string stays a string.
class Value {
  constructor(tag, text) {
    this[MARK] = tag
    this.value = text
    Object.freeze(this)
  }
  toString() { return this.value }
  toJSON() { return this.value }
}

const make = (tag, text) => new Value(tag, checkText(tag, text))
// `HH:MM` as `HH:MM:00`: the wire always has seconds.
const withSeconds = text => typeof text === 'string' ? text.replace(/(^|T)(\d{2}:\d{2})$/, '$1$2:00') : text

/** A local date and time without a time zone (`datetime`): `'2026-10-15T14:30'`, `'2026-10-15T14:30:05.25'`. */
export const datetime = text => make('datetime', withSeconds(text))
/** A calendar date (`date`): `'2026-10-15'`. */
export const date = text => make('date', text)
/** A time of day without a time zone (`time`): `'14:30'`, `'14:30:05.25'`. */
export const time = text => make('time', withSeconds(text))
/** A decimal number (`decimal`), its trailing zeros kept: `'1.10'`, `'-12.3400'`; a bigint is taken as its digits. */
export const decimal = text => make('decimal', typeof text === 'bigint' ? String(text) : text)
/** A UUID (`uuid`), in either case; sent in lowercase. */
export const uuid = text => make('uuid', typeof text === 'string' ? text.toLowerCase() : text)
