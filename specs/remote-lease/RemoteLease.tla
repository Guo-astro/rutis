----------------------------- MODULE RemoteLease -----------------------------
(***************************************************************************)
(* A remote runtime that listens, and the controllers that dial it: the    *)
(* lease of the runtime, takeover by a newer connection, reconnecting, a   *)
(* link stopping, and a controller's identity revoking the runtime.        *)
(*                                                                         *)
(* Design: docs/design-remote-plugins-2026-10-03.md §3.2 (retry, link       *)
(* stop, revocation), §4.4 (lease, takeover order); docs/design-remote-     *)
(* runtime-dial-2026-10-14.md §五 (revocation hook, #273), §六 (retry).     *)
(* Implementation modelled: crates/rutis-bridge/src/link.rs `dialing`,      *)
(* `start` (controller); python/rutis/rutis/__main__.py `serve`,            *)
(* go/rutis/listen.go `listen`, node/rutis-runtime/src/serve.mjs `serve`    *)
(* (the runtime). README.md next to this file lists the simplifications.   *)
(*                                                                         *)
(* Connection <<c, n>> is the n-th connection controller c dialed. Each    *)
(* direction of a connection is FIFO; frames are delayed arbitrarily long. *)
(* A side that closed a connection reads nothing more on it: frames to it  *)
(* are dropped (kept only where a mutation reads them).                    *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Controllers,  \* controller links dialing the runtime (model values)
    MaxConns,     \* connections each controller may dial (1 + reconnects)
    MaxLoads,     \* rows a controller loads on one session
    Healthy,      \* TRUE: no revocation, stop, timeout or lost connection
    RevokeCheck,  \* TRUE: the link refuses a revoked far end before it
                  \* provides the peer (the fix); FALSE: link.rs before it
    Mutation      \* "none", or a deliberate bug (README, "Mutations")

ASSUME MaxConns \in Nat \ {0} /\ MaxLoads \in Nat
ASSUME Healthy \in BOOLEAN /\ RevokeCheck \in BOOLEAN
ASSUME Mutation \in {"none", "startBeforeClean", "readOld", "stopKeepsRetry",
                     "lateHello", "waitForOld"}

VARIABLES
    lst,      \* lst[c]: "connecting" | "greeting" | "ready" | "waiting" | "stopped"
    cur,      \* cur[c]: number of c's current connection, 0 before the first
    ndial,    \* ndial[c]: connections c dialed
    sent,     \* sent[c]: rows c loaded on its current session
    revoked,  \* revoked[c]: c's identity revoked the runtime (Revocations)
    stopped,  \* stopped[c]: c's link was disposed
    cs,       \* cs[k]: controller side of k: "none" | "open" | "closed"
    toR,      \* toR[k]: frames sent on k to the runtime, not read yet
    toC,      \* toC[k]: frames sent on k to the controller, not read yet
    rs,       \* rs[k]: runtime side: "none" | "queued" | "serving" | "closed" | "done"
    accq,     \* connections the listener accepted, not taken yet
    rcur,     \* connection whose session the runtime runs (or replaces)
    rnew,     \* the connection a takeover is for
    rphase,   \* "idle" | "closing" | "cleaning" | "starting" (and mutations')
    rows,     \* rows loaded on the runtime: [lease, by]
    ghost     \* observations for the invariants

vars == <<lst, cur, ndial, sent, revoked, stopped, cs, toR, toC, rs, accq,
          rcur, rnew, rphase, rows, ghost>>
ctl == <<lst, cur, ndial, sent, revoked, stopped>>
rt == <<rs, accq, rcur, rnew, rphase, rows>>

Conns == Controllers \X (1..MaxConns)
NoConn == <<"none", 0>>
Cur(c) == <<c, cur[c]>>

\* Whether each side still reads k.
ReadsC(k) == cs[k] = "open" \/ Mutation = "lateHello"
ReadsR(k) == rs[k] \in {"none", "queued", "serving"} \/ Mutation = "readOld"

(***************************************************************************)
(* Controller: link.rs `dialing`.                                          *)
(***************************************************************************)
\* Close c's side of k: the session's close reaches the runtime; nothing
\* more is read on k.
CloseC(k) ==
    /\ cs' = [cs EXCEPT ![k] = "closed"]
    /\ toR' = [toR EXCEPT ![k] = IF ReadsR(k) THEN Append(@, "close") ELSE @]
    /\ toC' = [toC EXCEPT ![k] = IF Mutation = "lateHello" THEN @ ELSE <<>>]

\* Top of the dial loop. A revoked far end is not dialed: an authentication
\* failure, waited out (link.rs `is_revoked`; `Revocable::credential` also
\* presents nothing, so the runtime would answer 401).
Dial(c) ==
    /\ lst[c] = "connecting"
    /\ IF revoked[c]
       THEN /\ lst' = [lst EXCEPT ![c] = "waiting"]
            /\ UNCHANGED <<cur, ndial, sent, cs, rs, accq, ghost>>
       ELSE /\ ndial[c] < MaxConns
            /\ LET n == ndial[c] + 1
                   k == <<c, n>>
               IN /\ ndial' = [ndial EXCEPT ![c] = n]
                  /\ cur' = [cur EXCEPT ![c] = n]
                  /\ sent' = [sent EXCEPT ![c] = 0]
                  \* The upgrade succeeded (the runtime's token check): the
                  \* runtime's listener queues the connection; the link
                  \* opens its session (`Greet::open`; its own hello is
                  \* read by whichever session the runtime starts on k, and
                  \* is left out).
                  /\ cs' = [cs EXCEPT ![k] = "open"]
                  /\ rs' = [rs EXCEPT ![k] = "queued"]
                  /\ accq' = Append(accq, k)
                  /\ lst' = [lst EXCEPT ![c] = "greeting"]
                  /\ ghost' = [ghost EXCEPT !.dialAfterStop = @ \/ stopped[c]]
    /\ UNCHANGED <<revoked, stopped, toR, toC, rcur, rnew, rphase, rows>>

\* The dial itself fails (connection refused, network down): retryable.
DialFails(c) ==
    /\ ~Healthy
    /\ lst[c] = "connecting"
    /\ ~revoked[c]
    /\ ndial[c] < MaxConns
    /\ ndial' = [ndial EXCEPT ![c] = @ + 1]
    /\ lst' = [lst EXCEPT ![c] = "waiting"]
    /\ ghost' = [ghost EXCEPT !.dialAfterStop = @ \/ stopped[c]]
    /\ UNCHANGED <<cur, sent, revoked, stopped, cs, toR, toC, rt>>

\* The wait (backoff, or 30 s after an authentication failure) is over.
Retry(c) ==
    /\ lst[c] = "waiting"
    /\ lst' = [lst EXCEPT ![c] = "connecting"]
    /\ UNCHANGED <<cur, ndial, sent, revoked, stopped, cs, toR, toC, rt, ghost>>

\* Read the next frame of connection n. A closed session reads nothing: a
\* handshake result arriving after the link gave up on it goes with the
\* session (the `lateHello` mutation reads it anyway).
CRecv(c, n) ==
    LET k == <<c, n>>
        f == Head(toC[k])
    IN
    /\ toC[k] /= <<>>
    /\ ReadsC(k)
    /\ CASE f = "hello" /\ cs[k] = "open" /\ lst[c] = "greeting" /\ cur[c] = n ->
              \* `start`: the far end greeted; the peer is provided.
              IF RevokeCheck /\ revoked[c]
              THEN /\ CloseC(k)
                   /\ lst' = [lst EXCEPT ![c] = "waiting"]
                   /\ UNCHANGED <<cur, ghost>>
              ELSE /\ toC' = [toC EXCEPT ![k] = Tail(@)]
                   /\ lst' = [lst EXCEPT ![c] = "ready"]
                   /\ ghost' = [ghost EXCEPT !.revokedReady = @ \/ revoked[c]]
                   /\ UNCHANGED <<cs, toR, cur>>
         [] f = "hello" /\ cs[k] = "closed" /\ lst[c] \in {"connecting", "waiting"} ->
              \* lateHello: a late greeting revives a session given up on.
              /\ toC' = [toC EXCEPT ![k] = Tail(@)]
              /\ lst' = [lst EXCEPT ![c] = "ready"]
              /\ cur' = [cur EXCEPT ![c] = n]
              /\ UNCHANGED <<cs, toR, ghost>>
         [] f = "close" /\ cs[k] = "open" ->
              \* The session ended (replaced: 4002, or the runtime ended
              \* it): retryable; the peer is withdrawn.
              /\ CloseC(k)
              /\ lst' = [lst EXCEPT ![c] = IF cur[c] = n /\ @ \in {"greeting", "ready"}
                                           THEN "waiting" ELSE @]
              /\ UNCHANGED <<cur, ghost>>
         [] OTHER ->
              /\ toC' = [toC EXCEPT ![k] = Tail(@)]
              /\ UNCHANGED <<lst, cur, cs, toR, ghost>>
    /\ UNCHANGED <<ndial, sent, revoked, stopped, rt>>

\* The loader, through the peer, loads a row on the session.
SendLoad(c) ==
    LET k == Cur(c) IN
    /\ lst[c] = "ready"
    /\ cs[k] = "open"
    /\ sent[c] < MaxLoads
    /\ sent' = [sent EXCEPT ![c] = @ + 1]
    /\ toR' = [toR EXCEPT ![k] = IF ReadsR(k) THEN Append(@, "load") ELSE @]
    /\ UNCHANGED <<lst, cur, ndial, revoked, stopped, cs, toC, rt, ghost>>

\* The handshake times out, or the connection is lost: retryable.
Lose(c) ==
    /\ ~Healthy
    /\ lst[c] \in {"greeting", "ready"}
    /\ cs[Cur(c)] = "open"
    /\ CloseC(Cur(c))
    /\ lst' = [lst EXCEPT ![c] = "waiting"]
    /\ UNCHANGED <<cur, ndial, sent, revoked, stopped, rt, ghost>>

\* The identity of c revokes the runtime (`Revocations::revoke`).
Revoke(c) ==
    /\ ~Healthy
    /\ ~revoked[c]
    /\ revoked' = [revoked EXCEPT ![c] = TRUE]
    /\ UNCHANGED <<lst, cur, ndial, sent, stopped, cs, toR, toC, rt, ghost>>

\* The ready session sees the revocation (`select!` on `revoked()`): `cut`
\* closes it, then the link waits as after an authentication failure.
Cut(c) ==
    /\ lst[c] = "ready"
    /\ revoked[c]
    /\ cs[Cur(c)] = "open"
    /\ CloseC(Cur(c))
    /\ lst' = [lst EXCEPT ![c] = "waiting"]
    /\ UNCHANGED <<cur, ndial, sent, revoked, stopped, rt, ghost>>

\* The link is disposed: its task is aborted; dropping it closes the
\* session and cancels the retry.
Stop(c) ==
    /\ ~Healthy
    /\ ~stopped[c]
    /\ stopped' = [stopped EXCEPT ![c] = TRUE]
    /\ IF cur[c] > 0 /\ cs[Cur(c)] = "open"
       THEN CloseC(Cur(c)) ELSE UNCHANGED <<cs, toR, toC>>
    /\ lst' = [lst EXCEPT ![c] =
                 IF Mutation = "stopKeepsRetry" /\ @ = "waiting" THEN @ ELSE "stopped"]
    /\ UNCHANGED <<cur, ndial, sent, revoked, rt, ghost>>

(***************************************************************************)
(* Runtime: one controller at a time; a newer connection takes over        *)
(* (§4.4): close the old session's channel and read none of its frames ->  *)
(* clean the old lease -> answer the new hello.                            *)
(***************************************************************************)
SendC(k, f) == toC' = [toC EXCEPT ![k] = IF ReadsC(k) THEN Append(@, f) ELSE @]

\* Stop reading k: what is still on its way is dropped.
Unread(k) == toR' = [toR EXCEPT ![k] = IF Mutation = "readOld" THEN @ ELSE <<>>]

StartSession(k) ==
    /\ rs' = [rs EXCEPT ![k] = "serving"]
    /\ SendC(k, "hello")

TakeNext ==
    /\ rphase = "idle"
    /\ accq /= <<>>
    /\ accq' = Tail(accq)
    /\ LET k == Head(accq) IN
       IF rcur = NoConn
       THEN /\ StartSession(k)
            /\ rcur' = k
            /\ UNCHANGED <<rnew, rphase>>
       ELSE /\ rnew' = k
            /\ rphase' = "closing"
            /\ UNCHANGED <<rs, toC, rcur>>
    /\ UNCHANGED <<ctl, cs, toR, rows, ghost>>

\* 1. Close the old channel (4002, "replaced"); no frame of it is read any
\* more. waitForOld: stop reading, but leave closing to the old controller.
TakeoverClose ==
    /\ rphase = "closing"
    /\ rs' = [rs EXCEPT ![rcur] = "closed"]
    /\ IF Mutation = "waitForOld" THEN UNCHANGED toC ELSE SendC(rcur, "close")
    /\ Unread(rcur)
    /\ rphase' = IF Mutation = "startBeforeClean" THEN "startEarly" ELSE "cleaning"
    /\ UNCHANGED <<ctl, cs, accq, rcur, rnew, rows, ghost>>

\* 2. Clean the old lease: every row and proxy of the old session goes.
TakeoverClean ==
    /\ rphase = "cleaning"
    /\ Mutation = "waitForOld" => cs[rcur] = "closed"
    /\ rows' = {r \in rows : r.lease /= rcur}
    /\ rs' = [rs EXCEPT ![rcur] = "done"]
    /\ rphase' = "starting"
    /\ UNCHANGED <<ctl, cs, toR, toC, accq, rcur, rnew, ghost>>

\* 3. Only then the new session starts and answers its hello.
TakeoverStart ==
    /\ rphase = "starting"
    /\ StartSession(rnew)
    /\ rcur' = rnew
    /\ rnew' = NoConn
    /\ rphase' = "idle"
    /\ UNCHANGED <<ctl, cs, toR, accq, rows, ghost>>

\* startBeforeClean: steps 2 and 3 swapped.
StartEarly ==
    /\ rphase = "startEarly"
    /\ StartSession(rnew)
    /\ rphase' = "cleanLate"
    /\ UNCHANGED <<ctl, cs, toR, accq, rcur, rnew, rows, ghost>>

CleanLate ==
    /\ rphase = "cleanLate"
    /\ rows' = {r \in rows : r.lease /= rcur}
    /\ rs' = [rs EXCEPT ![rcur] = "done"]
    /\ rcur' = rnew
    /\ rnew' = NoConn
    /\ rphase' = "idle"
    /\ UNCHANGED <<ctl, cs, toR, toC, accq, ghost>>

\* The runtime reads the next frame of a session it serves. readOld: it
\* still reads a replaced session, and applies what it reads to the rows
\* it now serves (as one shared row table would).
RRead(k) ==
    LET f == Head(toR[k]) IN
    /\ toR[k] /= <<>>
    /\ \/ rs[k] = "serving"
       \/ Mutation = "readOld" /\ rs[k] \in {"closed", "done"}
    /\ CASE f = "load" ->
              LET target == IF rs[k] = "serving" THEN k ELSE rcur IN
              /\ target \in Conns /\ rs[target] = "serving"
              /\ toR' = [toR EXCEPT ![k] = Tail(@)]
              /\ rows' = rows \cup {[lease |-> target, by |-> k]}
              /\ UNCHANGED <<rs, rcur>>
         [] f = "close" ->
              \* The session ended: its lease is cleaned up (`end`). While a
              \* takeover runs, the loop is busy; it closes this one anyway.
              /\ rs[k] = "serving" /\ rphase = "idle" /\ rcur = k
              /\ toR' = [toR EXCEPT ![k] = <<>>]
              /\ rows' = {r \in rows : r.lease /= k}
              /\ rs' = [rs EXCEPT ![k] = "done"]
              /\ rcur' = NoConn
    /\ UNCHANGED <<ctl, cs, toC, accq, rnew, rphase, ghost>>

(***************************************************************************)
Init ==
    /\ lst = [c \in Controllers |-> "connecting"]
    /\ cur = [c \in Controllers |-> 0]
    /\ ndial = [c \in Controllers |-> 0]
    /\ sent = [c \in Controllers |-> 0]
    /\ revoked = [c \in Controllers |-> FALSE]
    /\ stopped = [c \in Controllers |-> FALSE]
    /\ cs = [k \in Conns |-> "none"]
    /\ toR = [k \in Conns |-> <<>>]
    /\ toC = [k \in Conns |-> <<>>]
    /\ rs = [k \in Conns |-> "none"]
    /\ accq = <<>>
    /\ rcur = NoConn
    /\ rnew = NoConn
    /\ rphase = "idle"
    /\ rows = {}
    /\ ghost = [revokedReady |-> FALSE, dialAfterStop |-> FALSE]

Controller(c) ==
    \/ Dial(c) \/ DialFails(c) \/ Retry(c) \/ SendLoad(c) \/ Lose(c)
    \/ Revoke(c) \/ Cut(c) \/ Stop(c)
    \/ \E n \in 1..MaxConns : CRecv(c, n)

Runtime ==
    \/ TakeNext \/ TakeoverClose \/ TakeoverClean \/ TakeoverStart
    \/ StartEarly \/ CleanLate
    \/ \E k \in Conns : RRead(k)

Next == Runtime \/ \E c \in Controllers : Controller(c)

\* Liveness is checked only on Healthy runs, where the environment
\* (revocation, stop, timeout, loss) does nothing and every behaviour is
\* finite, so weak fairness of Next makes every enabled step happen.
Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

\* Controllers are interchangeable (safety configurations only: TLC does
\* not combine symmetry with liveness checking).
Symmetry == Permutations(Controllers)

(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)
\* The runtime holds a lease for k: k's session runs, or rows of k remain.
Leased(k) == rs[k] = "serving" \/ \E r \in rows : r.lease = k

\* At most one active lease per runtime instance.
OneLease == Cardinality({k \in Conns : Leased(k)}) <= 1

\* A command of one session never acts on another session's lease.
OldCommandsStayOld == \A r \in rows : r.by = r.lease

\* A stopped link never dials again and keeps no session.
StoppedStaysDown ==
    /\ ~ghost.dialAfterStop
    /\ \A c \in Controllers : stopped[c] =>
          \A n \in 1..MaxConns : cs[<<c, n>>] /= "open"

\* A controller is ready only on its current, open connection: a late
\* handshake result never revives a session the link gave up on.
ReadyIsCurrent ==
    \A c \in Controllers : lst[c] = "ready" => cur[c] > 0 /\ cs[Cur(c)] = "open"

\* Once c's identity revoked the runtime, c never becomes ready again (the
\* peer is not provided, so nothing is mounted or loaded for it).
RevokedStaysOut == ~ghost.revokedReady

\* Liveness: some controller is ready on the session the runtime serves.
Held == \E c \in Controllers : lst[c] = "ready" /\ rs[Cur(c)] = "serving"
EventuallyHeld == <>Held
EndsHeld == []<>Held
==============================================================================
