// A row's config against its plugin's JSON Schema: defaults filled, then
// validated (config schema design, docs/design-config-schema-2026-10-27.en.md
// §3–§6). The rules every language's SDK has; the shared case list
// specs/config-schema/cases.json keeps them the same word for word, and its
// README states the details settled while implementing.
//
// This file is the same in the SDK's testing tool (node/rutis), the Node
// runtime (node/rutis-runtime) and the Bun runtime (bun/rutis-bun); a test
// checks the copies are equal. It knows nothing of the runtimes.
//
// Keywords checked: type, properties, required, additionalProperties,
// items, enum, const, minimum, maximum, default, anyOf, oneOf, allOf, $ref
// (to #/$defs/… and #/definitions/…). Others are neither checked nor
// warned about.

// At most this many problems are written; the rest as one last line.
const SHOWN = 20
const TYPES = ['string', 'number', 'integer', 'boolean', 'object', 'array', 'null']
// A plain object: what a JSON object is (a Date, Map or class instance is
// not one, and is left as it is).
const isObject = value => {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return false
  const prototype = Object.getPrototypeOf(value)
  return prototype === Object.prototype || prototype === null
}

// The schema a plugin declares as `config`, to fill and validate with:
// `undefined` when it has none, or not a JSON Schema object (a function
// of an older SDK, say). A Standard Schema object (schemastery, zod, …) is
// refused as `definePlugin` refuses it (config schema design §2.3).
export function schemaOf(config) {
  const standard = config?.['~standard']
  if (standard !== undefined) {
    throw new TypeError(`config of definePlugin should be a JSON Schema object, got ${standard?.vendor === 'schemastery' ? 'a schemastery Schema' : 'a Standard Schema'}`)
  }
  return isObject(config) ? config : undefined
}

// Fill `config` (undefined or null: no config) with the defaults of
// `schema`, then validate it: `{ config, errors, warnings }`, each problem
// `{ path, message }` (`path` a list of keys and indices), sorted by path.
// `config` is a copy: changing it changes nothing of the `config` given.
export function checkConfig(schema, config, { required = true } = {}) {
  // The schema as the host sees it (JSON): `undefined` keywords gone,
  // shared objects apart.
  try { schema = JSON.parse(JSON.stringify(schema)) } catch {}
  const doc = new Document(schema)
  const none = config === undefined || config === null
  let value
  if (!none) value = copy(config)
  else {
    const fallback = doc.defaultOf([schema])
    if (fallback !== undefined) value = copy(fallback)
    else if (doc.isObjectRoot(schema)) value = {}
    else value = null
  }
  doc.fill([schema], value)
  const sink = { errors: [], warnings: [], warn: true }
  const holder = { value }
  doc.validate([schema], holder, [], { required }, sink, new Map())
  value = holder.value
  if (value === null && none && sink.errors.length) sink.errors = [issue([], 'is required')]
  return { config: value, errors: sorted(sink.errors), warnings: sorted(sink.warnings) }
}

// The lines a config error is written as: `config: <problem>`, at most 20,
// then `config: and N more problems`.
export function configLines(issues) {
  const lines = issues.slice(0, SHOWN).map(problem => `config: ${problem.message}`)
  if (issues.length > SHOWN) lines.push(`config: and ${issues.length - SHOWN} more problems`)
  return lines
}

// The error a row fails with when its config does not match: a
// BindingError with code `bad-config`, its message the lines of
// `configLines`, its data `{ issues }` (design §5.2).
export function configError(issues) {
  const error = new Error(configLines(issues).join('\n'))
  Object.defineProperty(error, 'name', { value: 'BindingError', configurable: true, writable: true, enumerable: false })
  return Object.assign(error, { code: 'bad-config', data: { issues: issues.map(({ path, message }) => ({ path, message })) } })
}

// How a path is written: `home.lat`, `cities[2]`, `headers["x-api"]`;
// `config` for the root, and before a first index or non-identifier key.
export function pathText(path) {
  let out = ''
  path.forEach((segment, index) => {
    if (typeof segment === 'number') out += `${index === 0 ? 'config' : ''}[${segment}]`
    else if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(segment)) out += `${index === 0 ? '' : '.'}${segment}`
    else out += `${index === 0 ? 'config' : ''}[${quoted(segment, Infinity)}]`
  })
  return out || 'config'
}

// How a value is written after `got`: strings, numbers, booleans and null
// as JSON (a string past 40 scalar values cut with `…`); `an object`, `an
// array of N`.
export function valueText(value) {
  if (value === null || value === undefined) return 'null'
  if (typeof value === 'string') return quoted(value, 40)
  if (typeof value === 'number') return Number.isFinite(value) ? String(value) : 'null'
  if (typeof value === 'boolean') return String(value)
  if (Array.isArray(value)) return `an array of ${value.length}`
  return 'an object'
}

