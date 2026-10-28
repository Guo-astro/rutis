----------------------------- MODULE AccessPoint -----------------------------
(***************************************************************************)
(* Runtimes dialing out to a rutis access point: the listener's            *)
(* registration and its generation, hand-over, admission of a self-        *)
(* declared id, takeover by a newer connection of the same id (one         *)
(* instance per id), and the access point stopping and starting again     *)
(* (registration revoked and renewed).                                     *)
(*                                                                         *)
(* Design: docs/design-remote-plugins-2026-10-03.md §3.2 (registrations,   *)
(* generation re-checked at hand-over); docs/design-remote-runtime-dial-   *)
(* 2026-10-14.md §四 (connection = instance), §五 (identity layer), §六     *)
(* (dial and retry rules, takeover by id).                                 *)
(* Implementation modelled: crates/rutis-bridge/src/registration.rs         *)
(* (`route`, `hand_over`), access_point.rs (`Shared::connection`, slots);   *)
(* the dialers:                                                            *)
(* python/rutis/rutis/dialing.py `keep_dialing` (and its Go, Node, Bun      *)
(* twins). README.md next to this file lists the simplifications.          *)
(*                                                                         *)
(* Connection <<d, n>> is the n-th connection dialer d made; d greets with *)
(* the id IdOf(d). Each direction of a connection is FIFO; frames are      *)
(* delayed arbitrarily long. A side that closed a connection reads nothing *)
(* more on it: frames to it are dropped (kept only where a mutation reads  *)
(* them).                                                                  *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Dialers,      \* runtimes dialing out (model values)
    SameId,       \* TRUE: every dialer greets with the same id
    MaxConns,     \* connections each dialer may make (1 + reconnects)
    MaxCalls,     \* calls a dialer makes on one session
    MaxRegs,      \* registrations of the access point over the run
    Faults,       \* what the environment may do: "lose" (no greeting in
                  \* time, lost connection, failed dial), "stop" (a runtime
                  \* stops), "registration" (the access point stops and
                  \* starts again)
    Mutation      \* "none", or a deliberate bug (README, "Mutations")

ASSUME SameId \in BOOLEAN
ASSUME MaxConns \in Nat \ {0} /\ MaxCalls \in Nat /\ MaxRegs \in Nat \ {0}
ASSUME Faults \subseteq {"lose", "stop", "registration"}
ASSUME Mutation \in {"none", "acceptBeforeRelease", "routeById", "noGenCheck",
                     "stopKeepsRetry"}

\* The id a dialer greets with: its own name, or one shared by all.
IdOf(d) == IF SameId THEN "one" ELSE d
Ids == {IdOf(d) : d \in Dialers}
Conns == Dialers \X (1..MaxConns)
Id(k) == IdOf(k[1])
NoConn == <<"none", 0>>

VARIABLES
    reg,        \* generation of the access point's registration; 0: none
    regs,       \* registrations made so far
    dst,        \* dst[d]: "connecting" | "greeting" | "ready" | "waiting" | "stopped"
    dcur,       \* dcur[d]: number of d's current connection, 0 before the first
    ndial,      \* ndial[d]: connections d made
    calls,      \* calls[d]: calls d made on its current session
    dstopped,   \* dstopped[d]: d was stopped (SIGTERM, `stop()`)
    ds,         \* ds[k]: dialer side of k: "none" | "open" | "closed"
    toD,        \* toD[k]: frames to the dialer: "hello" | "close"
    toAP,       \* toAP[k]: frames to the access point: "call" | "close"
    apc,        \* apc[k]: access point side of k: [st, gen, closed]
    order,      \* connections handed over and not ended, oldest first
    lock,       \* lock[i]: connection holding the slot of id i
    inst,       \* inst[i]: the current connection (instance) of id i
    applied,    \* calls the access point applied: [inst, by]
    ghost       \* observations for the invariants

vars == <<reg, regs, dst, dcur, ndial, calls, dstopped,
          ds, toD, toAP, apc, order, lock, inst, applied, ghost>>
dialer == <<dst, dcur, ndial, calls, dstopped, ds>>
env == <<reg, regs>>

\* apc[k].st: "none"; "routed" (has a ticket of generation gen, not handed
\* over); "admitted" (handed over, greeted, its id admitted); "accepting"
\* (holds its id's slot, the old connection gone); "acceptEarly"
\* (mutation); "live" (accepted: an instance); "ended" (closed, released;
\* the rest forgotten). apc[k].closed: the access point read k's close.
Ended == [st |-> "ended", gen |-> 0, closed |-> FALSE]
\* Where k is in `order`; and forgetting ended connections.
Pos(k) == CHOOSE p \in 1..Len(order) : order[p] = k
Forget(ks) == order' = SelectSeq(order, LAMBDA x : x \notin ks)
St(k) == apc[k].st
DCur(d) == <<d, dcur[d]>>

\* Whether each side still reads k.
ReadsD(k) == ds[k] = "open"
ReadsAP(k) == St(k) /= "ended" \/ Mutation = "routeById"

(***************************************************************************)
(* Dialers: the rules of §六 (dialing.py `keep_dialing`).                  *)
(***************************************************************************)
\* Close the dialer's side of k: its rows go; the close reaches rutis.
CloseD(k) ==
    /\ ds' = [ds EXCEPT ![k] = "closed"]
    /\ toAP' = [toAP EXCEPT ![k] = IF ReadsAP(k) THEN Append(@, "close") ELSE @]
    /\ toD' = [toD EXCEPT ![k] = <<>>]

\* Dial: the transport routes the upgrade by its credential
\* (`Registrations::route`): nothing registered on the listener is
\* retryable; otherwise the connection gets a ticket of the current
\* registration generation.
DoDial(d) ==
    /\ dst[d] = "connecting"
    /\ ndial[d] < MaxConns
    /\ LET n == ndial[d] + 1
           k == <<d, n>>
       IN /\ ndial' = [ndial EXCEPT ![d] = n]
          /\ ghost' = [ghost EXCEPT !.dialAfterStop = @ \/ dstopped[d]]
          /\ IF reg = 0
             THEN /\ dst' = [dst EXCEPT ![d] = "waiting"]
                  /\ UNCHANGED <<dcur, calls, ds, apc>>
             ELSE /\ dcur' = [dcur EXCEPT ![d] = n]
                  /\ calls' = [calls EXCEPT ![d] = 0]
                  /\ ds' = [ds EXCEPT ![k] = "open"]
                  /\ apc' = [apc EXCEPT ![k] = [@ EXCEPT !.st = "routed", !.gen = reg]]
                  /\ dst' = [dst EXCEPT ![d] = "greeting"]
    /\ UNCHANGED <<env, dstopped, toD, toAP, order, lock, inst, applied>>

DialFails(d) ==
    /\ "lose" \in Faults
    /\ dst[d] = "connecting"
    /\ ndial[d] < MaxConns
    /\ ndial' = [ndial EXCEPT ![d] = @ + 1]
    /\ dst' = [dst EXCEPT ![d] = "waiting"]
    /\ ghost' = [ghost EXCEPT !.dialAfterStop = @ \/ dstopped[d]]
    /\ UNCHANGED <<env, dcur, calls, dstopped, ds, toD, toAP, apc, order,
                   lock, inst, applied>>

Retry(d) ==
    /\ dst[d] = "waiting"
    /\ dst' = [dst EXCEPT ![d] = "connecting"]
    /\ UNCHANGED <<env, dcur, ndial, calls, dstopped, ds, toD, toAP, apc,
                   order, lock, inst, applied, ghost>>

\* Read the next frame of an open connection: rutis' hello (connected), or
\* the close (replaced 4002 and lost are retryable, a rejected id 4003 is
\* an authentication failure: both wait, then dial again).
DRecv(d, n) ==
    LET k == <<d, n>>
        f == Head(toD[k])
    IN
    /\ toD[k] /= <<>>
    /\ ReadsD(k)
    /\ IF f = "hello"
       THEN /\ toD' = [toD EXCEPT ![k] = Tail(@)]
            /\ dst' = [dst EXCEPT ![d] = IF @ = "greeting" /\ dcur[d] = n THEN "ready" ELSE @]
            /\ UNCHANGED <<ds, toAP>>
       ELSE /\ CloseD(k)
            /\ dst' = [dst EXCEPT ![d] = IF dcur[d] = n /\ @ \in {"greeting", "ready"}
                                         THEN "waiting" ELSE @]
    /\ UNCHANGED <<env, dcur, ndial, calls, dstopped, apc, order, lock,
                   inst, applied, ghost>>

\* A ready runtime calls rutis on its session.
SendCall(d) ==
    LET k == DCur(d) IN
    /\ dst[d] = "ready"
    /\ ds[k] = "open"
    /\ calls[d] < MaxCalls
    /\ calls' = [calls EXCEPT ![d] = @ + 1]
    /\ toAP' = [toAP EXCEPT ![k] = IF ReadsAP(k) THEN Append(@, "call") ELSE @]
    /\ UNCHANGED <<env, dst, dcur, ndial, dstopped, ds, toD, apc, order,
                   lock, inst, applied, ghost>>

\* No greeting in time, or the connection is lost: retryable.
Lose(d) ==
    /\ "lose" \in Faults
    /\ dst[d] \in {"greeting", "ready"}
    /\ ds[DCur(d)] = "open"
    /\ CloseD(DCur(d))
    /\ dst' = [dst EXCEPT ![d] = "waiting"]
    /\ UNCHANGED <<env, dcur, ndial, calls, dstopped, apc, order, lock,
                   inst, applied, ghost>>

\* Stopped (SIGTERM): the rows go, the connection closes, no more dials.
StopDialer(d) ==
    /\ "stop" \in Faults
    /\ ~dstopped[d]
    /\ dstopped' = [dstopped EXCEPT ![d] = TRUE]
    /\ IF dcur[d] > 0 /\ ds[DCur(d)] = "open"
       THEN CloseD(DCur(d)) ELSE UNCHANGED <<ds, toAP, toD>>
    /\ dst' = [dst EXCEPT ![d] =
                 IF Mutation = "stopKeepsRetry" /\ @ = "waiting" THEN @ ELSE "stopped"]
    /\ UNCHANGED <<env, dcur, ndial, calls, apc, order, lock, inst,
                   applied, ghost>>

(***************************************************************************)
(* rutis: the transport, the registration, the access point.               *)
(***************************************************************************)
\* Send to the dialer of k: `closer.replaced` (4002), `rejected` (4003),
\* the session's close, or rutis' hello.
SendD(k, f) == toD' = [toD EXCEPT ![k] = IF ReadsD(k) THEN Append(@, f) ELSE @]

\* The access point is done with k: it closes it and reads no more of it.
EndAP(k) ==
    /\ SendD(k, "close")
    /\ toAP' = [toAP EXCEPT ![k] = IF Mutation = "routeById" THEN @ ELSE <<>>]

\* `Registrations::hand_over`, under the registry lock: deliver only to the
\* registration the ticket was routed to (same generation); otherwise the
\* connection is closed. The access point then greets it, and the
\* admission admits the id its hello names (`Admitting`).
\* noGenCheck: deliver to whatever is registered now. (Hand-over and the
\* greeting are one step: the access point stopping in between is the same
\* as stopping just after, which `TakeSlot` handles.)
HandOver(k) ==
    /\ St(k) = "routed"
    /\ IF \/ reg = apc[k].gen
          \/ Mutation = "noGenCheck" /\ reg /= 0
       THEN /\ apc' = [apc EXCEPT ![k] = [st |-> "admitted", gen |-> reg, closed |-> FALSE]]
            /\ order' = Append(order, k)
            /\ SendD(k, "hello")
            /\ ghost' = [ghost EXCEPT !.staleHandOver = @ \/ reg /= apc[k].gen]
            /\ UNCHANGED toAP
       ELSE /\ apc' = [apc EXCEPT ![k] = Ended]
            /\ EndAP(k)
            /\ UNCHANGED <<order, ghost>>
    /\ UNCHANGED <<env, dialer, lock, inst, applied>>

\* The slot of the id is locked: takeovers of one id run one at a time. A
\* connection that arrived before the current one is superseded (4002);
\* one whose access point stopped is closed. Otherwise the current
\* connection, if any, is closed as replaced (4002) and released: its
\* instance goes before the new one is accepted. acceptBeforeRelease:
\* closed, but released only after the new one is accepted.
TakeSlot(k) ==
    LET i == Id(k)
        old == inst[i]
    IN
    /\ St(k) = "admitted"
    /\ lock[i] = NoConn
    /\ IF \/ reg /= apc[k].gen
          \/ old /= NoConn /\ Pos(old) > Pos(k)
       THEN /\ apc' = [apc EXCEPT ![k] = Ended]
            /\ EndAP(k)
            /\ Forget({k})
            /\ UNCHANGED <<lock, inst>>
       ELSE /\ lock' = [lock EXCEPT ![i] = k]
            /\ IF old = NoConn
               THEN /\ apc' = [apc EXCEPT ![k].st = "accepting"]
                    /\ UNCHANGED <<toD, toAP, inst, order>>
               ELSE IF Mutation = "acceptBeforeRelease"
               THEN /\ apc' = [apc EXCEPT ![k].st = "acceptEarly"]
                    /\ SendD(old, "close")
                    /\ UNCHANGED <<toAP, inst, order>>
               ELSE /\ apc' = [apc EXCEPT ![k].st = "accepting", ![old] = Ended]
                    /\ EndAP(old)
                    /\ Forget({old})
                    /\ inst' = [inst EXCEPT ![i] = NoConn]
    /\ UNCHANGED <<env, dialer, applied, ghost>>

\* `Accept::accept`: the instance is created, the runtime mounted on the
\* session.
Accept(k) ==
    LET i == Id(k) IN
    /\ St(k) = "accepting"
    /\ lock' = [lock EXCEPT ![i] = NoConn]
    /\ apc' = [apc EXCEPT ![k].st = "live"]
    /\ inst' = [inst EXCEPT ![i] = k]
    /\ UNCHANGED <<env, dialer, toD, toAP, order, applied, ghost>>

\* acceptBeforeRelease: the new instance is accepted, then (ReleaseLate)
\* the old one released.
AcceptEarly(k) ==
    LET i == Id(k) IN
    /\ St(k) = "acceptEarly"
    /\ apc' = [apc EXCEPT ![k].st = "live"]
    /\ inst' = [inst EXCEPT ![i] = k]
    /\ lock' = [lock EXCEPT ![i] = NoConn]
    /\ UNCHANGED <<env, dialer, toD, toAP, order, applied, ghost>>

ReleaseLate(k) ==
    /\ St(k) = "live"
    /\ inst[Id(k)] /= k
    /\ apc' = [apc EXCEPT ![k] = Ended]
    /\ toAP' = [toAP EXCEPT ![k] = <<>>]
    /\ Forget({k})
    /\ UNCHANGED <<env, dialer, toD, lock, inst, applied, ghost>>

\* The access point reads a frame of k. A call acts on the instance of the
\* session it came on. routeById: a replaced session is still read, and
\* its calls go to the current instance of its id.
APRecv(k) ==
    LET f == Head(toAP[k])
        target == IF St(k) = "live" THEN k ELSE inst[Id(k)]
    IN
    /\ toAP[k] /= <<>>
    /\ \/ St(k) = "live"
       \/ Mutation = "routeById" /\ St(k) = "ended"
    /\ toAP' = [toAP EXCEPT ![k] = Tail(@)]
    /\ IF f = "call"
       THEN /\ target /= NoConn /\ St(target) = "live"
            /\ applied' = applied \cup {[inst |-> target, by |-> k]}
            /\ UNCHANGED apc
       ELSE /\ apc' = [apc EXCEPT ![k].closed = St(k) = "live"]
            /\ UNCHANGED applied
    /\ UNCHANGED <<env, dialer, toD, order, lock, inst, ghost>>

\* A live connection ends: its session closed, or its access point
\* stopped. Under the slot's lock, if it is still current: closed, then
\* released.
EndLive(k) ==
    LET i == Id(k) IN
    /\ St(k) = "live"
    /\ lock[i] = NoConn
    /\ inst[i] = k
    /\ apc[k].closed \/ reg /= apc[k].gen
    /\ apc' = [apc EXCEPT ![k] = Ended]
    /\ inst' = [inst EXCEPT ![i] = NoConn]
    /\ EndAP(k)
    /\ Forget({k})
    /\ UNCHANGED <<env, dialer, lock, applied, ghost>>

\* The access point stops: its registration is revoked (dropped).
StopAP ==
    /\ "registration" \in Faults
    /\ reg /= 0
    /\ reg' = 0
    /\ UNCHANGED <<regs, dialer, toD, toAP, apc,
                   order, lock, inst, applied, ghost>>

\* It starts again: a new registration, a new generation.
StartAP ==
    /\ "registration" \in Faults
    /\ reg = 0
    /\ regs < MaxRegs
    /\ regs' = regs + 1
    /\ reg' = regs + 1
    /\ UNCHANGED <<dialer, toD, toAP, apc, order, lock, inst, applied, ghost>>

(***************************************************************************)
Init ==
    /\ reg = 1
    /\ regs = 1
    /\ dst = [d \in Dialers |-> "connecting"]
    /\ dcur = [d \in Dialers |-> 0]
    /\ ndial = [d \in Dialers |-> 0]
    /\ calls = [d \in Dialers |-> 0]
    /\ dstopped = [d \in Dialers |-> FALSE]
    /\ ds = [k \in Conns |-> "none"]
    /\ toD = [k \in Conns |-> <<>>]
    /\ toAP = [k \in Conns |-> <<>>]
    /\ apc = [k \in Conns |-> [st |-> "none", gen |-> 0, closed |-> FALSE]]
    /\ order = <<>>
    /\ lock = [i \in Ids |-> NoConn]
    /\ inst = [i \in Ids |-> NoConn]
    /\ applied = {}
    /\ ghost = [dialAfterStop |-> FALSE, staleHandOver |-> FALSE]

Dialer(d) ==
    \/ DoDial(d) \/ DialFails(d) \/ Retry(d) \/ SendCall(d) \/ Lose(d)
    \/ StopDialer(d)
    \/ \E n \in 1..MaxConns : DRecv(d, n)

Rutis ==
    \/ \E k \in Conns :
          \/ HandOver(k) \/ TakeSlot(k) \/ Accept(k) \/ AcceptEarly(k) \/ ReleaseLate(k) \/ APRecv(k)
          \/ EndLive(k)
    \/ StopAP \/ StartAP

Next == Rutis \/ \E d \in Dialers : Dialer(d)

\* Liveness is checked only without faults: every behaviour is finite
\* there, so weak fairness of Next makes every enabled step happen.
Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

\* Dialers are interchangeable (safety configurations only).
Symmetry == Permutations(Dialers)

(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)
\* At most one instance (lease) per connection id.
OneInstancePerId ==
    \A i \in Ids : Cardinality({k \in Conns : St(k) = "live" /\ Id(k) = i}) <= 1

\* A call of one session never acts on another session's instance.
OldCallsStayOld == \A a \in applied : a.inst = a.by

\* A handshake routed to a registration that was revoked since (the access
\* point stopped, perhaps started again) never completes hand-over.
NoStaleHandOver == ~ghost.staleHandOver

\* A stopped runtime never dials again and keeps no connection open.
StoppedStaysDown ==
    /\ ~ghost.dialAfterStop
    /\ \A d \in Dialers : dstopped[d] =>
          \A n \in 1..MaxConns : ds[<<d, n>>] /= "open"

\* Liveness: each id has an instance whose runtime is connected to it.
Held(i) == \E k \in Conns :
              /\ Id(k) = i /\ St(k) = "live"
              /\ dst[k[1]] = "ready" /\ dcur[k[1]] = k[2]
EventuallyHeld == \A i \in Ids : <>Held(i)
EndsHeld == \A i \in Ids : []<>Held(i)
==============================================================================
