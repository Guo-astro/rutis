// Dialing out to a rutis access point, and dialing again: the rules every
// runtime follows (docs/design-remote-runtime-dial-2026-10-14.md §六), as
// rutis-host's `rutis-host/dial` row does (`connect/dial.rs`).
//
// A failure is retryable (backoff from 0.5 s, doubling up to 30 s, reset by
// a session that lasted 60 s), an authentication failure (again every 30 s:
// 401 / 403 at the upgrade, a server certificate this side refuses, the
// access point rejecting this id with close code 4003 after the hello, or
// the far end greeting as another endpoint than `--peer`), or incompatible
// (stop: retrying cannot fix it). Every session is new: the rows of one go
// when it ends, and the next connection is mounted afresh. The same rules
// as the Node runtime's (its `dialing.mjs`), in this runtime's own code.

export const RETRYABLE = 'retryable', AUTH_REJECTED = 'auth-rejected', INCOMPATIBLE = 'incompatible'
export type Category = typeof RETRYABLE | typeof AUTH_REJECTED | typeof INCOMPATIBLE
// The close code of an access point rejecting the id this side greeted with.
export const REJECTED = 4003

// Reconnection, in ms: the defaults of `rutis-host/dial`. `jitter`: ± this
// fraction of each wait, at random; `stable`: a session lasting this long
// resets the backoff; `rejected`: the wait after an authentication
// failure; `handshake`: how long the access point may take to greet.
export const RETRY = Object.freeze({ initial: 500, max: 30_000, jitter: 0.2, stable: 60_000, rejected: 30_000, handshake: 10_000 })
export type Retry = { -readonly [K in keyof typeof RETRY]: number }

// The defaults, unless tests shorten them with
// RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>.
export function retryFrom(env: Record<string, string | undefined> = process.env): Retry {
  if (!env.RUTIS_RETRY) return { ...RETRY }
  const [initial, max, stable, rejected] = env.RUTIS_RETRY.split(',').map(Number)
  return { ...RETRY, initial, max, stable, rejected }
}

// Exponential backoff with jitter.
export class Backoff {
  #retry: Retry; #next: number; #random: () => number
  constructor(retry: Retry = RETRY, random = Math.random) { this.#retry = retry; this.#next = retry.initial; this.#random = random }
  reset() { this.#next = this.#retry.initial }
  // The next wait: the current step ± jitter; the step doubles up to the
  // maximum.
  next(): number {
    const base = this.#next
    this.#next = Math.min(this.#next * 2, this.#retry.max)
    return base * (1 + this.#retry.jitter * (2 * this.#random() - 1))
  }
}

// What a session's end means for retrying: the access point rejecting this
// id (4003), or a greeting this side refused (its `category`: the far end
// greeting as another endpoint than expected is an authentication failure,
// one it cannot speak incompatible); anything else, being replaced (4002)
// too, is retryable.
export function sessionFailure(code: number | undefined, refusal?: { category?: string }): Category {
  if (code === REJECTED) return AUTH_REJECTED
  if (refusal?.category === AUTH_REJECTED || refusal?.category === INCOMPATIBLE) return refusal.category as Category
  return RETRYABLE
}

// A wait in ms, for a state line: 0.5s, 1s, 30s.
export const seconds = (ms: number) => `${Number((ms / 1000).toFixed(1))}s`

export const report = (line: string) => process.stderr.write(`rutis: ${line}\n`)

const sleep = (ms: number, stopped: Promise<void>) => new Promise<void>(resolve => {
  const timer = setTimeout(resolve, ms)
  stopped.then(() => { clearTimeout(timer); resolve() })
})

// Dial `address`, serve the session, and dial again by the rules, until
// `stopped` resolves (0) or the access point is incompatible (1).
//
// `dial()` resolves to a connection or rejects with an error whose
// `category` says what it means. `start(connection)` returns a started
// session: `greeted` (resolves once the access point greeted as expected;
// rejects with a categorized error otherwise), `ended` (resolves with
// { reason, code } once the connection closed), `close(reason)`, `end()`
// (the rows go; resolves with { reason, code } once the connection closed
// and the rows are gone) and `stop()` (the rows go, then the connection
// closes). `say` gets one line per state change.
export interface Ended { reason?: string, code?: number }
export interface Session {
  greeted: Promise<void>
  ended: Promise<Ended>
  close(reason: string): void
  end(): Promise<Ended>
  stop(): Promise<void>
}
export async function keepDialing<C extends { close?(reason: string): void }>({ address, dial, start, stopped, retry = retryFrom(), say = report }: {
  address: string
  dial(): Promise<C>
  start(connection: C): Session
  stopped: Promise<void>
  retry?: Retry
  say?: (line: string) => void
}): Promise<number> {
  const backoff = new Backoff(retry)
  const STOP = Symbol('stop')
  const orStop = <T>(promise: Promise<T>) => Promise.race([promise, stopped.then(() => STOP as typeof STOP)])
  let stopping = false
  stopped.then(() => { stopping = true })
  for (;;) {
    const attempt = dial()
    const dialed = await orStop(attempt.then(connection => ({ connection, error: undefined as any }), error => ({ connection: undefined, error })))
    if (dialed === STOP) {
      // A dial that connects after the runtime stopped is closed.
      attempt.then(connection => connection.close?.('the runtime stopped'), () => {})
      return 0
    }
    let failure: Category, reason: string, lost: string
    if (dialed.error) {
      failure = dialed.error.category ?? RETRYABLE
      reason = dialed.error.message
      lost = 'cannot connect'
    } else {
      const session = start(dialed.connection!)
      let timer: ReturnType<typeof setTimeout> | undefined
      type Greeted = { ok?: boolean, refusal?: Error & { category?: string }, late?: boolean }
      const greeted = await orStop(Promise.race<Greeted>([
        session.greeted.then(() => ({ ok: true }), refusal => ({ refusal })),
        session.ended.then(() => ({})),
        new Promise<Greeted>(resolve => { timer = setTimeout(() => resolve({ late: true }), retry.handshake) }),
      ]))
      clearTimeout(timer)
      if (greeted === STOP) { await session.stop(); return 0 }
      let refusal: (Error & { category?: string }) | undefined = greeted.refusal
      if (greeted.late) {
        refusal = new Error('the access point did not greet in time')
        session.close(refusal.message)
      } else if (refusal) {
        session.close(refusal.message)
      } else if (greeted.ok) {
        say(`connected to ${address}`)
        const since = Date.now()
        if (await orStop(session.ended) === STOP) { await session.stop(); return 0 }
        if (Date.now() - since >= retry.stable) backoff.reset()
      }
      const ended = await session.end()
      failure = sessionFailure(ended.code, refusal)
      reason = refusal?.message ?? ended.reason ?? `the access point closed the connection (${ended.code})`
      lost = 'connection lost'
    }
    let wait: number
    if (failure === INCOMPATIBLE) {
      say(`refused: incompatible (${reason}); stopping`)
      return 1
    } else if (failure === AUTH_REJECTED) {
      wait = retry.rejected
      say(`refused: auth-rejected (${reason}); retrying in ${seconds(wait)}`)
    } else {
      wait = backoff.next()
      say(`${lost} (${reason}); retrying in ${seconds(wait)}`)
    }
    await sleep(wait, stopped)
    if (stopping) return 0
  }
}
