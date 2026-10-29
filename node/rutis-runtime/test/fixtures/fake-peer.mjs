// A host for the bus tests, in this process (platform cleanup §7.5: every
// emission goes to the host): it keeps the listener table the runtime
// registers (`events.on` / `events.off`) and dispatches `events.dispatch`
// on it by calling the runtime back (`events.call`, a Bus's `call`), as
// the Rust host does; ./fake-host.mjs is the one over a socket, and
// bun/rutis-bun/test/fixtures/fake-host.ts the Bun runtime's. It is a Bus
// `peer`. Only what the tests need: one runtime, no listeners of its own,
// `own` always true.

const isResult = value => value !== undefined && value !== null && value !== false

// `answer(method, args)`: what the host answers to anything else.
export function fakePeer(answer = () => undefined) {
  let callBack = () => { throw new Error('the fake host is not connected to a runtime') }
  let dispatchId = 0
  const pending = new Set()

  const host = {
    entries: [], // { id, name, row, plugin, kind, prepend, global }
    // Every message the runtime sent: [method, args].
    sent: [],
    // What emit listeners failed with, as the host is told (§5.6).
    failures: [],
    // The scope chain of each row, for descriptors (row -> scope).
    scopes: new Map(),
    // `bus`: the Bus whose `call` the host calls back.
    connect(bus) { callBack = fields => bus.call(fields); return host },
    // Every dispatch and emit listener call the host made so far has settled.
    async settled() { while (pending.size) await Promise.allSettled([...pending]) },
    call: (_target, method, args) => control(method, args, true),
    callAsync: (_target, method, args) => {
      try { return Promise.resolve(control(method, args, false)) } catch (error) { return Promise.reject(error) }
    },
    callAnswered: (target, method, args, answered) => {
      host.callAsync(target, method, args).then(value => answered({ ok: true, value }), value => answered({ ok: false, value }))
    },
    callFunctionAsync: (fn, args) => Promise.resolve().then(() => fn(...args)),
  }

  function control(method, args, sync) {
    host.sent.push([method, args])
    const [fields] = args ?? []
    switch (method) {
      case 'events.on':
        if (fields.prepend) host.entries.unshift(fields); else host.entries.push(fields)
        return undefined
      case 'events.off':
        host.entries = host.entries.filter(entry => !fields.includes(entry.id))
        return undefined
      case 'events.dispatch': {
        if (sync) return bail(fields)
        const running = dispatch(fields)
        const done = running.then(() => {}, () => {}).finally(() => { pending.delete(done) })
        pending.add(done)
        return running
      }
      default: return answer(method, args)
    }
  }

  const one = (entry, fields, id, next) =>
    callBack({ listener: entry.id, dispatch: id, own: true, name: fields.name, mode: fields.mode, args: fields.args ?? [], ...(next ? { next } : {}) })

  const describe = entry => ({ name: entry.name, prefix: false, plugin: entry.plugin, row: entry.row, runtime: 'node', node: null, scope: host.scopes.get(entry.row) ?? [], global: false, listener: entry.id })

  // The filter (§6): one selector call with the non-global candidates.
  function select(entries, answers) {
    const filtered = entries.filter(entry => !entry.global)
    const dropped = new Set(filtered.filter((_, i) => !answers[i]))
    return entries.filter(entry => !dropped.has(entry))
  }

  async function candidates(fields) {
    const entries = host.entries.filter(entry => entry.name === fields.name)
    if (!fields.filter) return entries
    const answers = await host.callFunctionAsync(fields.filter, [entries.filter(entry => !entry.global).map(describe)])
    return select(entries, answers)
  }

  // The host's dispatch (§5), on the runtime's entries only.
  async function dispatch(fields) {
    const id = `d${++dispatchId}`
    const entries = await candidates(fields)
    const call = (entry, next) => Promise.resolve().then(() => one(entry, fields, id, next))
    switch (fields.mode) {
      case 'emit':
        for (const entry of entries) {
          const running = call(entry).catch(error => { host.failures.push(error) }).finally(() => { pending.delete(running) })
          pending.add(running)
        }
        return undefined
      case 'parallel': {
        const settled = await Promise.allSettled(entries.map(entry => call(entry)))
        const errors = settled.filter(result => result.status === 'rejected').map(result => result.reason)
        if (errors.length) throw new AggregateError(errors, `${errors.length} listeners of ${fields.name} failed`)
        return undefined
      }
      case 'serial':
        for (const entry of entries) {
          const result = await call(entry)
          if (isResult(result)) return result
        }
        return undefined
      case 'waterfall': {
        const from = index => {
          if (index >= entries.length) return fields.terminal ? host.callFunctionAsync(fields.terminal, fields.args ?? []) : undefined
          return call(entries[index], () => from(index + 1))
        }
        return from(0)
      }
      default: throw new Error(`no mode ${fields.mode}`)
    }
  }

  // bail is a synchronous call (§5.4): the selector is called in it too.
  function bail(fields) {
    const id = `d${++dispatchId}`
    let entries = host.entries.filter(entry => entry.name === fields.name)
    if (fields.filter) entries = select(entries, fields.filter(entries.filter(entry => !entry.global).map(describe)))
    for (const entry of entries) {
      const result = one(entry, fields, id)
      if (isResult(result)) return result
    }
    return undefined
  }

  return host
}
