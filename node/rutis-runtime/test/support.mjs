// Wait helpers for the tests (quality standard Q7.1, #231).
//
// A timeout here only guards against a hang (HANG_GUARD_MS); it never
// stands in for an event. Wait for what the test is about: an event, a
// message, a state. Poll (`eventually`) only where nothing can be awaited.
// See docs/design-test-clocks-2026-10-11.md §3.

/** How long a wait lasts before the test fails as hung. Never what a test measures. */
export const HANG_GUARD_MS = 10_000

/** How often `eventually` looks again. */
const POLL_MS = 10

/** `promise`, unless it takes longer than HANG_GUARD_MS: then it rejects, naming `what`. */
export async function within(what, promise) {
  let timer
  const guard = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`timed out (${HANG_GUARD_MS} ms): ${what}`)), HANG_GUARD_MS)
  })
  try {
    return await Promise.race([promise, guard])
  } finally {
    clearTimeout(timer)
  }
}

/**
 * The first truthy result of `check` (which may be async), looked at every
 * 10 ms, within HANG_GUARD_MS; else it rejects, naming `what` and, when
 * given, `describe()` (the state at the time).
 */
export async function eventually(what, check, describe) {
  const deadline = Date.now() + HANG_GUARD_MS
  for (;;) {
    const value = await check()
    if (value) return value
    if (Date.now() >= deadline) {
      throw new Error(`timed out (${HANG_GUARD_MS} ms): ${what}${describe ? `\nstate: ${describe()}` : ''}`)
    }
    await new Promise(resolve => setTimeout(resolve, POLL_MS))
  }
}
