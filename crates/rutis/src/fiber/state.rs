//! A fiber's listener state in one word, the calls an unload waits for
//! (#1125), and the object a listener call reaches them through (#1126).

use std::sync::atomic::Ordering;

use crate::sync::{AtomicU64, AtomicUsize};

/// [`OwnerState::live_generation`] while no listener of the generation is
/// live: the fiber is not loading or active.
pub(crate) const NOT_LIVE: u64 = u64::MAX;

/// The word is alive (`FiberInner::alive` is true).
const ALIVE: u64 = 1 << 0;
/// The word is closing (`FiberInner::closing` is true).
const CLOSING: u64 = 1 << 1;
/// A subtree shutdown has committed this fiber's permanent marker
/// (`FiberInner::lineage_closed` is true, #1025).
const LINEAGE_CLOSED: u64 = 1 << 2;
/// Bits 3 and up: [`NOT_LIVE`] as 0, any other generation as itself + 1.
const GENERATION_SHIFT: u32 = 3;

/// Everything a listener call asks about its owner, in one atomic word.
///
/// # Why one word, and why `SeqCst`
///
/// A call counts itself on the owner so the owner's unload waits for it
/// ([`EventFlights`]), and then asks whether it may run. The two are separate
/// atomics, so the order between them is the proof of the drain rule: a call
/// the unload's wait did not see must see the closed state that the unload
/// published before waiting, or a callback could start after the unload
/// passed (and run against a fiber that is gone).
///
/// That order is the sequential consistency of the four operations:
///
/// - the call adds to the count (`fetch_add(1, SeqCst)`) and then loads this
///   word ([`admits`](Self::admits), `load(SeqCst)`);
/// - the unload writes this word (`fetch_and`/`fetch_or(…, SeqCst)`) and then
///   reads the count (`load(SeqCst)`).
///
/// All four are in the one total order of `SeqCst` operations, so a call whose
/// add the wait did not see necessarily reads a word written after the wait's
/// own write: it cannot read "live" (#1125). The call's read is one load;
/// `Acquire` and `SeqCst` loads are the same instruction on x86-64 and
/// aarch64, and the five loads a call made before (#1025: `alive`, `closing`,
/// `lineage_closed`, `live_generation`, each `SeqCst`) are what it replaces.
///
/// The word is a copy of the four fields `FiberInner` keeps for its other
/// readers (the driver, admission, the shuttle models): every write to one is
/// made together with the write to the word (`FiberInner::stop_driver`,
/// `close_marker`, `lineage_marker`, `publish_live`), and
/// `fiber/marker_tests.rs` compares the two where the fiber is quiescent.
pub(crate) struct OwnerState {
    word: AtomicU64,
}

impl OwnerState {
    /// A fiber's starting state: whether it starts alive and lineage-closed,
    /// and the generation whose listeners are live ([`NOT_LIVE`] for none).
    pub fn new(alive: bool, lineage_closed: bool, live: u64) -> Self {
        let mut word = 0;
        if alive {
            word |= ALIVE;
        }
        if lineage_closed {
            word |= LINEAGE_CLOSED;
        }
        if live != NOT_LIVE {
            word |= encode(live);
        }
        Self {
            word: AtomicU64::new(word),
        }
    }

    /// The driver has exited: `FiberInner::alive` went false.
    pub fn stop(&self) {
        self.word.fetch_and(!ALIVE, Ordering::SeqCst);
    }

    /// `FiberInner::closing` went true.
    pub fn publish_closing(&self) {
        self.word.fetch_or(CLOSING, Ordering::SeqCst);
    }

    /// `FiberInner::lineage_closed` went true (#1025).
    pub fn mark_lineage_closed(&self) {
        self.word.fetch_or(LINEAGE_CLOSED, Ordering::SeqCst);
    }

