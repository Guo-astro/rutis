// A host for one runtime, in JavaScript, for the runtimes' event tests
// (#464): it speaks the endpoint format over a Unix socket, keeps the
// listener table the runtime registers (`events.on` / `events.off`),
// pushes `events.elsewhere`, and dispatches `events.dispatch` on that table
// by calling the runtime back (`events.call`), as the Rust host does. It
// provides `events-log` (sync `add`) to the rows. Only what the tests need:
// one runtime, no listeners of its own (a test adds "elsewhere" names).
import { createServer } from 'node:net'
import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { frame } from '../../src/channel/unix.mjs'
import { decode, encode } from '../../src/codec.mjs'
import { RECORD, Session } from '../../src/session.mjs'

const isResult = value => value !== undefined && value !== null && value !== false

// `command`: the runtime's argv (its channel, `--id rt --peer host`,
// inserted after the program); `hello`: a raw hello to send
// instead of the session's (to be refused).
export async function fakeHost(command, { hello, env } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'rutis-fake-host-'))
  const path = join(directory, 'host.sock')
  const server = createServer()
  await new Promise(resolve => server.listen(path, resolve))
  const [program, script, ...rest] = command
  const child = spawn(program, [script, path, '--id', 'rt', '--peer', 'host', ...rest], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, ...env } })
  let stderr = ''
  child.stderr.on('data', chunk => { stderr += chunk })
  // What the runtime writes to stdout, kept for the output tests (#678).
  let stdout = ''
  child.stdout.on('data', chunk => { stdout += chunk })
  const exited = new Promise(resolve => child.once('exit', code => resolve(code)))
  const socket = await Promise.race([
    new Promise(resolve => server.once('connection', resolve)),
    exited.then(code => { throw new Error(`the runtime exited (${code}) before connecting: ${stderr}`) }),
  ])
  server.close()

  const host = {
    entries: [], // { id, name, row, plugin, kind, prepend, global }
    dispatches: [],
    // Names whose `events.on` the host refuses (BindingError).
    refused: new Set(),
    log: [],
    services: new Map(), // id -> handle
    get stderr() { return stderr },
    get stdout() { return stdout },
    exited,
    child,
  }
  let dispatchId = 0
  // The rows loaded, by key: their id and scope chain (rows.load's seventh
  // argument), for descriptors.
  const rows = new Map()
  const placed = row => rows.get(row) ?? { id: row, scope: [] }
  const control = (method, args) => {
    const [fields] = args ?? []
    switch (method) {
      case 'events.on':
        if (host.refused.has(fields.name)) throw Object.assign(new Error(`listening to ${fields.name} is refused here`), { name: 'BindingError' })
        if (fields.prepend) host.entries.unshift(fields); else host.entries.push(fields)
        return undefined
      case 'events.off':
        host.entries = host.entries.filter(entry => !fields.includes(entry.id))
        return undefined
      case 'events.dispatch': return dispatch(fields)
      case 'service': {
        const [id, handle] = args
        host.services.set(id, handle)
        return undefined
      }
      case 'rows.ended': return undefined
      default: throw new Error(`the fake host has no ${method}`)
    }
  }
  const call = (entry, fields) => session.invokeAsync('', 'events.call', [{ [RECORD]: true, listener: entry.id, dispatch: fields.id, own: true, name: fields.name, mode: fields.mode, args: fields.args ?? [], ...(fields.next ? { next: fields.next } : {}) }])
  // The host's dispatch (§5), on the runtime's entries only.
  async function dispatch(fields) {
    const id = `d${++dispatchId}`
    host.dispatches.push({ ...fields, id })
    let entries = host.entries.filter(entry => entry.name === fields.name)
    if (fields.filter) {
      const candidates = entries.filter(entry => !entry.global)
      // The runtime's own entries carry their id (`listener`), as the
      // Rust host sends them.
      const answers = await session.callFunctionAsync(fields.filter, [candidates.map(entry => ({ name: entry.name, prefix: false, plugin: entry.plugin, row: placed(entry.row).id, runtime: 'rt', node: null, scope: placed(entry.row).scope, global: false, listener: entry.id }))])
      const dropped = new Set(candidates.filter((_, i) => !answers[i]))
      entries = entries.filter(entry => !dropped.has(entry))
    }
    const one = (entry, more = {}) => call(entry, { ...fields, id, ...more })
    switch (fields.mode) {
      case 'emit': for (const entry of entries) one(entry).catch(() => {}); return undefined
      case 'parallel': {
        const settled = await Promise.allSettled(entries.map(entry => one(entry)))
        const errors = settled.filter(result => result.status === 'rejected').map(result => result.reason)
        if (errors.length) throw Object.assign(new AggregateError(errors, `${errors.length} listeners of ${fields.name} failed`), { name: 'AggregateError' })
        return undefined
      }
      case 'serial': case 'bail':
        for (const entry of entries) {
          const result = await one(entry)
          if (isResult(result)) return result
        }
        return undefined
      case 'waterfall': {
        const from = index => {
          if (index >= entries.length) return fields.terminal ? session.callFunctionAsync(fields.terminal, fields.args ?? []) : undefined
          return one(entries[index], { next: () => from(index + 1) })
        }
        return from(0)
      }
      default: throw new Error(`no mode ${fields.mode}`)
    }
  }
  const session = new Session({
    send: text => channel.send(text),
    pump: () => { throw new Error('the fake host makes no synchronous calls') },
    dispatch: (target, method, args) => {
      if (target === '') return control(method, args)
      if (target === 'host:events-log' && method === 'add') { host.log.push(args[0]); return undefined }
      throw new Error(`the fake host has no ${target}.${method}`)
    },
    endpoint: { local: 'host' },
  })
  const channel = frame(socket, {
    message: text => session.receive(decode(text)),
    closed: () => session.close(new Error('the runtime disconnected')),
  })
  if (hello) channel.send(encode(hello))
  else session.start()
  host.ready = hello ? Promise.resolve() : session.ready
  host.session = session
  host.call = (method, args) => {
    if (method === 'rows.load') {
      const [key, , , , , , place] = args
      rows.set(key, { id: place?.id ?? key, scope: place?.scope ?? [] })
    }
    return session.invokeAsync('', method, args)
  }
  host.elsewhere = change => session.invokeAsync('', 'events.elsewhere', [change])
  host.end = async () => {
    channel.end()
    const code = await exited
    rmSync(directory, { recursive: true, force: true })
    return code
  }
  return host
}
