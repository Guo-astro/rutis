import { fileURLToPath } from 'node:url'
import { isAbsolute, relative } from 'node:path'
// Error graphs preserve AggregateError order, shared causes, cycles and thrown
// values. They are data; decoding never selects an arbitrary global constructor.
const constructors = { Error, TypeError, RangeError, ReferenceError, SyntaxError, URIError, EvalError }
// The graph each error received from the other side came in, rooted at that
// error. Kept out of sight of the plugin's code: the error it gets has its
// own `stack`, and thrown on it crosses with the graph as received (§4.4).
const received = new WeakMap()

// The directory this runtime's own modules are in: their frames, and those
// of Cordis, are not the plugin's and stay out of the stack that crosses
// (§4.5); the plugin's frames and those of its own dependencies stay.
// Stacks name a module by its path or its file: URL.
const runtimeDirs = [new URL('.', import.meta.url), new URL('../node_modules/', import.meta.url)]
const NOT_THE_PLUGINS = [
  ...runtimeDirs.flatMap(url => [url.href, fileURLToPath(url)]),
  'node_modules/@deepseek-ai/cordis/', 'node_modules\\@deepseek-ai\\cordis\\', '@arcships/rutis/src/', '@arcships\\rutis\\src\\',
  // Node's and Bun's own modules.
  '(node:', 'at node:', '(native:', 'at native:',
]
function pluginStack(stack) {
  if (typeof stack !== 'string') return stack
  return stack.split('\n').filter(line => !/^\s*at /.test(line) || !NOT_THE_PLUGINS.some(part => line.includes(part))).join('\n')
}