    /// The generation whose listeners are live, as `FiberInner::live_generation`
    /// is published (#980): under the transition lock, and only rewritten when
    /// it changes, so a call never reads a half-written generation.
    pub fn publish_live(&self, live: u64) {
        let wanted = if live == NOT_LIVE { 0 } else { encode(live) };
        if self.word.load(Ordering::SeqCst) >> GENERATION_SHIFT == wanted >> GENERATION_SHIFT {
            return;
        }
        // Two steps, under the transition lock: between them the word says no
        // generation is live, so a call skips rather than admits a listener of
        // a generation that is on its way out. Listeners of the new generation
        // are registered after the load it belongs to started.
        self.word
            .fetch_and(!(u64::MAX << GENERATION_SHIFT), Ordering::SeqCst);
        if wanted != 0 {
            self.word.fetch_or(wanted, Ordering::SeqCst);
        }
    }

    /// May a listener registered in `generation` be called now? One load, the
    /// same word every call reads.
    pub fn admits(&self, generation: u64) -> bool {
        let word = self.word.load(Ordering::SeqCst);
        let open = word & (ALIVE | CLOSING | LINEAGE_CLOSED) == ALIVE;
        let live = word >> GENERATION_SHIFT;
        open && live != 0 && live == generation.wrapping_add(1)
    }

    /// The word's own view of the marker `FiberInner::alive` keeps: the debug
    /// check in `fiber.rs` holds the two together.
    #[cfg(all(test, debug_assertions))]
    pub fn alive(&self) -> bool {
        self.word.load(Ordering::SeqCst) & ALIVE != 0
    }

    /// [`alive`](Self::alive) for `FiberInner::closing`.
    #[cfg(all(test, debug_assertions))]
    pub fn closing(&self) -> bool {
        self.word.load(Ordering::SeqCst) & CLOSING != 0
    }

    /// [`alive`](Self::alive) for `FiberInner::lineage_closed`.
    #[cfg(all(test, debug_assertions))]
    pub fn lineage_closed(&self) -> bool {
        self.word.load(Ordering::SeqCst) & LINEAGE_CLOSED != 0
    }

    /// [`alive`](Self::alive) for `FiberInner::live_generation`.
    #[cfg(all(test, debug_assertions))]
    pub fn live_generation(&self) -> u64 {
        match self.word.load(Ordering::SeqCst) >> GENERATION_SHIFT {
            0 => NOT_LIVE,
            encoded => encoded - 1,
        }
    }
}

fn encode(live: u64) -> u64 {
    debug_assert!(
        live < (u64::MAX >> GENERATION_SHIFT),
        "generation {live} does not fit the state word"
    );
    (live.wrapping_add(1) << GENERATION_SHIFT) & (u64::MAX << GENERATION_SHIFT)
}

/// The calls started on one fiber, which its unload waits for (protocol spec
/// §7.1).
///
/// An unload waits until the count is zero. The count is written by every
/// call (`begin`/`finish`, once per dispatch and owner, #1125) and read only
/// by an unload, which is why reaching zero is the only place a waiter is
/// woken: with no unload waiting, a finish is one atomic subtraction.
pub(crate) struct EventFlights {
    count: AtomicUsize,
    /// How many unloads wait for the count to reach zero.
    waiting: AtomicUsize,
    /// Wakes them; only notified on reaching zero with a waiter.
    wake: tokio::sync::Notify,
}