// `s` as a JSON string, its content cut to `limit` scalar values with `…`.
function quoted(s, limit) {
  const scalars = Array.from(s)
  if (scalars.length <= limit) return JSON.stringify(s)
  const cut = JSON.stringify(scalars.slice(0, limit).join(''))
  return `${cut.slice(0, -1)}…"`
}

// The name of `known` closest to `name`, when close enough to be a
// misspelling: at most one edit for 3–6 scalar values, two for more, none
// for fewer; the smallest distance, then the first in `known` (sorted by
// code point).
export function closest(name, known) {
  const length = Array.from(name).length
  if (length <= 2) return undefined
  const limit = length <= 6 ? 1 : 2
  let best
  let bestDistance = Infinity
  for (const candidate of known) {
    if (candidate === name) continue
    const d = distance(name, candidate)
    if (d <= limit && d < bestDistance) [best, bestDistance] = [candidate, d]
  }
  return best
}

// Edits (insert, delete, replace, swap two neighbours) between `a` and `b`,
// in scalar values.
export function distance(a, b) {
  a = Array.from(a)
  b = Array.from(b)
  const rows = Array.from({ length: a.length + 1 }, (_, i) => Array.from({ length: b.length + 1 }, (_, j) => (i === 0 ? j : j === 0 ? i : 0)))
  for (let i = 1; i <= a.length; i++) {
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1
      let best = Math.min(rows[i - 1][j] + 1, rows[i][j - 1] + 1, rows[i - 1][j - 1] + cost)
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) best = Math.min(best, rows[i - 2][j - 2] + 1)
      rows[i][j] = best
    }
  }
  return rows[a.length][b.length]
}

// Strings by code point (not by UTF-16 unit, as `<` compares).
export function byCodePoint(a, b) {
  const x = Array.from(a, c => c.codePointAt(0))
  const y = Array.from(b, c => c.codePointAt(0))
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    if (x[i] !== y[i]) return x[i] - y[i]
  }
  return x.length - y.length
}

// Paths segment by segment: keys by code point, indices as numbers, a key
// before an index, a prefix first.
function comparePaths(a, b) {
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    const [x, y] = [a[i], b[i]]
    if (typeof x === 'number' && typeof y === 'number') {
      if (x !== y) return x - y
      continue
    }
    if (typeof x === 'number') return 1
    if (typeof y === 'number') return -1
    const order = byCodePoint(x, y)
    if (order !== 0) return order
  }
  return a.length - b.length
}

// Problems sorted by path (stable: at one path, in the order found), each
// once.
function sorted(issues) {
  const seen = new Set()
  return issues
    .map((problem, index) => ({ problem, index }))
    .sort((a, b) => comparePaths(a.problem.path, b.problem.path) || a.index - b.index)
    .map(({ problem }) => problem)
    .filter(problem => {
      const key = JSON.stringify([problem.path, problem.message])
      if (seen.has(key)) return false
      seen.add(key)
      return true
    })
}

function issue(path, reason) {
  return { path: [...path], message: `${pathText(path)} ${reason}` }
}

// A JSON value copied: its arrays and plain objects new (a key whose value
// is `undefined` left out, as JSON has none); other values as they are.
function copy(value) {
  if (Array.isArray(value)) return value.map(copy)
  if (!isObject(value)) return value
  const out = {}
  for (const [key, item] of Object.entries(value)) if (item !== undefined) put(out, key, copy(item))
  return out
}

// `object[key] = value`, `__proto__` included as an own key.
function put(object, key, value) {
  Object.defineProperty(object, key, { value, writable: true, enumerable: true, configurable: true })
}

// JSON equality: numbers by value, objects regardless of key order.
function same(a, b) {
  if (Array.isArray(a) || Array.isArray(b)) return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((item, i) => same(item, b[i]))
  if (isObject(a) || isObject(b)) {
    if (!isObject(a) || !isObject(b)) return false
    const keys = Object.keys(a)
    return keys.length === Object.keys(b).length && keys.every(key => Object.hasOwn(b, key) && same(a[key], b[key]))
  }
  return a === b
}

// The type names of `type` this module knows.
function typesOf(type) {
  const names = typeof type === 'string' ? [type] : Array.isArray(type) ? type.filter(name => typeof name === 'string') : []
  return names.filter(name => TYPES.includes(name))
}