// A thrown value that is not an error, shown as the developer wrote it:
// `"need 5 words"`, `{code: 3}`, `[1, 2]`, `42` (§4.5). Short: nested
// values beyond two levels and long texts are cut.
export function shown(value, depth = 0) {
  if (typeof value === 'string') return JSON.stringify(value.length > 200 ? `${value.slice(0, 200)}…` : value)
  if (typeof value === 'bigint') return `${value}n`
  if (value === undefined || value === null || typeof value !== 'object') return String(value)
  if (depth >= 2) return Array.isArray(value) ? '[…]' : '{…}'
  if (Array.isArray(value)) {
    const items = value.slice(0, 10).map(item => shown(item, depth + 1))
    return `[${items.join(', ')}${value.length > 10 ? ', …' : ''}]`
  }
  const entries = Object.entries(value)
  const items = entries.slice(0, 10).map(([key, item]) => `${/^[A-Za-z_$][\w$]*$/.test(key) ? key : JSON.stringify(key)}: ${shown(item, depth + 1)}`)
  return `{${items.join(', ')}${entries.length > 10 ? ', …' : ''}}`
}
// Whether `value` is an error, or reads as one (an error of another realm).
const errorLike = value => value instanceof Error || (value !== null && typeof value === 'object' && typeof value.message === 'string' && typeof value.name === 'string')

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
      const at = frames(pluginStack(value.stack)), root = nodes[offset + graph.root.value]
      const rethrown = Array.isArray(root.rethrown) ? root.rethrown : []
      if (at && rethrown.length < 8) nodes[offset + graph.root.value] = { ...root, rethrown: [...rethrown, at] }
      return { type: 'reference', value: offset + graph.root.value }
    }
    const id = nodes.length
    seen.set(value, id); nodes.push(null)
    if (value instanceof Error) {
      nodes[id] = {
        type: 'error', name: value.name, message: value.message, stack: pluginStack(value.stack),
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
  // A thrown value that is not an error is named ThrownValue; its message
  // is the value shown, which is what is printed (§8.2).
  const head = errorLike(thrown) ? { name: thrown.name, message: thrown.message } : { name: 'ThrownValue', message: shown(thrown) }
  return { ...head, graph: { root: visit(thrown), nodes } }
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

// `site`: where this process made the call the error answers (`callSite`):
// the error's stack is the plugin's own from there (§4.4).
export function decodeError({ name, message, graph }, site) {
  const make = node => node.errors !== undefined ? new AggregateError([], node.message)
    : new (Object.hasOwn(constructors, node.name) ? constructors[node.name] : Error)(node.message)
  if (!graph) return own(Object.assign(make({ name, message }), { name }), site)
  // A graph not in its shape (an older or another far end's) does not end
  // the session: the error is then its name and message (#675).
  try { return decodeGraph(graph, make, site) } catch { return own(Object.assign(make({ name, message }), { name }), site) }
}

function decodeGraph(graph, make, site) {
  if (!Array.isArray(graph.nodes)) throw new TypeError('invalid error graph')
  const nodes = graph.nodes.map(node => {
    // `stack` stays this process's own; the other side's is in the graph.
    if (node.type === 'error') return own(Object.assign(make(node), { name: node.name }), site)
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

// Where this process makes a call, taken as it is made; its stack is read
// only when an error answers it.
export function callSite() {
  const site = {}
  if (typeof Error.captureStackTrace === 'function') Error.captureStackTrace(site, callSite)
  return site
}

// The error's stack: the plugin's frames where it made the call (`site`),
// else where the error is decoded.
function own(error, site) {
  const at = typeof site?.stack === 'string' ? frames(pluginStack(site.stack)) : undefined
  if (at) Object.defineProperty(error, 'stack', { value: `${error.name}: ${error.message}\n${at}`, configurable: true, writable: true })
  else if (typeof Error.captureStackTrace === 'function') Error.captureStackTrace(error, decodeError)
  return error
}

// What calls in progress fail with once the host is gone (§4.3): the
// reason, when there is one beyond that, only in `data`.
export const HOST_GONE = 'the host disconnected'
export function hostGone(reason) {
  return bindingError('peer-gone', HOST_GONE, reason && reason !== HOST_GONE ? { reason } : {})
}

// The errors this runtime makes when it refuses a call or a use (§4.3):
// a BindingError with a code for programs and data with the details; the
// text only uses names the developer wrote. The same in every runtime.
export function bindingError(code, message, data) {
  const error = new Error(message)
  Object.defineProperty(error, 'name', { value: 'BindingError', configurable: true, writable: true, enumerable: false })
  return Object.assign(error, { code, data })
}
// `<service> has no method <method> (methods: a, b)`, sorted.
export function noMethod(service, method, methods) {
  const sorted = [...methods].sort()
  return bindingError('no-method', `${service} has no method ${method} (${sorted.length ? `methods: ${sorted.join(', ')}` : 'no methods'})`, { service, method, methods: sorted })
}
// A service its plugin uses, which neither the plugin's `inject` nor its
// row's declares: what to declare, not that it is missing (#676).
export const UNDECLARED = name => `service ${name} is not available to this plugin, which does not declare it in inject: add '${name}' to its inject`
export const notDeclared = service => bindingError('not-declared', UNDECLARED(service), { service })
// A declared service nothing provides (now).
export const unavailable = (service, reason) =>
  bindingError('unavailable', `service ${service} is not available${reason ? ` (${reason})` : ''}`, reason ? { service, reason } : { service })
// The name a developer gave a service, without the scope label this
// runtime keys it by (`name\0label`).
export const serviceName = id => String(id).split('\0')[0]

// ── Loading a plugin (§8.1 row 5) ──────────────────────────────────

// A file as the developer knows it: relative to where the host runs, when
// it is under it.
export function fileShown(file, cwd = process.cwd()) {
  let path = String(file)
  if (path.startsWith('file:')) { try { path = fileURLToPath(path) } catch { return path } }
  if (!isAbsolute(path)) return path
  const near = relative(cwd, path)
  return near && !near.startsWith('..') && !isAbsolute(near) ? near : path
}

// The name of the error a plugin that cannot be loaded fails with; its
// message already says what failed, the error the module threw included.
export const LOAD_ERROR = 'LoadError'
// What importing a plugin's module threw, said of the module that threw it
// (a submodule the entry imports, say), not of the entry:
// `cannot load src/util.ts: RangeError: bad util (src/util.ts:3:7)`.
export function cannotLoad(thrown, entry, cwd = process.cwd()) {
  const { file, line, column, name, message } = loadFailure(thrown)
  const shownFile = fileShown(file ?? entry, cwd)
  const where = line !== undefined ? ` (${shownFile}:${line}${column !== undefined ? `:${column}` : ''})` : ''
  const error = new Error(`cannot load ${shownFile}: ${name ? `${name}: ` : ''}${message}${where}`, { cause: thrown })
  Object.defineProperty(error, 'name', { value: LOAD_ERROR, configurable: true, writable: true, enumerable: false })
  error.stack = [`${LOAD_ERROR}: ${error.message}`, ...(pluginFrames(thrown?.stack) ?? [])].join('\n')
  return error
}

// The entry file of a row that was there and is no more.
export function entryGone(entry, cwd = process.cwd()) {
  const error = new Error(`the entry file ${fileShown(entry, cwd)} is gone`)
  Object.defineProperty(error, 'name', { value: LOAD_ERROR, configurable: true, writable: true, enumerable: false })
  return error
}

// A module whose default export is not a plugin.
export function notDefinePlugin(entry, cwd = process.cwd()) {
  const error = new Error(`the default export of ${fileShown(entry, cwd)} is not a plugin made with definePlugin`)
  Object.defineProperty(error, 'name', { value: LOAD_ERROR, configurable: true, writable: true, enumerable: false })
  return error
}

const FRAME = /^\s*at (?:.*? \()?((?:file:\/\/)?[^()\s]+?):(\d+):(\d+)\)?$/
function pluginFrames(stack) {
  if (typeof stack !== 'string') return undefined
  return pluginStack(stack).split('\n').filter(line => FRAME.test(line))
}

// Which file threw, where, and what, however the loader said it.
function loadFailure(thrown) {
  // A thrown value that is not an error is shown alone, as written.
  const name = errorLike(thrown) || typeof thrown?.position === 'object' ? String(thrown.name) : undefined
  const text = errorLike(thrown) ? String(thrown.message) : shown(thrown)
  const firstLine = text.split('\n')[0]
  // Bun: a syntax or resolution error says where in `position`.
  const at = thrown?.position
  if (at && typeof at.file === 'string' && typeof at.line === 'number') {
    return { file: at.file, line: at.line, column: typeof at.column === 'number' ? at.column : undefined, name: name === 'BuildMessage' ? 'SyntaxError' : name, message: firstLine.replace(/ imported from .*$/, '') }
  }
  // tsx (esbuild): `Transform failed with 1 error:\n<file>:<line>:<column>: ERROR: <text>`.
  const transform = /^(.+?):(\d+):(\d+): ERROR: (.*)$/m.exec(text)
  if (name === 'TransformError' || (transform && /^Transform failed/.test(text))) {
    if (transform) return { file: transform[1], line: Number(transform[2]), column: Number(transform[3]), name: 'SyntaxError', message: transform[4] }
  }
  // A module not found: said of the module that imports it.
  if (thrown?.code === 'MODULE_NOT_FOUND' || thrown?.code === 'ERR_MODULE_NOT_FOUND' || name === 'ResolveMessage') {
    const importer = /\nRequire stack:\n- (.+)/.exec(text)?.[1] ?? / imported from (\S+)/.exec(text)?.[1]
    return { file: importer, name: name === 'ResolveMessage' ? 'Error' : name, message: firstLine.replace(/ imported from .*$/, '') }
  }
  // Node's own syntax error: the stack starts with `<file>:<line>`.
  const head = typeof thrown?.stack === 'string' ? /^((?:file:\/\/)?\/[^\n]*?):(\d+)\n/.exec(thrown.stack) : undefined
  if (head) return { file: head[1], line: Number(head[2]), name, message: firstLine }
  // Anything else: the first frame of the plugin's own.
  const frame = pluginFrames(thrown?.stack)?.map(line => FRAME.exec(line)).find(match => match && !/[\\/]node_modules[\\/]/.test(match[1]))
  if (frame) return { file: frame[1], line: Number(frame[2]), column: Number(frame[3]), name, message: firstLine }
  return { name, message: firstLine }
}
