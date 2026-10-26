// Error graphs preserve AggregateError order, shared causes, cycles and thrown
// values. They are data; decoding never selects an arbitrary global constructor.
const constructors = { Error, TypeError, RangeError, ReferenceError, SyntaxError, URIError, EvalError }
// The graph each error received from the other side came in, rooted at that
// error. Kept out of sight of the plugin's code: the error it gets has its
// own `stack`, and thrown on it crosses with the graph as received (§4.4).
const received = new WeakMap()

export function encodeError(thrown) {
  const nodes = [], seen = new Map()
  const visit = value => {
    if (typeof value === 'function' || typeof value === 'symbol') throw new TypeError('function or symbol in error graph is not supported')
    if (value === undefined) return { type: 'undefined' }
    if (typeof value === 'bigint') return { type: 'bigint', value: String(value) }
    if (typeof value === 'number' && !Number.isFinite(value)) return { type: 'number', value: String(value) }
    if (value === null || typeof value !== 'object') return { type: 'data', value }
    if (seen.has(value)) return { type: 'reference', value: seen.get(value) }
    const graph = received.get(value)
    if (graph) {
      // Spliced in as received: nodes appended, references shifted.
      const offset = nodes.length
      seen.set(value, offset + graph.root.value)
      nodes.push(...(offset === 0 ? graph.nodes : graph.nodes.map(node => shiftNode(node, offset))))
      // Where it was thrown on: one more `rethrown` item on its root, on a
      // copy (the graph kept stays as received).
      const at = frames(value.stack), root = nodes[offset + graph.root.value]
      const rethrown = Array.isArray(root.rethrown) ? root.rethrown : []
      if (at && rethrown.length < 8) nodes[offset + graph.root.value] = { ...root, rethrown: [...rethrown, at] }
      return { type: 'reference', value: offset + graph.root.value }
    }
    const id = nodes.length
    seen.set(value, id); nodes.push(null)
    if (value instanceof Error) {
      nodes[id] = {
        type: 'error', name: value.name, message: value.message, stack: value.stack,
        ...('cause' in value ? { cause: visit(value.cause) } : {}),
        ...(value instanceof AggregateError ? { errors: [...value.errors].map(visit) } : {}),
      }
      // A string `code` and its `data` (what a Rust service's error carries)
      // cross too; `data` only when it is data the graph can hold.
      if (typeof value.code === 'string') nodes[id].code = value.code
      if (Object.hasOwn(value, 'data')) {
        const data = attempt(value.data)
        if (data) nodes[id].data = data
      }
    } else if (Array.isArray(value)) nodes[id] = { type: 'array', values: value.map(visit) }
    else nodes[id] = { type: 'object', values: Object.entries(value).map(([key, value]) => [key, visit(value)]) }
    return { type: 'reference', value: id }
  }
  // `value` visited, or nothing (and nothing of it recorded) if it cannot be.
  const attempt = value => {
    const mark = nodes.length
    try {
      return visit(value)
    } catch {
      nodes.length = mark
      for (const [object, id] of seen) if (id >= mark) seen.delete(object)
      return undefined
    }
  }
  return { name: thrown?.name ?? 'ThrownValue', message: String(thrown?.message ?? thrown), graph: { root: visit(thrown), nodes } }
}

// The frames of a stack, without its "Name: message" line(s); none, nothing.
function frames(stack) {
  if (typeof stack !== 'string') return undefined
  const lines = stack.split('\n'), first = lines.findIndex(line => /^\s*at /.test(line))
  return first < 0 ? undefined : lines.slice(first).join('\n')
}

const shift = (value, offset) => value?.type === 'reference' ? { type: 'reference', value: value.value + offset } : value
function shiftNode(node, offset) {
  if (node.type === 'array') return { ...node, values: node.values.map(value => shift(value, offset)) }
  if (node.type === 'object') return { ...node, values: node.values.map(([key, value]) => [key, shift(value, offset)]) }
  const shifted = { ...node }
  if ('cause' in node) shifted.cause = shift(node.cause, offset)
  if (node.errors !== undefined) shifted.errors = node.errors.map(value => shift(value, offset))
  if (node.data !== undefined) shifted.data = shift(node.data, offset)
  return shifted
}

export function decodeError({ name, message, graph }) {
  const make = node => node.errors !== undefined ? new AggregateError([], node.message)
    : new (Object.hasOwn(constructors, node.name) ? constructors[node.name] : Error)(node.message)
  if (!graph) return Object.assign(make({ name, message }), { name })
  // A graph not in its shape (an older or another far end's) does not end
  // the session: the error is then its name and message (#675).
  try { return decodeGraph(graph, make) } catch { return Object.assign(make({ name, message }), { name }) }
}

function decodeGraph(graph, make) {
  if (!Array.isArray(graph.nodes)) throw new TypeError('invalid error graph')
  const nodes = graph.nodes.map(node => {
    // `stack` stays this process's own; the other side's is in the graph.
    if (node.type === 'error') return own(Object.assign(make(node), { name: node.name }))
    if (node.type === 'array') return []
    if (node.type === 'object') return {}
    throw new TypeError('invalid error node')
  })
  const read = value => {
    switch (value.type) {
      case 'undefined': return undefined
      case 'data': return value.value
      case 'bigint': return BigInt(value.value)
      case 'number': return Number(value.value)
      case 'reference':
        if (!Number.isSafeInteger(value.value) || value.value < 0 || value.value >= nodes.length) throw new TypeError('invalid error reference')
        return nodes[value.value]
      default: throw new TypeError('invalid error value')
    }
  }
  graph.nodes.forEach((node, id) => {
    const target = nodes[id]
    if (node.type === 'error') {
      if ('cause' in node) Object.defineProperty(target, 'cause', { value: read(node.cause), configurable: true, writable: true })
      if (node.errors !== undefined) target.errors = node.errors.map(read)
      // What a Rust service's `Err(e)` carries: `code` and `data` (#440).
      if (typeof node.code === 'string') Object.defineProperty(target, 'code', { value: node.code, enumerable: true, configurable: true, writable: true })
      if (node.data !== undefined) Object.defineProperty(target, 'data', { value: read(node.data), enumerable: true, configurable: true, writable: true })
    } else if (node.type === 'array') target.push(...node.values.map(read))
    else for (const [key, value] of node.values) Object.defineProperty(target, key, { value: read(value), enumerable: true, configurable: true, writable: true })
  })
  const root = read(graph.root)
  graph.nodes.forEach((node, id) => {
    if (node.type === 'error') received.set(nodes[id], { root: { type: 'reference', value: id }, nodes: graph.nodes })
  })
  return root
}

// The stack from where the error is decoded, without the decoding's frames.
function own(error) {
  if (typeof Error.captureStackTrace === 'function') Error.captureStackTrace(error, decodeError)
  return error
}