function isType(name, value) {
  switch (name) {
    case 'string': return typeof value === 'string'
    case 'number': return typeof value === 'number' && Number.isFinite(value)
    case 'integer': return Number.isInteger(value)
    case 'boolean': return typeof value === 'boolean'
    case 'object': return isObject(value)
    case 'array': return Array.isArray(value)
    case 'null': return value === null
  }
  return false
}

const TYPE_TEXT = { string: 'a string', number: 'a number', integer: 'an integer', boolean: 'a boolean', object: 'an object', array: 'an array', null: 'null' }

// `a`, `a or b`, `one of a, b, c`.
function joined(texts) {
  if (texts.length === 1) return texts[0]
  if (texts.length === 2) return `${texts[0]} or ${texts[1]}`
  return `one of ${texts.join(', ')}`
}

// The schema document `$ref`s resolve in.
class Document {
  constructor(root) {
    this.root = root
  }

  // `#/$defs/…` or `#/definitions/…` in the document.
  resolve(reference) {
    if (typeof reference !== 'string' || !(reference.startsWith('#/$defs/') || reference.startsWith('#/definitions/'))) return undefined
    let node = this.root
    for (const raw of reference.slice(2).split('/')) {
      const key = raw.replaceAll('~1', '/').replaceAll('~0', '~')
      // An array's item by its index (no leading zero), as a JSON Pointer.
      if (Array.isArray(node) && /^(0|[1-9][0-9]*)$/.test(key) && Number(key) < node.length) node = node[Number(key)]
      else if (isObject(node) && Object.hasOwn(node, key)) node = node[key]
      else return undefined
    }
    return node
  }

  // `schemas` and what they take in through `$ref` and `allOf`, each once:
  // a schema, then its `$ref`'s, then its `allOf` members' (depth-first
  // pre-order).
  parts(schemas) {
    const out = []
    const seen = new Set()
    const collect = schema => {
      if (!isObject(schema) || seen.has(schema)) return
      seen.add(schema)
      out.push(schema)
      const target = this.resolve(schema.$ref)
      if (target !== undefined) collect(target)
      if (Array.isArray(schema.allOf)) schema.allOf.forEach(collect)
    }
    schemas.forEach(collect)
    return out
  }

  // The first `default` of `schemas` (through `$ref` and `allOf`).
  defaultOf(schemas) {
    const part = this.parts(schemas).find(part => Object.hasOwn(part, 'default'))
    return part?.default
  }

  // The root's type is `object` or includes it.
  isObjectRoot(schema) {
    const part = this.parts([schema]).find(part => Object.hasOwn(part, 'type'))
    return part !== undefined && typesOf(part.type).includes('object')
  }

  static property(parts, key) {
    return parts.filter(part => isObject(part.properties) && Object.hasOwn(part.properties, key)).map(part => part.properties[key])
  }

  static additional(parts) {
    const part = parts.find(part => Object.hasOwn(part, 'additionalProperties'))
    return part?.additionalProperties
  }

  static known(parts) {
    const keys = new Set()
    for (const part of parts) if (isObject(part.properties)) for (const key of Object.keys(part.properties)) keys.add(key)
    return [...keys].sort(byCodePoint)
  }

  // Fill defaults level by level: in an object that is there, each missing
  // property with a `default`; then into the child objects and array
  // items. Not into `anyOf` / `oneOf`.
  fill(schemas, value) {
    const parts = this.parts(schemas)
    if (isObject(value)) {
      for (const part of parts) {
        if (!isObject(part.properties)) continue
        for (const [key, property] of Object.entries(part.properties)) {
          if (Object.hasOwn(value, key)) continue
          const fallback = this.defaultOf([property])
          if (fallback !== undefined) put(value, key, copy(fallback))
        }
      }
      const known = new Set(Document.known(parts))
      const additional = Document.additional(parts)
      for (const [key, child] of Object.entries(value)) {
        const below = known.has(key) ? Document.property(parts, key) : isObject(additional) ? [additional] : []
        if (below.length) this.fill(below, child)
      }
    } else if (Array.isArray(value)) {
      const below = parts.map(part => part.items).filter(isObject)
      if (below.length) value.forEach(item => this.fill(below, item))
    }
  }

