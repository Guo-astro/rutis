--------------------------------- MODULE EventsEmit -----------------------------------
(**************************************************************************************)
(* `emit` order at one runtime and one host: a call held during a synchronous wait, *)
(* the reply frame read in place, later emits of the same name (from the wait     *)
(* chain and from another row), a filter that may answer late or never, the host's *)
(* per-session queue whose items remember their row, a row that unloads while its *)
(* emits wait, the receiver's rule a (overlapping calls) or b (one call per       *)
(* listener at a time), a receiver that starts a call only on a later turn        *)
(* (Python's `loop.call_soon`), and a host that replies before the calls started  *)
(* (c2c1b7e8 before, #513).                                                       *)
(*                                                                                    *)
(* Design: docs/design-events-protocol-2026-10-18.md §5.2 (fast path), §5.4 (`emit`  *)
(* starts every call after the one before started; the reply after that), §5.5 (the *)
(* order of emits: the host queues a session's emits, the runtime queues its own    *)
(* while a filter answers, a held call is passed by no later emit, the reply is     *)
(* read in place, after the reply the name is off the via-host path), §6 (a filter  *)
(* can answer late), §7.1 (a row's queued and filtering emits go with it).           *)
(* Implementation: crates/rutis/src/bus.rs (`emit_erased`: the per-name tail and    *)
(* the backlog), crates/rutis-bridge/src/events/hub.rs (`SessionEvents::dispatch`,  *)
(* `run_emit`, `next_emit`, `let_go_of_its_emits`, `cancelling`), and the runtimes' *)
(* emit queues (node/rutis-runtime/src/events.mjs, python/rutis/rutis/runner.py,    *)
(* go/rutis/internal/runtime/events.go) and Python's frame dispatch                 *)
(* (python/rutis/rutis/peer.py `_dispatch`: `loop.call_soon`).                      *)
(*                                                                                    *)
(* The scenario (README.md, "Simplified"): R1's row A emits E1 through the host    *)
(* while its listener L1 is inside a synchronous wait (a service call), so E1's     *)
(* call at L1 is held; R1 reads E1's reply in place; the wait chain emits E2, and   *)
(* row B emits E3. The host takes one emit at a time, in issue order; every item    *)
(* remembers its row, so unloading row A drops only A's emits. L2 is R2's listener, *)
(* which is why E1 goes through the host, and whose receive rule is a or b.        *)
(**************************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, EventsHost

CONSTANTS
    L1,           \* R1's listener on the emitting row A
    L2,           \* R2's listener (outside R1: E1 goes through the host)
    RowA,         \* the row that emits E1 and E2
    RowB,         \* the row that emits E3
    Filter1,      \* E1's filter: "none" | "late" | "never"
    Filter2,      \* E2's filter
    Filter3,      \* E3's filter
    R2Rule,       \* "a" (starts a call when the frame is read) | "b" (one at a time)
    Defer,        \* "none" | "nextRound" (R2 starts a call on a later turn, FIFO)
    Mutation      \* "none" | "replyAsync" | "heldEmit" | "unloadQueue" |
                  \* "deadListener" | "futureReply"

ASSUME L1 # L2
ASSUME RowA # RowB
ASSUME Filter1 \in {"none", "late", "never"}
ASSUME Filter2 \in {"none", "late", "never"}
ASSUME Filter3 \in {"none", "late", "never"}
ASSUME R2Rule \in {"a", "b"}
ASSUME Defer \in {"none", "nextRound"}
ASSUME Mutation \in {"none", "replyAsync", "heldEmit", "unloadQueue",
                     "deadListener", "futureReply"}

Listeners == {L1, L2}

Emits == {1, 2, 3}

E1 == 1

E2 == 2

E3 == 3

Rows == {RowA, RowB}

RowOf == [e \in Emits |-> IF e = E3 THEN RowB ELSE RowA]

Filter(e) ==
    CASE e = E1 -> Filter1
      [] e = E2 -> Filter2
      [] OTHER -> Filter3

-------------------------------------------------------------------------------
(* Helpers                                                                    *)
-------------------------------------------------------------------------------

RECURSIVE StartsOf(_)
StartsOf(seq) ==
    IF seq = <<>> THEN <<>>
    ELSE <<[emit |-> Head(seq)[1], listener |-> Head(seq)[2]]>>
         \o StartsOf(Tail(seq))

RECURSIVE CallsOf(_, _)
CallsOf(seq, e) ==
    IF seq = <<>> THEN <<>>
    ELSE IF Head(seq)[1] = e
         THEN <<Head(seq)[2]>> \o CallsOf(Tail(seq), e)
         ELSE CallsOf(Tail(seq), e)

RECURSIVE EmitsOfHeld(_)
EmitsOfHeld(seq) ==
    IF seq = <<>> THEN <<>>
    ELSE <<Head(seq)[1]>> \o EmitsOfHeld(Tail(seq))

-------------------------------------------------------------------------------
(* State                                                                      *)
-------------------------------------------------------------------------------

VARIABLES
    phase,        \* emit -> "none" | "queued" | "filtering" | "starting" |
                  \*         "replied" | "dropped"
    path,         \* emit -> "none" | "host" | "fast"
    kept,         \* emit -> the listeners its filter keeps
    left,         \* emit -> the listeners whose call has not started yet, in order
    read,         \* emit -> R1 has read its reply frame
    marked,       \* emit -> R1 recorded the reply (only replyAsync separates them)
    wait,         \* R1 is in a synchronous wait (a service call, not this emit)
    held,         \* calls held at R1, in arrival order: <<emit, listener>>
    deferred,     \* calls at R2 whose frame was read, to start on a later turn,
                  \* in arrival order: <<emit, listener>> (only Defer = "nextRound")
    started,      \* emit -> the listeners whose call started, in order (ghost)
    starts,       \* all calls in the order they started (ghost)
    l1Starts,     \* the emits whose L1 call started, in order (ghost)
    issueOrder,   \* the emits in the order they were issued (ghost)
    hostQ,        \* the host's queue: emits in issue order, each knows its row
    running,      \* calls at L2 that have not ended yet: <<emit, listener>>
    rowDown,      \* row -> the row has unloaded
    atUnload,     \* emit -> its started calls when its row unloaded (ghost)
    atUnloadL1,   \* l1Starts when row A unloaded (ghost)
    afterRead,    \* emit -> it was issued after E1's reply was read (ghost)
    deadListener  \* a call to an unloaded row's listener started (ghost)

vars == <<phase, path, kept, left, read, marked, wait, held, deferred, started,
          starts, l1Starts, issueOrder, hostQ, running, rowDown, atUnload,
          atUnloadL1, afterRead, deadListener>>

IssuedBefore(f, e) ==
    \E i, j \in 1..Len(issueOrder):
        issueOrder[i] = f /\ issueOrder[j] = e /\ i < j

(* A listener's call for `f` has not started, or `f` may still keep it.       *)
(* `futureReply` is the case where that is no longer true: R1 was told the    *)
(* emit is over while the host still owes its calls, so its later emits do    *)
(* not queue behind them (#513).                                              *)
HostRepliedEarly(f) ==
    Mutation = "futureReply" /\ read[f]

PendingAt(f, l) ==
    \/ l \in Elements(left[f]) /\ ~HostRepliedEarly(f)
    \/ \E h \in Elements(held): h[1] = f /\ h[2] = l
    \/ phase[f] \in {"queued", "filtering"} /\ ~HostRepliedEarly(f)

EarlierPending(e, l) ==
    \E f \in Emits: IssuedBefore(f, e) /\ PendingAt(f, l)

ListenerAlive(l) ==
    l = L2 \/ ~rowDown[RowA]

R2Allows(l) ==
    l # L2 \/ R2Rule = "a"
           \/ \A f \in Emits: <<f, L2>> \notin running

-------------------------------------------------------------------------------
(* What R1 does                                                              *)
-------------------------------------------------------------------------------

Issue(e, fast) ==
    /\ phase[e] = "none"
    /\ ~rowDown[RowOf[e]]
    /\ LET p == IF fast THEN "fast" ELSE "host"
       IN /\ issueOrder' = Append(issueOrder, e)
          /\ path' = [path EXCEPT ![e] = p]
          /\ phase' = [phase EXCEPT ![e] = IF fast THEN "starting" ELSE "queued"]
          /\ afterRead' = [afterRead EXCEPT ![e] = e # E1 /\ read[E1]]
          /\ kept' = [kept EXCEPT ![e] = IF fast THEN Listeners ELSE {}]
          /\ left' = [left EXCEPT ![e] = IF fast THEN <<L1>> ELSE <<>>]
          /\ hostQ' = IF fast THEN hostQ ELSE Append(hostQ, e)
          /\ UNCHANGED <<read, marked, wait, held, deferred, started, starts,
                         l1Starts, running, rowDown, atUnload, atUnloadL1,
                         deadListener>>

(* E1 is emitted by row A, through the host (L2 is outside R1).               *)
Emit1 ==
    Issue(E1, FALSE)

(* R1 waits for a service call (not for this emit).                          *)
BeginWait ==
    /\ ~wait
    /\ wait' = TRUE
    /\ UNCHANGED <<phase, path, kept, left, read, marked, held, deferred, started,
                   starts, l1Starts, issueOrder, hostQ, running, rowDown,
                   atUnload, atUnloadL1, afterRead, deadListener>>

(* §5.5: the reply frame is read in place and the emit recorded as replied;    *)
(* `replyAsync` records it one step later (#513, c2c1b7e8).                  *)
ReadReply(e) ==
    /\ phase[e] = "replied"
    /\ ~read[e]
    /\ read' = [read EXCEPT ![e] = TRUE]
    /\ marked' = [marked EXCEPT ![e] = Mutation # "replyAsync"]
    /\ UNCHANGED <<phase, path, kept, left, wait, held, deferred, started, starts,
                   l1Starts, issueOrder, hostQ, running, rowDown, atUnload,
                   atUnloadL1, afterRead, deadListener>>

(* c2c1b7e8 before, the host's half (#513): the reply to an emit still        *)
(* queued, its calls not started, is written anyway, and R1 reads that frame  *)
(* in place. From then on R1 counts the name as done: its later emits take    *)
(* the fast path and do not queue behind the calls the host still owes        *)
(* (`PendingAt`).                                                             *)
ReplyEarly(e) ==
    /\ Mutation = "futureReply"
    /\ phase[e] \in {"queued", "filtering", "starting"}
    /\ ~read[e]
    /\ read' = [read EXCEPT ![e] = TRUE]
    /\ marked' = [marked EXCEPT ![e] = TRUE]
    /\ UNCHANGED <<phase, path, kept, left, wait, held, deferred, started, starts,
                   l1Starts, issueOrder, hostQ, running, rowDown, atUnload,
                   atUnloadL1, afterRead, deadListener>>

MarkReply(e) ==
    /\ Mutation = "replyAsync"
    /\ read[e]
    /\ ~marked[e]
    /\ marked' = [marked EXCEPT ![e] = TRUE]
    /\ UNCHANGED <<phase, path, kept, left, read, wait, held, deferred, started,
                   starts, l1Starts, issueOrder, hostQ, running, rowDown,
                   atUnload, atUnloadL1, afterRead, deadListener>>

(* R1's wait chain emits E2. Once E1's reply is read, the name is no longer on *)
(* the via-host path (§5.5); `replyAsync` still counts it as unreplied.        *)
Emit2 ==
    /\ phase[E1] # "none"
    /\ phase[E2] = "none"
    /\ Issue(E2, read[E1] /\ ~(Mutation = "replyAsync" /\ ~marked[E1]))

(* Row B emits E3; it need not be the wait chain.                            *)
Emit3 ==
    /\ phase[E1] # "none"
    /\ phase[E3] = "none"
    /\ Issue(E3, read[E1] /\ ~(Mutation = "replyAsync" /\ ~marked[E1]))

(* §5.5: the wait ends. Held calls start in arrival order; the fixed code put  *)
(* a later fast-path call behind them rather than ahead.                     *)
EndWait ==
    /\ wait
    /\ wait' = FALSE
    /\ held' = <<>>
    /\ started' = [e \in Emits |-> started[e] \o CallsOf(held, e)]
    /\ starts' = starts \o StartsOf(held)
    /\ l1Starts' = l1Starts \o EmitsOfHeld(held)
    /\ UNCHANGED <<phase, path, kept, left, read, marked, issueOrder, hostQ,
                   running, rowDown, atUnload, atUnloadL1, afterRead,
                   deadListener, deferred>>

(* §9 / §5.5: an emit's call starts, in registration order. At L1, while R1    *)
(* waits, the call is held; L2's call starts at once (rule a) or when L2 has  *)
(* no running call (rule b). `deadListener` calls a listener of an unloaded    *)
(* row, which the fixed code skips. With `Defer = "nextRound"` an L2 call     *)
(* starts only from the head of `deferred`: Python reads the frame and runs   *)
(* the call a turn later, in the order the frames were read (`DeferCall`).    *)
StartCall(e) ==
    /\ phase[e] = "starting"
    /\ left[e] /= <<>>
    /\ (Defer = "none" \/ Head(left[e]) # L2
        \/ (deferred /= <<>> /\ Head(deferred) = <<e, L2>>))
    /\ LET l == Head(left[e])
           keep == l \in kept[e]
           alive == ListenerAlive(l)
           mightStart == alive \/ Mutation = "deadListener"
           hold == l = L1 /\ wait /\ ~(Mutation = "heldEmit" /\ e # E1)
       IN /\ (EarlierPending(e, l) = FALSE \/ Mutation = "heldEmit")
          /\ R2Allows(l)
          /\ left' = [left EXCEPT ![e] = Tail(@)]
          /\ IF keep /\ mightStart /\ hold
             THEN held' = Append(held, <<e, l>>)
             ELSE held' = held
          /\ IF keep /\ mightStart /\ ~hold
             THEN started' = [started EXCEPT ![e] = Append(@, l)]
             ELSE started' = started
          /\ starts' = IF keep /\ mightStart /\ ~hold
                        THEN Append(starts, [emit |-> e, listener |-> l])
                        ELSE starts
          /\ l1Starts' = IF keep /\ mightStart /\ ~hold /\ l = L1
                          THEN Append(l1Starts, e)
                          ELSE l1Starts
          /\ running' = IF keep /\ mightStart /\ ~hold /\ l = L2
                         THEN running \cup {<<e, l>>}
                         ELSE running
          /\ deadListener' = (deadListener \/ (keep /\ ~alive /\ Mutation = "deadListener"))
          /\ deferred' = IF Defer = "nextRound" /\ l = L2
                         THEN Tail(deferred)
                         ELSE deferred
          /\ phase' = [phase EXCEPT ![e] =
                         IF Tail(left[e]) = <<>> THEN "replied" ELSE "starting"]
          /\ UNCHANGED <<path, kept, read, marked, wait, issueOrder, hostQ,
                         rowDown, atUnload, atUnloadL1, afterRead>>

(* Python reads a call's frame and runs it on the next turn of the loop        *)
(* (`peer.py` `_dispatch`: `self.loop.call_soon(self._run, job)`), in the      *)
(* order the frames were read. `nextRound` models that: the call at L2 waits   *)
(* in `deferred` and starts from its head, so it starts later than the frame   *)
(* was read, never out of order.                                               *)
DeferCall(e) ==
    /\ Defer = "nextRound"
    /\ phase[e] = "starting"
    /\ left[e] /= <<>>
    /\ Head(left[e]) = L2
    /\ EarlierPending(e, L2) = FALSE
    /\ \A i \in 1..Len(deferred): deferred[i] # <<e, L2>>
    /\ deferred' = Append(deferred, <<e, L2>>)
    /\ UNCHANGED <<phase, path, kept, left, read, marked, wait, held, started,
                   starts, l1Starts, issueOrder, hostQ, running, rowDown,
                   atUnload, atUnloadL1, afterRead, deadListener>>

(* The receiver of rule b ends a call; rule a also allows this, but does not  *)
(* require it.                                                                *)
EndCall(e) ==
    /\ <<e, L2>> \in running
    /\ running' = running \ {<<e, L2>>}
    /\ UNCHANGED <<phase, path, kept, left, read, marked, wait, held, deferred,
                   started, starts, l1Starts, issueOrder, hostQ, rowDown,
                   atUnload, atUnloadL1, afterRead, deadListener>>

(* §7.1 / §5.5: a row unloads. Its queued emit is dropped from the host queue, *)
(* its filtering/starting emit is let go (an answer then starts nothing), the *)
(* calls already held for it are dropped, and the listener it owned stops     *)
(* taking calls. `unloadQueue` keeps all of them (#536).                     *)
UnloadRow(r) ==
    /\ r \in Rows
    /\ ~rowDown[r]
    /\ rowDown' = [rowDown EXCEPT ![r] = TRUE]
    /\ atUnload' = [e \in Emits |->
                       IF RowOf[e] = r THEN started[e] ELSE atUnload[e]]
    /\ atUnloadL1' = l1Starts
    /\ hostQ' = SelectSeq(hostQ, LAMBDA e: RowOf[e] # r)
    /\ IF Mutation = "unloadQueue"
       THEN UNCHANGED <<phase, left, held, deferred, path, kept, read, marked,
                         wait, started, starts, running, l1Starts, issueOrder,
                         afterRead, deadListener>>
       ELSE /\ phase' = [e \in Emits |->
                            IF RowOf[e] = r
                               /\ phase[e] \in {"queued", "filtering", "starting"}
                            THEN "dropped" ELSE phase[e]]
            /\ left' = [e \in Emits |-> IF RowOf[e] = r THEN <<>> ELSE left[e]]
            /\ held' = SelectSeq(held, LAMBDA h:
                                    RowOf[h[1]] # r /\ (r # RowA \/ h[2] # L1))
            /\ deferred' = SelectSeq(deferred, LAMBDA q: RowOf[q[1]] # r)
            /\ running' = { c \in running :
                              RowOf[c[1]] # r /\ (r # RowA \/ c[2] # L1) }
            /\ UNCHANGED <<path, kept, read, marked, wait, started, starts,
                            l1Starts, issueOrder, afterRead, deadListener>>

-------------------------------------------------------------------------------
(* What the host does                                                        *)
-------------------------------------------------------------------------------

NoHostActive ==
    \A e \in Emits:
        path[e] # "host" \/ phase[e] \notin {"filtering", "starting"}

(* The host takes the next emit of this session; a filter-waiting emit blocks *)
(* the ones behind it, whatever their row.                                   *)
HostTake ==
    /\ hostQ /= <<>>
    /\ NoHostActive
    /\ LET e == Head(hostQ)
       IN /\ phase[e] = "queued"
          /\ phase' = [phase EXCEPT ![e] =
                         IF Filter(e) = "none" THEN "starting" ELSE "filtering"]
          /\ kept' = [kept EXCEPT ![e] =
                        IF Filter(e) = "none" THEN Listeners ELSE {}]
          /\ left' = [left EXCEPT ![e] =
                        IF Filter(e) = "none" THEN <<L1, L2>> ELSE <<>>]
          /\ hostQ' = Tail(hostQ)
          /\ UNCHANGED <<path, read, marked, wait, held, deferred, started, starts,
                         l1Starts, issueOrder, running, rowDown, atUnload,
                         atUnloadL1, afterRead, deadListener>>

(* §6: the selector answers, keeping a set of listeners. `never` never        *)
(* answers; a row that let the emit go makes the answer start nothing.        *)
FilterAnswer(e) ==
    /\ phase[e] = "filtering"
    /\ Filter(e) = "late"
    /\ \E keep \in SUBSET Listeners:
         /\ kept' = [kept EXCEPT ![e] = keep]
         /\ phase' = [phase EXCEPT ![e] = "starting"]
         /\ left' = [left EXCEPT ![e] = <<L1, L2>>]
         /\ UNCHANGED <<path, read, marked, wait, held, deferred, started, starts,
                        l1Starts, issueOrder, hostQ, running, rowDown,
                        atUnload, atUnloadL1, afterRead, deadListener>>

Next ==
    \/ Emit1
    \/ BeginWait
    \/ \E e \in Emits: ReadReply(e)
    \/ \E e \in Emits: MarkReply(e)
    \/ \E e \in Emits: ReplyEarly(e)
    \/ Emit2
    \/ Emit3
    \/ EndWait
    \/ \E e \in Emits: StartCall(e)
    \/ \E e \in Emits: DeferCall(e)
    \/ \E e \in Emits: EndCall(e)
    \/ \E r \in Rows: UnloadRow(r)
    \/ HostTake
    \/ \E e \in Emits: FilterAnswer(e)

(* No work left: no queued emit, no filter or calls in flight, no held or     *)
(* deferred call.                                                             *)
Done ==
    /\ hostQ = <<>>
    /\ held = <<>>
    /\ deferred = <<>>
    /\ running = {}
    /\ ~wait
    /\ \A e \in Emits:
         phase[e] \in {"none", "replied", "dropped"}
    /\ UNCHANGED vars

Init ==
    /\ phase = [e \in Emits |-> "none"]
    /\ path = [e \in Emits |-> "none"]
    /\ kept = [e \in Emits |-> {}]
    /\ left = [e \in Emits |-> <<>>]
    /\ read = [e \in Emits |-> FALSE]
    /\ marked = [e \in Emits |-> FALSE]
    /\ wait = FALSE
    /\ held = <<>>
    /\ deferred = <<>>
    /\ started = [e \in Emits |-> <<>>]
    /\ starts = <<>>
    /\ l1Starts = <<>>
    /\ issueOrder = <<>>
    /\ hostQ = <<>>
    /\ running = {}
    /\ rowDown = [r \in Rows |-> FALSE]
    /\ atUnload = [e \in Emits |-> <<>>]
    /\ atUnloadL1 = <<>>
    /\ afterRead = [e \in Emits |-> FALSE]
    /\ deadListener = FALSE

Spec == Init /\ [][Next]_vars

-------------------------------------------------------------------------------
(* Properties                                                                 *)
-------------------------------------------------------------------------------

(* §5.5: for one process and one listener, the calls of its emits start in the *)
(* order the emits were issued. E1 is issued first, and so on.                *)
EmitOrder ==
    \A i, j \in 1..Len(starts):
      i < j /\ starts[i].listener = starts[j].listener =>
        ~IssuedBefore(starts[j].emit, starts[i].emit)

(* §5.5: once the reply frame was read, a later emit is not left on the        *)
(* via-host path.                                                            *)
FastPathByFrame ==
    \A e \in Emits: afterRead[e] => path[e] = "fast"

(* §5.5 / §7.1: an emit of a row that unloaded starts no call, even after its   *)
(* filter answers.                                                           *)
UnloadedRowEmitsNothing ==
    \A r \in Rows:
      rowDown[r] =>
        \A e \in Emits:
          RowOf[e] = r => started[e] = atUnload[e]

(* §7.1: the listener of an unloaded row starts no call after the unload.      *)
NoCallToUnloadedListener ==
    rowDown[RowA] => l1Starts = atUnloadL1

=======================================================================================
