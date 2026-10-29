// A host for the bus tests, in this process (platform cleanup §7.5: every
// emission goes to the host): it keeps the listener table the runtime
// registers (`events.on` / `events.off`) and dispatches `events.dispatch`
// on it by calling the runtime back (`events.call`), as the Rust host
// does; see node/rutis-runtime/test/fixtures/fake-host.mjs for the one
// over a socket. It is both a Bus `peer` and a client for
// `Runtime.attach`. Only what the tests need: one runtime, no listeners
// of its own, `own` always true.

type Fields = Record<string, any>
type Entry = { id: string, name: string, row: string | null, plugin: string, kind: string, prepend: boolean, global: boolean }

const isResult = (value: unknown) => value !== undefined && value !== null && value !== false

export function fakeHost(answer: (method: string, args: any[]) => unknown = () => undefined) {
  let callBack: (fields: Fields) => unknown = () => { throw new Error('the fake host is not connected to a runtime') }
  let dispatchId = 0
  const pending = new Set<Promise<unknown>>()

  const host = {
    entries: [] as Entry[],
    // Every message the runtime sent: [method, args].
    sent: [] as [string, any[]][],
    // What emit listeners failed with, as the host is told (§5.6).
    failures: [] as unknown[],
    // `call(fields)`: `events.call` on the runtime (a Bus's `call`, or the
    // Runtime's control method).
    connect(call: (fields: Fields) => unknown) { callBack = call; return host },
    // The host withdrew `row`'s listeners itself (its unload, or a failed
    // load), as the Rust host does.
    forget(row: string) { host.entries = host.entries.filter(entry => entry.row !== row) },
    // Every dispatch and emit listener call the host made so far has settled.
    async settled() { while (pending.size) await Promise.allSettled([...pending]) },
    call: (_target: string, method: string, args: any[]) => control(method, args, true),
    callAsync: (_target: string, method: string, args: any[]) => {
      try { return Promise.resolve(control(method, args, false)) } catch (error) { return Promise.reject(error) }
    },
    callAnswered: (target: string, method: string, args: any[], answered: (reply: { ok: boolean, value: unknown }) => void) => {
      host.callAsync(target, method, args).then(value => answered({ ok: true, value }), value => answered({ ok: false, value }))
    },
    callFunctionAsync: (fn: (...values: unknown[]) => unknown, args: unknown[]) => Promise.resolve().then(() => fn(...args)),
  }

  function control(method: string, args: any[], sync: boolean): unknown {
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
        const done: Promise<unknown> = running.then(() => {}, () => {}).finally(() => { pending.delete(done) })
        pending.add(done)
        return running
      }
      default: return answer(method, args)
    }
  }

  const one = (entry: Entry, fields: Fields, id: string, next?: () => unknown) =>
    callBack({ listener: entry.id, dispatch: id, own: true, name: fields.name, mode: fields.mode, args: fields.args ?? [], ...(next ? { next } : {}) })

  async function candidates(fields: Fields) {
    let entries = host.entries.filter(entry => entry.name === fields.name)
    if (fields.filter) {
      const filtered = entries.filter(entry => !entry.global)
      const answers = await host.callFunctionAsync(fields.filter, [filtered.map(entry => ({ name: entry.name, prefix: false, plugin: entry.plugin, row: entry.row, runtime: 'bun', node: null, scope: [], global: false, listener: entry.id }))]) as boolean[]
      const dropped = new Set(filtered.filter((_, i) => !answers[i]))
      entries = entries.filter(entry => !dropped.has(entry))
    }
    return entries
  }

  // The host's dispatch (§5), on the runtime's entries only.
  async function dispatch(fields: Fields) {
    const id = `d${++dispatchId}`
    const entries = await candidates(fields)
    const call = (entry: Entry, next?: () => unknown) => Promise.resolve().then(() => one(entry, fields, id, next))
    switch (fields.mode) {
      case 'emit':
        for (const entry of entries) {
          const running: Promise<unknown> = call(entry).catch(error => { host.failures.push(error) }).finally(() => { pending.delete(running) })
          pending.add(running)
        }
        return undefined
      case 'parallel': {
        const settled = await Promise.allSettled(entries.map(entry => call(entry)))
        const errors = settled.filter(result => result.status === 'rejected').map(result => (result as PromiseRejectedResult).reason)
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
        const from = (index: number): unknown => {
          if (index >= entries.length) return fields.terminal ? host.callFunctionAsync(fields.terminal, fields.args ?? []) : undefined
          return call(entries[index], () => from(index + 1))
        }
        return from(0)
      }
      default: throw new Error(`no mode ${fields.mode}`)
    }
  }

  // bail is a synchronous call (§5.4); no filter in these tests.
  function bail(fields: Fields) {
    const id = `d${++dispatchId}`
    for (const entry of host.entries.filter(entry => entry.name === fields.name)) {
      const result = one(entry, fields, id)
      if (isResult(result)) return result
    }
    return undefined
  }

  return host
}
