--------------------------- MODULE CallLifecycle ----------------------------
(***************************************************************************)
(* Calls on one session between the two endpoints of protocol 5: A calls,  *)
(* B runs the method and replies, A may cancel before the reply arrives,   *)
(* the reply grants a reference that A releases, and either side may close *)
(* the session.                                                            *)
(*                                                                         *)
(* The model also answers the questions of #626:                           *)
(*  - What a reader thread's write-back does when its outbound channel is  *)
(*    full. `ReaderWrites = "direct"` is the shape before #1005: the       *)
(*    reader writes on the channel and blocks in `send`. `"queued"` is     *)
(*    today's: the reader hands the frame to the writer thread (an         *)
(*    unbounded queue) and goes on reading; the writer thread is the one   *)
(*    that blocks. The pre-#1005 shape deadlocks: both readers block on a  *)
(*    full channel while the only frames that would free a channel are     *)
(*    behind the other blocked reader.                                     *)
(*  - How a callee resolves a cancel it read (protocol 5, SESS-61).       *)
(*    `CancelTable = "ack"` is the rule: a call still queued is dropped, a *)
(*    running one keeps running, and either way the callee acknowledges    *)
(*    and answers no more; `Replies = "always"` is the mutation that       *)
(*    replies after the acknowledgement. A reply written before the cancel *)
(*    was read is still allowed and arrives first.                         *)
(*  - What a locally abandoned synchronous call leaves behind. `Abandon =  *)
(*    "keep"` keeps the caller's waiter until the reply (today's sync      *)
(*    wait, which has no drop guard); `"drop"` forgets it without a        *)
(*    cancel, and the reply then is an unknown call and ends the session.  *)
(*  - Whether a reference the caller sent as an argument is given back     *)
(*    when the callee drops the call still queued: the callee's reader     *)
(*    drops the arguments, and each import they carried writes `release`   *)
(*    back (the same write-back as a `throw`). `Args` names the calls that *)
(*    carry one; `argLeaks` is the mutation where nothing is written back. *)
(*  - `PinnedTarget`: an admitted call keeps the object it names alive, so *)
(*    a release that arrives before the method runs does not fail it, and *)
(*    a release of an older grant does not take a newer one. `RefCalls`    *)
(*    names the calls that name an object; `noPin` and `releaseDropsAll`   *)
(*    are the two mutations.                                              *)
(*  - Frames still on the channel when the session closes. `AfterClose =   *)
(*    "lost"` drops them with the close; `"read"` keeps them, and both     *)
(*    sides then read them and refuse them (`receive` looks at `closed`    *)
(*    before the frame). `lateFrameActs` is the mutation that applies      *)
(*    such a reply anyway, which settles a call twice.                    *)
(*  - The host's bound on the writer queue (SESS-75): `OutboundLimit =      *)
(*    "frames"` with `Limit` ends the session instead of queueing past it, *)
(*    so a peer that stopped reading ends the session rather than growing *)
(*    the queue.                                                          *)
(*  - The caller's cancelled table and what clears it (#1099, and the      *)
(*    `BoundedTables` of #626). `CancelTable = "ack"` is protocol 5's rule: *)
(*    the callee acknowledges the cancel it read and answers no more, so   *)
(*    the entry goes on the ack, and an ack that crossed a reply (which    *)
(*    cleared the table first) is ignored. `"late"` is the shape before    *)
(*    protocol 5: only a late reply clears an entry, and no conforming     *)
(*    callee answers a call cancelled while queued, so it grows            *)
(*    (`call-gap-cancel-table`). `"bounded"` keeps at most `TableCap`      *)
(*    entries and forgets the oldest, whose replies it then discards       *)
(*    instead of failing them (`SESS-41` relaxed); `"forget"` drops the    *)
(*    entry at once, which the late reply of a running call then turns     *)
(*    into an unknown call (`call-gap-unknown-late`).                      *)
(*                                                                         *)
(* Design: docs/protocol-session.md §5 (call numbers, one settlement),     *)
(* §6 (references), §7 (cancel, a callee resolves it with `ack` or the      *)
(* reply it wrote first), §10 (backlog and `outbound_limit`);              *)
(* docs/formal-trace.md §3.4 (cancel) and §3.5 (errors);                   *)
(* docs/design-formal-verification-2026-11-08.md §3.4; issue #626, #1099.  *)
(* Implementation: crates/rutis-bridge/src/session/rpc.rs —                *)
(* `Calls::admit` (a call number is taken once and only grows),            *)
(* `reply_target` (a reply takes its waiter out; the reply of a cancelled  *)
(* call is discarded and counted in `orphans`), `receive` for              *)
(* `Frame::Cancel` and `Frame::Ack` (a queued call is dropped, a running   *)
(* one's result dropped, and the cancel acknowledged), `Frame::Release`    *)
(* (the export entry goes when its grants reach zero, `release of          *)
(* unknown reference` beyond them) and                                    *)
(* `Frame::Return` (the reader writes back a `throw`/`release`; README     *)
(* "Model"), `Import`'s `Drop` (the arguments and the relayed references   *)
(* write `release` back), `Connection::cancel`, `Connection::close` (the   *)
(* `closed` check that refuses a frame read after a close),                *)
(* `Outbound`/`write_frames` (the writer thread #1005 and `outbound_limit`; *)
(* a send never waits for the far end; the channel's `send` blocks on that *)
(* thread, not on the reader's). README.md next to this file lists the     *)
(* simplifications.                                                       *)
(*                                                                         *)
(* A calls; B admits, runs and answers. The session's two directions are   *)
(* FIFO and delayed arbitrarily long, so a reply may be in flight when a   *)
(* cancel is written and the reply of a cancelled call arrives late. With  *)
(* `Bound`, each direction is a channel of `Cap` frames, fed by the writer *)
(* thread from an unbounded queue; a reader's write-back either goes into  *)
(* the channel and blocks it when full (`"direct"`) or into that queue     *)
(* (`"queued"`). Either side may close the session; the other side learns  *)
(* it when the channel ends.                                               *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Calls,       \* the calls A may start ("c1", "c2"), in no fixed order
    MaxCancels,  \* how often A may cancel in the session (1)
    Closes,      \* either side may close the session
    Mutation,    \* "none", or a deliberate bug (README, "Mutations")
    Bound,       \* FALSE: no channel bound (the model without #626)
    Cap,         \* frames a direction holds when Bound
    ReaderWrites,\* "direct" (before #1005) or "queued" (today)
    Rejects,     \* B may refuse a call it read (a `throw` written by the
                 \* reader thread: bad arguments, too deep)
    Replies,     \* after a cancel: "always" or "never" (SESS-61)
    Sync,        \* the calls A makes synchronously (a subset of Calls)
    Abandon,     \* a locally abandoned synchronous call: "none", "keep",
                 \* "drop"
    Args,        \* the calls whose arguments carry one reference of A's
    RefCalls,    \* the calls that name an object B exports (a pinned target)
    AfterClose,  \* a frame in flight when the session closes: "lost" (it
                 \* goes with the close) or "read" (it is read and refused)
    OutboundLimit,\* "none", or "frames": a host bound on the writer queue
    Limit,       \* frames that may wait for the far end when the bound is on
    CancelTable, \* what clears an entry of the caller's cancelled table:
                 \* "late" (today), "forget", "bounded", "ack"
    TableCap     \* entries the bounded table keeps

ASSUME Calls /= {}
ASSUME MaxCancels \in Nat
ASSUME Closes \in BOOLEAN
ASSUME Mutation \in {"none", "cancelCloses", "lateReplyTwice", "closeLeaks",
                     "reuseId", "abandonLeaks", "lateReplyKeepsGrant",
                     "argLeaks", "noPin", "releaseDropsAll", "lateFrameActs",
                     "limitIgnored", "forgetStrict"}
ASSUME Bound \in BOOLEAN
ASSUME Cap \in Nat
ASSUME ReaderWrites \in {"direct", "queued"}
ASSUME Rejects \in BOOLEAN
ASSUME Replies \in {"always", "never"}
ASSUME Sync \subseteq Calls
ASSUME Abandon \in {"none", "keep", "drop"}
ASSUME Args \subseteq Calls
ASSUME RefCalls \subseteq Calls
ASSUME AfterClose \in {"lost", "read"}
ASSUME OutboundLimit \in {"none", "frames"}
ASSUME OutboundLimit = "frames" => Bound
ASSUME Limit \in Nat
ASSUME CancelTable \in {"late", "forget", "bounded", "ack"}
ASSUME TableCap \in Nat

VARIABLES
    aclosed,     \* A's session is over
    bclosed,     \* B's session is over
    closeCause,  \* ghost: what closed the session: "none" | "a" | "b" |
                 \*   "cancel" | "unknownCall" | "notReading" | "badRelease"
    acall,       \* per call at A: "idle" | "waiting" | "settled" |
                 \*   "cancelled" | "abandoned"
    anext,       \* the number A gives its next call
    aid,         \* per call: the number A gave it (0 before it started)
    aTable,      \* per call: what A's table holds for a reply: "none" |
                 \*   "waiting" | "cancelled" | "abandoned" (a call A no
                 \*   longer places makes a reply an unknown call)
    aedge,       \* ghost: per sync call, the wait-graph hop A recorded
    bcall,       \* per call at B: "none" | "queued" | "running" | "done" |
                 \*   "dropped"
    bseq,        \* per call: the number B read with the call
    bmark,       \* ghost: per sync call, B's mark of a call it admitted
    bcancelled,  \* per call: a cancel reached B while it was running it
    cancels,     \* how often A cancelled
    toA,         \* B -> A channel, FIFO: <<"return"|"throw", call, number>>,
                 \*   <<"ack", call>>, <<"release", count>>
    toB,         \* A -> B channel, FIFO: <<"invoke", call, number>>,
                 \*   <<"cancel", call>>, <<"release", count>>
    aout,        \* A's writer queue (A -> B), unbounded
    bout,        \* B's writer queue (B -> A), unbounded
    ablock,      \* the frame A's reader is blocked writing (<<>> when none)
    bblock,      \* the frame B's reader is blocked writing
    aheld,       \* reference grants A holds
    bgrants,     \* reference grants B made and nobody released yet
    granted,     \* ghost: how often B granted the reference
    aReleased,   \* ghost: grant units A gave back (a release written, or a
                 \* discarded reply's value released at once)
    aCleared,    \* ghost: grant units A's close dropped
    aArgsOut,    \* reference grants A made through the arguments of `Args`
                 \*   and has not been given back
    aArgGiven,   \* ghost: grant units A made through those arguments
    aArgBack,    \* ghost: grant units read back as B's `release`
    bArgsGiven,  \* per call: B let that call's arguments go
    bLost,       \* ghost: calls B dropped for want of their target
    forgotten,   \* calls the bounded table dropped: a reply to one of them is
                 \*   discarded, not an error (the bound's price)
    settlesClosed,\* ghost: `settles` when the session closed
    aheldClosed, \* ghost: `aheld` when the session closed
    settles,     \* per call: how often it was settled at A
    outcome,     \* per call: "none" | "ok" | "error" | "cancel" |
                 \*   "abandon" | "closed"
    settledBy,   \* per call: the call whose reply settled it ("none" before)
    orphaned     \* per call: replies discarded after the call was cancelled
                 \*   or abandoned

vars == <<aclosed, bclosed, closeCause, acall, anext, aid, aTable, aedge,
          bcall, bseq, bmark, bcancelled, cancels, toA, toB, aout, bout,
          ablock, bblock, aheld, bgrants, granted, aReleased, aCleared,
          aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
          settlesClosed, aheldClosed, settles, outcome, settledBy, orphaned>>

Init ==
    /\ aclosed = FALSE
    /\ bclosed = FALSE
    /\ closeCause = "none"
    /\ acall = [c \in Calls |-> "idle"]
    /\ anext = 1
    /\ aid = [c \in Calls |-> 0]
    /\ aTable = [c \in Calls |-> "none"]
    /\ aedge = [c \in Calls |-> FALSE]
    /\ bcall = [c \in Calls |-> "none"]
    /\ bseq = [c \in Calls |-> 0]
    /\ bmark = [c \in Calls |-> FALSE]
    /\ bcancelled = [c \in Calls |-> FALSE]
    /\ cancels = 0
    /\ toA = <<>>
    /\ toB = <<>>
    /\ aout = <<>>
    /\ bout = <<>>
    /\ ablock = <<>>
    /\ bblock = <<>>
    /\ aheld = 0
    /\ bgrants = 0
    /\ granted = 0
    /\ aReleased = 0
    /\ aCleared = 0
    /\ aArgsOut = 0
    /\ aArgGiven = 0
    /\ aArgBack = 0
    /\ bArgsGiven = [c \in Calls |-> FALSE]
    /\ bLost = 0
    /\ forgotten = {}
    /\ settlesClosed = [c \in Calls |-> 0]
    /\ aheldClosed = 0
    /\ settles = [c \in Calls |-> 0]
    /\ outcome = [c \in Calls |-> "none"]
    /\ settledBy = [c \in Calls |-> "none"]
    /\ orphaned = [c \in Calls |-> 0]

(***************************************************************************)
(* The two directions. Unbounded (`~Bound`), a frame goes on the channel   *)
(* as it is made. Bounded, the frame goes into the writer's queue first    *)
(* (the writer thread blocks in the channel's `send`, not the thread that  *)
(* made the frame, #1005). A reader's write-back is the one that can       *)
(* block: before #1005 it went on the channel from the reader thread;      *)
(* today it goes into the writer queue too.                               *)
(*                                                                         *)
(* With a host bound (`OutboundLimit`), a queue holds at most `Limit`      *)
(* frames: the frame that would make it longer is not queued, and the      *)
(* session ends instead (SESS-75, `OverLimitA`/`OverLimitB`).             *)
(***************************************************************************)

\* A push needs room for one more frame. The mutation queues past the bound
\* (`limitIgnored`), which is what a host without the bound would do.
RoomToB == OutboundLimit = "none" \/ Mutation = "limitIgnored"
           \/ Len(aout) < Limit
RoomToA == OutboundLimit = "none" \/ Mutation = "limitIgnored"
           \/ Len(bout) < Limit

SendToB(f) ==
    /\ RoomToB
    /\ IF Bound
       THEN aout' = Append(aout, f) /\ UNCHANGED toB
       ELSE toB' = Append(toB, f) /\ UNCHANGED aout

SendToA(f) ==
    /\ RoomToA
    /\ IF Bound
       THEN bout' = Append(bout, f) /\ UNCHANGED toA
       ELSE toA' = Append(toA, f) /\ UNCHANGED bout

\* A's reader writes to B: straight on the channel and blocked when it is
\* full (before #1005), or handed to the writer thread (today).
ReaderToB(f) ==
    IF ~Bound \/ ReaderWrites = "queued"
    THEN SendToB(f) /\ UNCHANGED ablock
    ELSE IF Len(toB) < Cap
         THEN toB' = Append(toB, f) /\ UNCHANGED aout /\ UNCHANGED ablock
         ELSE ablock' = f /\ UNCHANGED toB /\ UNCHANGED aout

ReaderToA(f) ==
    IF ~Bound \/ ReaderWrites = "queued"
    THEN SendToA(f) /\ UNCHANGED bblock
    ELSE IF Len(toA) < Cap
         THEN toA' = Append(toA, f) /\ UNCHANGED bout /\ UNCHANGED bblock
         ELSE bblock' = f /\ UNCHANGED toA /\ UNCHANGED bout

(***************************************************************************)
(* A: starting calls, reading replies, abandoning, cancelling, releasing,  *)
(* closing.                                                                *)
(***************************************************************************)

\* A closes its side: the calls it still waits for fail, its exports go, and
\* the frames in both directions are lost — or, with `AfterClose = "read"`,
\* stay on the channels to be read and refused (`AReadAfterClose`).
CloseA(cause) ==
    /\ aclosed' = TRUE
    /\ closeCause' = IF closeCause = "none" THEN cause ELSE closeCause
    /\ acall' = [c \in Calls |-> IF acall[c] = "waiting" THEN "settled"
                                      ELSE acall[c]]
    /\ settles' = [c \in Calls |-> IF acall[c] = "waiting" THEN settles[c] + 1
                                         ELSE settles[c]]
    /\ settlesClosed' = settles'
    /\ outcome' = [c \in Calls |-> IF acall[c] = "waiting" THEN "closed"
                                         ELSE outcome[c]]
    /\ aTable' = [c \in Calls |-> "none"]
    /\ aedge' = [c \in Calls |-> FALSE]
    /\ aheld' = 0
    /\ aheldClosed' = 0
    /\ aCleared' = aCleared + aheld
    /\ aArgsOut' = 0
    /\ IF AfterClose = "read"
       THEN UNCHANGED <<toA, toB>>
       ELSE /\ toA' = <<>>
            /\ toB' = <<>>
    /\ aout' = <<>>
    /\ bout' = <<>>
    /\ ablock' = <<>>
    /\ bblock' = <<>>
    /\ UNCHANGED <<bclosed, anext, aid, bcall, bseq, bmark, bcancelled,
                   bgrants, granted, aReleased, aArgGiven, aArgBack, bArgsGiven,
                   bLost, forgotten, settledBy, orphaned>>

AClose ==
    /\ Closes
    /\ ~aclosed
    /\ CloseA("a")
    /\ UNCHANGED <<cancels>>

\* A call takes the next number. The mutation keeps the counter, so a new
\* call can take a number an older call already has. A call that names an
\* object needs a grant of it, and a call whose arguments carry a reference
\* grants it on the way out.
AStart(c) ==
    /\ ~aclosed
    /\ ~bclosed
    /\ acall[c] = "idle"
    /\ (c \in RefCalls => aheld > 0)
    /\ acall' = [acall EXCEPT ![c] = "waiting"]
    /\ aid' = [aid EXCEPT ![c] = anext]
    /\ aTable' = [aTable EXCEPT ![c] = "waiting"]
    /\ aedge' = [aedge EXCEPT ![c] = (c \in Sync)]
    /\ aArgsOut' = IF c \in Args THEN aArgsOut + 1 ELSE aArgsOut
    /\ aArgGiven' = IF c \in Args THEN aArgGiven + 1 ELSE aArgGiven
    /\ SendToB(<<"invoke", c, anext>>)
    /\ anext' = IF Mutation = "reuseId" THEN anext ELSE anext + 1
    /\ UNCHANGED <<aclosed, bclosed, closeCause, bcall, bseq, bmark,
                   bcancelled, cancels, toA, bout, ablock, bblock, aheld,
                   bgrants, granted, aReleased, aCleared, aArgBack, bArgsGiven,
                   bLost, forgotten, settlesClosed, aheldClosed, settles,
                   outcome, settledBy, orphaned>>

\* A reply for a call A still waits for: the call whose number the reply
\* carries is settled once, with the reference a `return` also carries. A
\* `throw` settles it with an error and grants nothing.
ADeliverLive(d) ==
    /\ ~aclosed
    /\ ablock = <<>>
    /\ toA /= <<>>
    /\ LET f == Head(toA)
       IN /\ f[1] \in {"return", "throw"}
          /\ aid[d] = f[3]
          /\ acall[d] = "waiting"
          /\ toA' = Tail(toA)
          /\ acall' = [acall EXCEPT ![d] = "settled"]
          /\ aTable' = [aTable EXCEPT ![d] = "none"]
          /\ aedge' = [aedge EXCEPT ![d] = FALSE]
          /\ settles' = [settles EXCEPT ![d] = @ + 1]
          /\ outcome' = [outcome EXCEPT ![d] =
                            IF f[1] = "return" THEN "ok" ELSE "error"]
          /\ settledBy' = [settledBy EXCEPT ![d] = f[2]]
          /\ aheld' = IF f[1] = "return" THEN aheld + 1 ELSE aheld
    /\ UNCHANGED <<aclosed, bclosed, closeCause, anext, aid, bcall, bseq,
                   bmark, bcancelled, cancels, toB, aout, bout, ablock,
                   bgrants, granted, aReleased, aCleared, aArgsOut, aArgGiven,
                   aArgBack, bArgsGiven, bLost, forgotten, settlesClosed,
                   aheldClosed, orphaned, bblock>>

\* A reply for a call A cancelled or abandoned while keeping its entry: it
\* is decoded (so the reference a `return` carries is released at once) and
\* discarded. The session goes on.
\* The mutation applies it as if the call were still waiting: the call is
\* settled a second time and its value kept.
ADeliverLate(d) ==
    /\ ~aclosed
    /\ ablock = <<>>
    /\ toA /= <<>>
    /\ LET f == Head(toA)
           bug == Mutation = "lateReplyTwice"
       IN /\ f[1] \in {"return", "throw"}
          /\ aid[d] = f[3]
          /\ acall[d] \in {"cancelled", "abandoned"}
          /\ aTable[d] # "none"
          /\ toA' = Tail(toA)
          /\ orphaned' = [orphaned EXCEPT ![d] = @ + 1]
          /\ acall' = IF bug THEN [acall EXCEPT ![d] = "settled"] ELSE acall
          /\ aTable' = IF bug THEN aTable
                       ELSE [aTable EXCEPT ![d] = "none"]
          /\ aedge' = IF Mutation = "abandonLeaks" /\ acall[d] = "abandoned"
                      THEN aedge ELSE [aedge EXCEPT ![d] = FALSE]
          /\ settles' = IF bug THEN [settles EXCEPT ![d] = @ + 1] ELSE settles
          /\ outcome' = IF bug THEN [outcome EXCEPT ![d] = "ok"] ELSE outcome
          /\ settledBy' = IF bug
                          THEN [settledBy EXCEPT ![d] = f[2]] ELSE settledBy
          /\ aheld' = IF bug /\ f[1] = "return" THEN aheld + 1 ELSE aheld
          /\ aReleased' = IF ~bug /\ f[1] = "return"
                              /\ Mutation # "lateReplyKeepsGrant"
                          THEN aReleased + 1 ELSE aReleased
          /\ IF bug \/ f[1] = "throw" \/ Mutation = "lateReplyKeepsGrant"
             THEN UNCHANGED <<toB, aout, ablock>>
             ELSE ReaderToB(<<"release", 1>>)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, anext, aid, bcall, bseq,
                   bmark, bcancelled, cancels, bout, bblock, bgrants,
                   granted, aCleared, aArgsOut, aArgGiven, aArgBack,
                   bArgsGiven, bLost, forgotten, settlesClosed, aheldClosed>>

\* A cancelled call the bounded table has dropped to stay inside its bound:
\* a reply to it is discarded rather than taken for an unknown call, which is
\* the "explicitly not guaranteed" that `SESS-41` has to carry for this rule.
\* A caller that keeps `SESS-41` strict for it instead (`forgetStrict`) ends
\* the session on that reply (`call-mut-forget-strict`).
Forgotten(d) ==
    /\ CancelTable = "bounded"
    /\ Mutation # "forgetStrict"
    /\ acall[d] = "cancelled"
    /\ d \in forgotten

\* A reply for a cancelled call whose entry the bounded table no longer
\* holds: it is discarded like a late reply (the "not guaranteed" the
\* bounded table buys: `SESS-41` must tolerate it). The reference it carries
\* is given back all the same.
ADeliverForgotten(d) ==
    /\ ~aclosed
    /\ ablock = <<>>
    /\ toA /= <<>>
    /\ LET f == Head(toA)
       IN /\ f[1] \in {"return", "throw"}
          /\ aid[d] = f[3]
          /\ aTable[d] = "none"
          /\ Forgotten(d)
          /\ toA' = Tail(toA)
          /\ orphaned' = [orphaned EXCEPT ![d] = @ + 1]
          /\ aReleased' = IF f[1] = "return" THEN aReleased + 1 ELSE aReleased
          /\ IF f[1] = "throw"
             THEN UNCHANGED <<toB, aout, ablock>>
             ELSE ReaderToB(<<"release", 1>>)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, bout,
                   bblock, aheld, bgrants, granted, aCleared, aArgsOut,
                   aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy>>

\* A reply for a call A can no longer place: the wait table has neither a
\* waiter nor an entry for it, so it is the protocol error `response for
\* unknown call` and the session ends (SESS-41).
ADeliverStray ==
    /\ ~aclosed
    /\ ablock = <<>>
    /\ toA /= <<>>
    /\ LET f == Head(toA)
       IN /\ f[1] \in {"return", "throw"}
          /\ \E d \in Calls : aid[d] = f[3] /\ acall[d] # "waiting"
                              /\ aTable[d] = "none"
                              /\ ~Forgotten(d)
    /\ CloseA("unknownCall")
    /\ UNCHANGED <<cancels>>

\* A gives up on a call: the caller gets its own error, and a reply that was
\* already on its way is discarded. The cancel frame goes out after the
\* invoke, in this direction's order, and does not end the session.
\* The mutation ends A's session right here.
\* What the table keeps for the cancelled call is the cancel-table rule:
\* `"late"` and `"ack"` keep the entry (the ack, or the late reply, clears
\* it), `"forget"` keeps nothing, and `"bounded"` drops the oldest entry once
\* it holds `TableCap` of them and remembers that its replies are to be
\* discarded.
ACancel(c) ==
    /\ ~aclosed
    /\ ~bclosed
    /\ acall[c] = "waiting"
    /\ cancels < MaxCancels
    /\ IF Mutation = "cancelCloses"
       THEN CloseA("cancel") /\ UNCHANGED <<cancels>>
       ELSE LET kept == {d \in Calls : aTable[d] = "cancelled"}
                evict == CancelTable = "bounded" /\ Cardinality(kept) >= TableCap
                         /\ Cardinality(kept) > 0
                old == IF kept = {}
                       THEN c
                       ELSE CHOOSE d \in kept : \A e \in kept : aid[d] <= aid[e]
            IN /\ cancels' = cancels + 1
               /\ acall' = [acall EXCEPT ![c] = "cancelled"]
               /\ aTable' = IF CancelTable = "forget"
                            THEN [aTable EXCEPT ![c] = "none"]
                            ELSE IF evict
                                 THEN [aTable EXCEPT ![c] = "cancelled",
                                                     ![old] = "none"]
                                 ELSE [aTable EXCEPT ![c] = "cancelled"]
               /\ forgotten' = IF evict THEN forgotten \cup {old}
                               ELSE forgotten
               /\ aedge' = [aedge EXCEPT ![c] = FALSE]
               /\ settles' = [settles EXCEPT ![c] = @ + 1]
               /\ outcome' = [outcome EXCEPT ![c] = "cancel"]
               /\ SendToB(<<"cancel", c>>)
               /\ UNCHANGED <<aclosed, bclosed, closeCause, anext, aid, bcall,
                              bseq, bmark, bcancelled, toA, bout, ablock,
                              bblock, aheld, bgrants, granted, aReleased,
                              aCleared, aArgsOut, aArgGiven, aArgBack,
                              bArgsGiven, bLost, settlesClosed, aheldClosed,
                              settledBy, orphaned>>

\* A synchronous call is given up locally, without a cancel frame: the
\* caller's own wait ends, the callee does not learn it. `keep` leaves the
\* waiter and the wait-graph edge in A's table, so the reply is absorbed as
\* a late reply and cleans both; `drop` forgets them, so the reply is an
\* unknown call (ADeliverStray). This is the shape of a sync wait, which
\* has no drop guard (an async call's future sends `cancel` when dropped).
AAbandon(c) ==
    /\ Abandon # "none"
    /\ ~aclosed
    /\ ~bclosed
    /\ c \in Sync
    /\ acall[c] = "waiting"
    /\ acall' = [acall EXCEPT ![c] = "abandoned"]
    /\ aTable' = [aTable EXCEPT ![c] =
                     IF Abandon = "keep" THEN "abandoned" ELSE "none"]
    /\ aedge' = IF Abandon = "drop"
                THEN [aedge EXCEPT ![c] = FALSE] ELSE aedge
    /\ settles' = [settles EXCEPT ![c] = @ + 1]
    /\ outcome' = [outcome EXCEPT ![c] = "abandon"]
    /\ UNCHANGED <<aclosed, bclosed, closeCause, anext, aid, bcall, bseq,
                   bmark, bcancelled, cancels, toA, toB, aout, bout,
                   ablock, bblock, aheld, bgrants, granted, aReleased,
                   aCleared, aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost,
                   forgotten, settlesClosed, aheldClosed, settledBy, orphaned>>

\* A drops what it holds and releases it: the count is what A holds now.
ARelease(n) ==
    /\ ~aclosed
    /\ ~bclosed
    /\ n \in 1..aheld
    /\ aheld' = aheld - n
    /\ aReleased' = aReleased + n
    /\ SendToB(<<"release", n>>)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toA,
                   bout, ablock, bblock, bgrants, granted, aCleared, aArgsOut,
                   aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

\* The reference an argument carried comes back: B's reader dropped the
\* arguments of a call it let go, and the import wrote `release`. A's export
\* entry goes with it.
AReadArgsRelease(n) ==
    /\ ~aclosed
    /\ ablock = <<>>
    /\ toA /= <<>>
    /\ Head(toA) = <<"release", n>>
    /\ n \in 1..aArgsOut
    /\ toA' = Tail(toA)
    /\ aArgsOut' = aArgsOut - n
    /\ aArgBack' = aArgBack + n
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toB, aout,
                   bout, ablock, bblock, aheld, bgrants, granted, aReleased,
                   aCleared, aArgGiven, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

\* The callee acknowledged the cancel (`CancelTable = "ack"`): the caller's
\* entry goes now instead of waiting for a reply that must not come.
AReadAck(c) ==
    /\ ~aclosed
    /\ ablock = <<>>
    /\ toA /= <<>>
    /\ Head(toA) = <<"ack", c>>
    /\ acall[c] = "cancelled"
    /\ aTable[c] \in {"cancelled", "none"}
    /\ toA' = Tail(toA)
    /\ aTable' = IF aTable[c] = "cancelled"
                 THEN [aTable EXCEPT ![c] = "none"] ELSE aTable
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aedge,
                   bcall, bseq, bmark, bcancelled, cancels, toB, aout, bout,
                   ablock, bblock, aheld, bgrants, granted, aReleased,
                   aCleared, aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost,
                   forgotten, settlesClosed, aheldClosed, settles, outcome,
                   settledBy, orphaned>>

\* B's close reached A: the channel ended, so A sees it.
ALearn ==
    /\ bclosed
    /\ ~aclosed
    /\ CloseA("b")
    /\ UNCHANGED <<cancels>>

\* A reader blocked on a full channel writes its frame once the other side
\* read one (before #1005, and only then).
AUnblock ==
    /\ ablock /= <<>>
    /\ Len(toB) < Cap
    /\ toB' = Append(toB, ablock)
    /\ ablock' = <<>>
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toA, aout,
                   bout, bblock, aheld, bgrants, granted, aReleased, aCleared,
                   aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

BUnblock ==
    /\ bblock /= <<>>
    /\ Len(toA) < Cap
    /\ toA' = Append(toA, bblock)
    /\ bblock' = <<>>
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toB, aout,
                   bout, ablock, aheld, bgrants, granted, aReleased, aCleared,
                   aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

(***************************************************************************)
(* Frames the channels still held when the session closed. Whichever side  *)
(* reads one, it is refused: `receive` looks at `closed` before the frame, *)
(* so the frame is consumed and nothing else changes. `lateFrameActs` is   *)
(* the mutation that applies a reply anyway.                               *)
(***************************************************************************)

AReadAfterClose ==
    /\ AfterClose = "read"
    /\ Mutation # "lateFrameActs"
    /\ aclosed \/ bclosed
    /\ toA /= <<>>
    /\ toA' = Tail(toA)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toB, aout,
                   bout, ablock, bblock, aheld, bgrants, granted, aReleased,
                   aCleared, aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost,
                   forgotten, settlesClosed, aheldClosed, settles, outcome,
                   settledBy, orphaned>>

BReadAfterClose ==
    /\ AfterClose = "read"
    /\ aclosed \/ bclosed
    /\ bblock = <<>>
    /\ toB /= <<>>
    /\ toB' = Tail(toB)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toA, aout,
                   bout, ablock, bblock, aheld, bgrants, granted, aReleased, aCleared,
                   aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

\* The mutation: a reply already on its way when the session closed is
\* applied as if the session were open. The call the close failed is settled
\* a second time, and a `return` keeps its reference in a closed session.
AReadAfterCloseActs(d) ==
    /\ Mutation = "lateFrameActs"
    /\ aclosed
    /\ toA /= <<>>
    /\ LET f == Head(toA)
       IN /\ f[1] \in {"return", "throw"}
          /\ aid[d] = f[3]
          /\ settles[d] = 1
          /\ toA' = Tail(toA)
          /\ settles' = [settles EXCEPT ![d] = @ + 1]
          /\ aheld' = IF f[1] = "return" THEN aheld + 1 ELSE aheld
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toB, aout,
                   bout, ablock, bblock, bgrants, granted, aReleased, aCleared,
                   aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, outcome, settledBy, orphaned>>

(***************************************************************************)
(* The writer threads: a frame waits in the queue until the channel takes  *)
(* it. The far end reads one frame at a time.                              *)
(***************************************************************************)

FlushB ==
    /\ Bound
    /\ aout /= <<>>
    /\ Len(toB) < Cap
    /\ toB' = Append(toB, Head(aout))
    /\ aout' = Tail(aout)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toA,
                   bout, ablock, bblock, aheld, bgrants, granted, aReleased,
                   aCleared, aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost,
                   forgotten, settlesClosed, aheldClosed, settles, outcome,
                   settledBy, orphaned>>

FlushA ==
    /\ Bound
    /\ bout /= <<>>
    /\ Len(toA) < Cap
    /\ toA' = Append(toA, Head(bout))
    /\ bout' = Tail(bout)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toB, aout,
                   ablock, bblock, aheld, bgrants, granted, aReleased,
                   aCleared, aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost,
                   forgotten, settlesClosed, aheldClosed, settles, outcome,
                   settledBy, orphaned>>

(***************************************************************************)
(* B: admitting calls, running them, replying, and reading cancels and     *)
(* releases.                                                               *)
(***************************************************************************)

\* B takes the next call. A call number only grows, so a number an older
\* call already took is not a new call. A synchronous call is marked for the
\* wait graph. An admitted call pins its target: the object it names stays
\* alive through the method, whatever the caller releases meanwhile.
BReadInvoke(c) ==
    /\ ~aclosed
    /\ ~bclosed
    /\ bblock = <<>>
    /\ toB /= <<>>
    /\ Head(toB)[1] = "invoke"
    /\ Head(toB)[2] = c
    /\ bcall[c] = "none"
    /\ bcall' = [bcall EXCEPT ![c] = "queued"]
    /\ bseq' = [bseq EXCEPT ![c] = Head(toB)[3]]
    /\ bmark' = [bmark EXCEPT ![c] = (c \in Sync)]
    /\ toB' = Tail(toB)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcancelled, cancels, toA, aout, bout, ablock,
                   bblock, aheld, bgrants, granted, aReleased, aCleared,
                   aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

\* B refuses the call as it reads it: the reader thread writes a `throw`
\* back (a `decode_args`/`too_deep` failure). A `throw` is a reply without a
\* grant; it goes through the same write-back as a discarded reply's
\* release.
BRejectInvoke(c) ==
    /\ Rejects
    /\ ~aclosed
    /\ ~bclosed
    /\ bblock = <<>>
    /\ toB /= <<>>
    /\ Head(toB)[1] = "invoke"
    /\ Head(toB)[2] = c
    /\ bcall[c] = "none"
    /\ bcall' = [bcall EXCEPT ![c] = "dropped"]
    /\ bseq' = [bseq EXCEPT ![c] = Head(toB)[3]]
    /\ toB' = Tail(toB)
    /\ ReaderToA(<<"throw", c, Head(toB)[3]>>)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bmark, bcancelled, cancels, aout, ablock, aheld,
                   bgrants, granted, aReleased, aCleared, aArgsOut, aArgGiven,
                   aArgBack, bArgsGiven, bLost, forgotten, settlesClosed,
                   aheldClosed, settles, outcome, settledBy, orphaned>>

\* The method starts. How long it takes is not modelled: the reply may be
\* written at any later point, also after a cancel arrived.
\* A call that names an object looks its target up when it starts only in
\* the mutation that does not pin it: the export entry may have gone with
\* the caller's release, and the call fails.
BRun(c) ==
    /\ bcall[c] = "queued"
    /\ LET lost == Mutation = "noPin" /\ c \in RefCalls /\ bgrants = 0
       IN IF lost
          THEN /\ bcall' = [bcall EXCEPT ![c] = "dropped"]
               /\ bLost' = bLost + 1
               /\ SendToA(<<"throw", c, bseq[c]>>)
               /\ UNCHANGED bmark
          ELSE /\ bcall' = [bcall EXCEPT ![c] = "running"]
               /\ UNCHANGED <<bmark, bLost, toA>>
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bseq, bcancelled, cancels, toB, aout, bout, ablock,
                   bblock, aheld, bgrants, granted, aReleased, aCleared,
                   aArgsOut, aArgGiven, aArgBack, bArgsGiven, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

\* The method answers: the call is done at B, and the reply (with the one
\* reference it carries) goes out. A running call answers also after a
\* cancel arrived when the rule says so (`Replies = "always"`); a queued one
\* is dropped instead (BReadCancel), and one cancelled while running is
\* dropped without an answer when the rule says `"never"` (BDrop).
BReply(c) ==
    /\ bcall[c] = "running"
    /\ Replies = "always" \/ ~bcancelled[c]
    /\ bcall' = [bcall EXCEPT ![c] = "done"]
    /\ bmark' = [bmark EXCEPT ![c] = FALSE]
    /\ SendToA(<<"return", c, bseq[c]>>)
    /\ bgrants' = bgrants + 1
    /\ granted' = granted + 1
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bseq, bcancelled, cancels, toB, aout, ablock,
                   bblock, aheld, aReleased, aCleared, aArgsOut, aArgGiven,
                   aArgBack, bArgsGiven, bLost, forgotten, settlesClosed,
                   aheldClosed, settles, outcome, settledBy, orphaned>>

\* The method finished but the rule is not to answer a call cancelled while
\* it ran: the call is dropped without a frame.
BDrop(c) ==
    /\ Replies = "never"
    /\ bcall[c] = "running"
    /\ bcancelled[c]
    /\ bcall' = [bcall EXCEPT ![c] = "dropped"]
    /\ bmark' = [bmark EXCEPT ![c] = FALSE]
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bseq, bcancelled, cancels, toA, toB, aout, bout,
                   ablock, bblock, aheld, bgrants, granted, aReleased,
                   aCleared, aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost,
                   forgotten, settlesClosed, aheldClosed, settles, outcome,
                   settledBy, orphaned>>

\* The call lets its arguments go (it ran, or it was dropped while queued):
\* the imports they carried are dropped, and each writes `release` back on
\* the same session — the reader thread's write-back when the call was
\* dropped, the executor's after the method. The mutation drops them without
\* writing anything back.
BReleaseArgs(c) ==
    /\ ~aclosed
    /\ ~bclosed
    /\ c \in Args
    /\ bcall[c] \in {"done", "dropped"}
    /\ ~bArgsGiven[c]
    /\ bArgsGiven' = [bArgsGiven EXCEPT ![c] = TRUE]
    /\ IF Mutation = "argLeaks"
       THEN UNCHANGED <<toA, bout, bblock>>
       ELSE ReaderToA(<<"release", 1>>)
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toB, aout,
                   ablock, aheld, bgrants, granted, aReleased, aCleared,
                   aArgsOut, aArgGiven, aArgBack, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

\* A cancel arrives. A call still queued never runs and is not answered; one
\* already running keeps running and answers (or is dropped, by the rule);
\* one already answered is not touched (the cancel crossed its reply).
\* With `CancelTable = "ack"` the callee acknowledges the cancel, which is
\* what lets the caller clear its entry; the caller must then not see a
\* reply, so the ack rule comes with `Replies = "never"`.
\* Nothing else changes.
BReadCancel(c) ==
    /\ ~aclosed
    /\ ~bclosed
    /\ bblock = <<>>
    /\ toB /= <<>>
    /\ Head(toB) = <<"cancel", c>>
    /\ toB' = Tail(toB)
    /\ bcall' = IF bcall[c] = "queued" THEN [bcall EXCEPT ![c] = "dropped"]
               ELSE bcall
    /\ bmark' = IF bcall[c] = "queued" THEN [bmark EXCEPT ![c] = FALSE]
               ELSE bmark
    /\ bcancelled' = IF Replies = "never" /\ bcall[c] = "running"
                     THEN [bcancelled EXCEPT ![c] = TRUE] ELSE bcancelled
    \* An acknowledgement resolves a call the callee still had: queued or
    \* running. A call already answered is not acknowledged: the reply it
    \* wrote cleared the caller's entry (the cancel crossed it).
    /\ IF CancelTable = "ack" /\ bcall[c] \in {"queued", "running"}
       THEN ReaderToA(<<"ack", c>>)
       ELSE UNCHANGED <<toA, bout, bblock>>
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bseq, cancels, aout, ablock, aheld, bgrants,
                   granted, aReleased, aCleared, aArgsOut, aArgGiven, aArgBack,
                   bArgsGiven, bLost, forgotten, settlesClosed, aheldClosed,
                   settles, outcome, settledBy, orphaned>>

\* A release arrives: the grants it names are given up. The export goes away
\* once nothing is granted any more — a release of an older grant leaves a
\* newer one in place. The mutation drops the whole entry on any release.
BReadRelease(n) ==
    /\ ~aclosed
    /\ ~bclosed
    /\ bblock = <<>>
    /\ toB /= <<>>
    /\ Head(toB) = <<"release", n>>
    /\ n \in 1..bgrants
    /\ toB' = Tail(toB)
    /\ bgrants' = IF Mutation = "releaseDropsAll" THEN 0 ELSE bgrants - n
    /\ UNCHANGED <<aclosed, bclosed, closeCause, acall, anext, aid, aTable,
                   aedge, bcall, bseq, bmark, bcancelled, cancels, toA, aout,
                   bout, ablock, bblock, aheld, granted, aReleased, aCleared,
                   aArgsOut,
                   aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

\* B closes its side: the calls it has not answered are dropped, its exports
\* go, and the frames in both directions are lost — or, with `AfterClose =
\* "read"`, stay on the channels to be read and refused.
\* The mutation keeps the export table.
CloseB(cause) ==
    /\ bclosed' = TRUE
    /\ closeCause' = IF closeCause = "none" THEN cause ELSE closeCause
    /\ bcall' = [c \in Calls |-> IF bcall[c] \in {"queued", "running"}
                                   THEN "dropped" ELSE bcall[c]]
    /\ bmark' = [c \in Calls |-> FALSE]
    /\ bgrants' = IF Mutation = "closeLeaks" THEN bgrants ELSE 0
    /\ IF AfterClose = "read"
       THEN UNCHANGED <<toA, toB>>
       ELSE /\ toA' = <<>>
            /\ toB' = <<>>
    /\ aout' = <<>>
    /\ bout' = <<>>
    /\ ablock' = <<>>
    /\ bblock' = <<>>
    /\ UNCHANGED <<aclosed, acall, anext, aid, aTable, aedge, bseq,
                   bcancelled, cancels, aheld, granted, aReleased, aCleared,
                   aArgsOut, aArgGiven, aArgBack, bArgsGiven, bLost, forgotten,
                   settlesClosed, aheldClosed, settles, outcome, settledBy,
                   orphaned>>

BClose ==
    /\ Closes
    /\ ~bclosed
    /\ CloseB("b")

\* A's close reached B.
BLearn ==
    /\ aclosed
    /\ ~bclosed
    /\ CloseB("a")

\* A release this side cannot honour: more units than it granted end the
\* session (SESS-47, "invalid reference release count"; SESS-48 for a
\* reference it does not hold at all). A conforming caller never sends one;
\* `releaseDropsAll` is what makes it reachable — the caller still holds a
\* grant the table has already thrown away.
BReadReleaseBad(n) ==
    /\ ~aclosed
    /\ ~bclosed
    /\ bblock = <<>>
    /\ toB /= <<>>
    /\ Head(toB) = <<"release", n>>
    /\ n \in 1..(aheld + aReleased)
    /\ n > bgrants
    /\ toB' = Tail(toB)
    /\ CloseB("badRelease")

\* The host's bound on the writer queue (SESS-75): the queue is at the bound
\* and the channel is full, so the writer cannot drain it. The session ends
\* with the far end not reading, instead of the queue growing.
OverLimitA ==
    /\ OutboundLimit = "frames"
    /\ ~aclosed
    /\ Len(aout) >= Limit
    /\ Len(toB) >= Cap
    /\ CloseA("notReading")
    /\ UNCHANGED <<cancels>>

OverLimitB ==
    /\ OutboundLimit = "frames"
    /\ ~bclosed
    /\ Len(bout) >= Limit
    /\ Len(toA) >= Cap
    /\ CloseB("notReading")

(***************************************************************************)
(* The specification.                                                      *)
(***************************************************************************)

\* Nothing is left to do: every call is settled on both sides and no frame
\* waits anywhere. Only then may the model stop, so that TLC's deadlock
\* check tells "the work is done" from "both readers are blocked".
AllSettled ==
    /\ \A c \in Calls : acall[c] \in {"idle", "settled", "cancelled",
                                      "abandoned"}
    /\ \A c \in Calls : bcall[c] \in {"none", "done", "dropped"}
    /\ toA = <<>>
    /\ toB = <<>>
    /\ aout = <<>>
    /\ bout = <<>>
    /\ ablock = <<>>
    /\ bblock = <<>>

Done ==
    /\ AllSettled
    /\ UNCHANGED vars

Next ==
    \/ \E c \in Calls : AStart(c) \/ ACancel(c) \/ AAbandon(c)
    \/ \E c \in Calls : ADeliverLive(c) \/ ADeliverLate(c) \/ ADeliverForgotten(c)
    \/ \E c \in Calls : AReadAck(c)
    \/ ADeliverStray
    \/ \E n \in 1..aheld : ARelease(n)
    \/ \E n \in 1..aArgsOut : AReadArgsRelease(n)
    \/ AClose
    \/ ALearn
    \/ AUnblock
    \/ AReadAfterClose
    \/ \E c \in Calls : AReadAfterCloseActs(c)
    \/ \E c \in Calls : BReadInvoke(c) \/ BRejectInvoke(c) \/ BRun(c)
         \/ BReply(c) \/ BDrop(c) \/ BReadCancel(c) \/ BReleaseArgs(c)
    \/ \E n \in 1..bgrants : BReadRelease(n)
    \/ \E n \in 1..(aheld + aReleased) : BReadReleaseBad(n)
    \/ BClose
    \/ BLearn
    \/ BUnblock
    \/ BReadAfterClose
    \/ FlushA
    \/ FlushB
    \/ OverLimitA
    \/ OverLimitB
    \/ Done

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)

\* Nothing waits anywhere: no frame on a channel or in a writer queue, no
\* blocked reader, no call in flight at either side.
Quiescent ==
    /\ ~aclosed
    /\ ~bclosed
    /\ toA = <<>>
    /\ toB = <<>>
    /\ aout = <<>>
    /\ bout = <<>>
    /\ ablock = <<>>
    /\ bblock = <<>>
    /\ \A c \in Calls : acall[c] # "waiting"
    /\ \A c \in Calls : bcall[c] \notin {"queued", "running"}
    \* A reference the arguments carried is given back when its call lets the
    \* arguments go; a call whose write-back is still owed is not at rest.
    /\ \A c \in Args : bArgsGiven[c]

\* A cancel never ends the session: the close that ends it is the caller's,
\* the callee's, or the one the channel end brings.
CancelKeepsSession == closeCause # "cancel"

\* A conforming pair never ends a session with a reply the caller cannot
\* place (SESS-41 read as "a correct sender never produces one"). A locally
\* abandoned call (AbandonKeepsSession) and a cancelled call whose table
\* entry was dropped too early (`call-gap-unknown-late`) are the two ways to
\* break it.
NoUnknownCall == closeCause # "unknownCall"

\* The same rule where a locally abandoned call is concerned.
AbandonKeepsSession == NoUnknownCall

\* A call is settled at most once, whether by its reply, by a cancel, by a
\* local abandonment or by the session ending.
SettlesAtMostOnce == \A c \in Calls : settles[c] <= 1

\* A late reply is discarded: the cancelled (or abandoned) call keeps the
\* outcome it got, and the reply does not settle it a second time. Each call
\* has at most one reply to discard.
LateReplyDropped ==
    \A c \in Calls :
        orphaned[c] > 0 => (outcome[c] \in {"cancel", "abandon"}
                            /\ settles[c] = 1)
        /\ orphaned[c] <= 1

\* The reply that settles a call is the reply of that call.
OwnReply == \A c \in Calls : settledBy[c] \in {"none", c}

\* A call number belongs to one call: no new call takes it again.
NoIdReuse ==
    \A c, d \in Calls :
        c # d /\ aid[c] # 0 /\ aid[d] # 0 => aid[c] # aid[d]

\* A gives back only what it was granted: a grant is released, dropped or
\* cleared once, never twice.
NoOverRelease == aheld + aReleased + aCleared <= granted

\* Once A's session is over it holds no reference, and once B's is over it
\* exports none.
ClosedClearsTables ==
    (aclosed => aheld = 0) /\ (bclosed => bgrants = 0)

\* A closed session leaves no call unfinished on either side.
FailUnfinishedOnClose ==
    /\ (aclosed => \A c \in Calls : acall[c] \in {"idle", "settled",
                                                   "cancelled", "abandoned"})
    /\ (bclosed => \A c \in Calls : bcall[c] \in {"none", "done", "dropped"})

\* A frame read after the session closed changes nothing: no call is settled
\* after the close that failed it, and no reference is taken into a closed
\* session.
LateFramesInert ==
    aclosed => (settles = settlesClosed /\ aheld = aheldClosed)

\* An admitted call keeps its target: no call is dropped for want of the
\* object it names, and a release of an older grant does not take a newer
\* one — a session that ends on a release count it granted was dropped for.
PinnedTarget ==
    /\ bLost = 0
    /\ closeCause # "badRelease"

\* Every reference A granted through an argument is given back by the callee
\* when the call lets the arguments go: at rest A's table holds none of them.
ArgRefsReleased ==
    Quiescent => aArgsOut = 0

\* The count A reads back is the count it gave out: the grants, the ones still
\* out and the ones given back agree while the session is open.
ArgRefsCounted ==
    ~aclosed => aArgsOut + aArgBack = aArgGiven

\* The writer queue never holds more than the host allows (SESS-75): what
\* would not fit ends the session instead (`OverLimitA`/`OverLimitB`).
BacklogBounded ==
    OutboundLimit = "none"
    \/ (Len(aout) <= Limit /\ Len(bout) <= Limit)

\* Liveness, with no close to fall back on: a call A waits for is answered
\* or cancelled, so the callee answers what it took.
WaitingSettles ==
    \A c \in Calls : (acall[c] = "waiting") ~> (acall[c] \in {"settled",
                                                              "cancelled",
                                                              "abandoned"})

\* The bounded table never holds more than `TableCap` entries (the #1099
\* "bounded table" option): an entry may stay until the session ends, but the
\* table cannot grow with the number of cancellations.
CancelledTableBounded ==
    Cardinality({d \in Calls : aTable[d] = "cancelled"}) <= TableCap

\* The caller's table of cancelled calls drains: no entry waits for a reply
\* to clear it. A callee that never answers a cancelled call leaves one
\* behind (the #626 gap: `call-gap-cancel-table`), whether the entry is
\* cleared by a late reply (`"late"`), by an acknowledgement (`"ack"`), or
\* dropped by the bounded table (`"bounded"`).
CancelledEntriesDrain ==
    Quiescent => \A c \in Calls : aTable[c] # "cancelled"

\* No grant is lost: at rest every unit B granted is held by A or given
\* back. A discarded late reply that did not release its reference would
\* leave a unit unaccounted (the reference-conservation rule of trace
\* contract §4).
GrantsConserved ==
    Quiescent => aheld + aReleased + aCleared = granted

\* A locally abandoned call leaves no wait-graph edge once the session is
\* quiet: the reply the callee still sends cleans it.
AbandonedCallsClean ==
    Quiescent => \A c \in Calls : ~aedge[c]

=============================================================================
