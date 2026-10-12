// Wait helpers for the tests (quality standard Q7.1, #231), and the hang
// guard of every test: bunfig.toml preloads this file. Bun's default is 5 s
// a test, and Bun 1.4 does not read `[test] timeout` from bunfig.toml, so
// the default is set here.
//
// A timeout only guards against a hang (HANG_GUARD_MS); it never stands in
// for an event. Wait for what the test is about: an event, a message, a
// state. Poll (`eventually`) only where nothing can be awaited. See
// docs/design-test-clocks-2026-10-11.md §3.
import { setDefaultTimeout } from 'bun:test'

/** How long a wait lasts before the test fails as hung. Never what a test measures. */
export const HANG_GUARD_MS = 10_000

// A whole test's bound: longer than one wait, so that a hung wait reports
// itself, naming what it waited for, before the test is cut off.
setDefaultTimeout(HANG_GUARD_MS * 2)

/** How often `eventually` looks again. */
const POLL_MS = 10

/** `promise`, unless it takes longer than HANG_GUARD_MS: then it rejects, naming `what`. */
export async function within<T>(what: string, promise: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const guard = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`timed out (${HANG_GUARD_MS} ms): ${what}`)), HANG_GUARD_MS)
  })
  try {
    return await Promise.race([promise, guard])
  } finally {
    clearTimeout(timer)
  }
}

/**
 * The first truthy result of `check`, looked at every 10 ms, within
 * HANG_GUARD_MS; else it rejects, naming `what`.
 */
export async function eventually<T>(what: string, check: () => T | undefined | null | false): Promise<T> {
  const deadline = Date.now() + HANG_GUARD_MS
  for (;;) {
    const value = check()
    if (value) return value
    if (Date.now() >= deadline) throw new Error(`timed out (${HANG_GUARD_MS} ms): ${what}`)
    await Bun.sleep(POLL_MS)
  }
}