impl EventFlights {
    pub fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            waiting: AtomicUsize::new(0),
            wake: tokio::sync::Notify::new(),
        }
    }

    /// Count one more call. The state word is read after this
    /// ([`OwnerState`]'s note).
    pub fn begin(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    /// One call ended.
    pub fn finish(&self) {
        let left = self.count.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(left > 0, "a call finished that never began");
        if left == 1 && self.waiting.load(Ordering::SeqCst) != 0 {
            self.wake.notify_waiters();
        }
    }

    /// Calls counted now.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Wait until no call is counted.
    pub async fn wait_zero(&self) {
        if self.count() == 0 {
            return;
        }
        self.waiting.fetch_add(1, Ordering::SeqCst);
        // A wait that ends at its deadline is dropped mid-await: the waiter
        // count goes back with it, so no finish notifies nobody.
        let _waiting = Waiting(&self.waiting);
        while self.count() != 0 {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            // Registered before the second look: a finish between the two is
            // either seen by the look or wakes this waiter.
            notified.as_mut().enable();
            if self.count() == 0 {
                break;
            }
            notified.await;
        }
    }
}

/// Everything a listener call asks of the fiber that registered it, in one
/// object of its own (#1126).
///
/// An entry holds an `Arc<Owner>`, not a `Weak` to the fiber: a call reaches
/// the state word and the count it is added to without a `Weak::upgrade` (a
/// read-modify-write on the fiber's strong count for every call), and without
/// touching the fiber itself, whose other fields a call never needs. The three
/// fields a call uses are the first three of the struct, so they land in one
/// cache line.
///
/// The fiber holds the other `Arc<Owner>`; nothing holds the fiber from here,
/// so the two do not form a cycle.
///
/// `repr(align(64))`: the three fields a call reads are the first three of the
/// struct, and aligning the object makes them the first cache line of it
/// rather than whichever line a 16-byte-aligned `Arc` header left them in.
/// The object is 64 bytes either way; the alignment costs one 64-byte block
/// per fiber.
#[repr(align(64))]
pub(crate) struct Owner {
    /// Alive, closing, lineage closed, and the generation whose listeners are
    /// live, in one word ([`OwnerState`]).
    pub state: OwnerState,
    /// The dispatch that last counted a flight on this owner (#1125).
    pub counted_by: AtomicU64,
    /// The calls started on this owner and not finished yet, which its unload
    /// waits for ([`EventFlights`]).
    pub flights: EventFlights,
}

impl Owner {
    pub fn new(alive: bool, lineage_closed: bool, live: u64) -> Self {
        Self {
            state: OwnerState::new(alive, lineage_closed, live),
            counted_by: AtomicU64::new(0),
            flights: EventFlights::new(),
        }
    }

    pub fn begin_event(&self) {
        self.flights.begin();
    }

    pub fn finish_event(&self) {
        self.flights.finish();
    }
}

/// One waiter registered on [`EventFlights::waiting`].
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_round_trips() {
        let state = OwnerState::new(true, false, 0);
        assert!(state.admits(0));
        assert!(!state.admits(1));
        state.publish_live(3);
        assert!(!state.admits(0));
        assert!(state.admits(3));
        state.publish_live(NOT_LIVE);
        assert!(!state.admits(3));
    }

    #[test]
    fn closing_and_lineage_close_every_generation() {
        let state = OwnerState::new(true, false, 7);
        assert!(state.admits(7));
        state.publish_closing();
        assert!(!state.admits(7));
        let state = OwnerState::new(true, false, 7);
        state.mark_lineage_closed();
        assert!(!state.admits(7));
        let state = OwnerState::new(true, false, 7);
        state.stop();
        assert!(!state.admits(7));
    }

    #[test]
    fn a_closed_fiber_starts_closed() {
        let state = OwnerState::new(false, true, NOT_LIVE);
        assert!(!state.admits(0));
    }

    #[test]
    fn owner_counts_and_state_are_one_object() {
        let owner = Owner::new(true, false, 4);
        assert!(owner.state.admits(4));
        owner.begin_event();
        owner.counted_by.store(7, Ordering::Relaxed);
        assert_eq!(owner.flights.count(), 1);
        assert_eq!(owner.counted_by.load(Ordering::Relaxed), 7);
        owner.finish_event();
        assert_eq!(owner.flights.count(), 0);
        owner.state.publish_closing();
        assert!(!owner.state.admits(4));
    }
}