  // Validate `holder.value` at `path` against every one of `schemas`.
  // `tried`: the `anyOf` / `oneOf` branches tried at this value, by schema
  // object: `'trying'`, `'failed'` or `{ passed }`.
  validate(schemas, holder, path, options, sink, tried) {
    const parts = this.parts(schemas)
    // The type first: when it is wrong, nothing else at or below it.
    for (const part of parts) {
      if (!Object.hasOwn(part, 'type')) continue
      const names = typesOf(part.type)
      if (!names.length) continue
      if (!names.some(name => isType(name, holder.value))) {
        sink.errors.push(issue(path, `should be ${joined(names.map(name => TYPE_TEXT[name]))}, got ${valueText(holder.value)}`))
        return
      }
    }
    for (const part of parts) {
      const value = holder.value
      if (Array.isArray(part.enum) && !part.enum.some(candidate => same(candidate, value))) {
        sink.errors.push(issue(path, `should be ${joined(part.enum.map(valueText))}, got ${valueText(value)}`))
      }
      if (Object.hasOwn(part, 'const') && !same(part.const, value)) {
        sink.errors.push(issue(path, `should be ${valueText(part.const)}, got ${valueText(value)}`))
      }
      if (typeof value === 'number') {
        if (typeof part.minimum === 'number' && value < part.minimum) sink.errors.push(issue(path, `should be at least ${valueText(part.minimum)}, got ${valueText(value)}`))
        if (typeof part.maximum === 'number' && value > part.maximum) sink.errors.push(issue(path, `should be at most ${valueText(part.maximum)}, got ${valueText(value)}`))
      }
      for (const keyword of ['anyOf', 'oneOf']) {
        const branches = part[keyword]
        if (!Array.isArray(branches)) continue
        let passed
        for (const branch of branches) {
          // At one value each branch is tried once: one being tried
          // (reached again through `$ref`) does not match, one tried
          // before gives its result again.
          let result
          const memo = isObject(branch) ? tried.get(branch) : undefined
          if (memo === 'trying' || memo === 'failed') result = undefined
          else if (memo !== undefined) result = { value: copy(memo.passed) }
          else {
            const trial = { errors: [], warnings: [], warn: false }
            const attempt = { value: copy(holder.value) }
            if (isObject(branch)) tried.set(branch, 'trying')
            this.validate([branch], attempt, path, options, trial, tried)
            result = trial.errors.length === 0 ? attempt : undefined
            if (isObject(branch)) tried.set(branch, result ? { passed: copy(result.value) } : 'failed')
          }
          if (result) {
            passed = result
            break
          }
        }
        if (passed) holder.value = passed.value
        else sink.errors.push(issue(path, `should be ${joined(this.branchTexts(branches))}, got ${valueText(holder.value)}`))
      }
    }
    const value = holder.value
    if (isObject(value)) {
      if (options.required) {
        for (const part of parts) {
          if (!Array.isArray(part.required)) continue
          for (const key of part.required) {
            if (typeof key === 'string' && !Object.hasOwn(value, key)) sink.errors.push(issue([...path, key], 'is required'))
          }
        }
      }
      const known = Document.known(parts)
      const additional = Document.additional(parts)
      const described = parts.some(part => Object.hasOwn(part, 'properties'))
      for (const key of Object.keys(value)) {
        const at = [...path, key]
        const child = { value: value[key] }
        if (known.includes(key)) {
          this.validate(Document.property(parts, key), child, at, options, sink, new Map())
        } else if (additional === false) {
          sink.errors.push(extra(at, key, known))
        } else if (isObject(additional)) {
          this.validate([additional], child, at, options, sink, new Map())
        } else if (additional === undefined && described && sink.warn) {
          sink.warnings.push(extra(at, key, known))
        }
        value[key] = child.value
      }
    } else if (Array.isArray(value)) {
      const below = parts.map(part => part.items).filter(isObject)
      if (below.length) {
        value.forEach((item, index) => {
          const child = { value: item }
          this.validate(below, child, [...path, index], options, sink, new Map())
          value[index] = child.value
        })
      }
    }
  }

  // How the branches of a failed `anyOf` / `oneOf` are written (§6.5): by
  // the first of `const`, `enum`, `type` a branch has through `$ref` and
  // `allOf`, in order, each once.
  branchTexts(branches) {
    const texts = []
    for (const branch of branches) {
      let found
      for (const part of this.parts([branch])) {
        if (Object.hasOwn(part, 'const')) {
          found = [valueText(part.const)]
          break
        }
        if (Array.isArray(part.enum)) {
          found = part.enum.map(valueText)
          break
        }
        if (Object.hasOwn(part, 'type')) {
          const names = typesOf(part.type)
          if (names.length) {
            found = names.map(name => TYPE_TEXT[name])
            break
          }
        }
      }
      for (const text of found ?? ['another accepted value']) if (!texts.includes(text)) texts.push(text)
    }
    return texts
  }
}

// `<path> is not a config key`, with a close known key suggested.
function extra(path, key, known) {
  const like = closest(key, known)
  return issue(path, like === undefined ? 'is not a config key' : `is not a config key (did you mean ${like}?)`)
}
