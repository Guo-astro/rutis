--------------------------- MODULE CrossRuntimeSync ---------------------------
(***************************************************************************)
(* Synchronous calls between language runtimes, relayed by the Rust host.  *)
(*                                                                         *)
(* Design: docs/design-multilang-runtimes-2026-10-03.md §五 (forwarding,   *)
(* path rewriting) and §九 (deadlock policy);                              *)
(* docs/design-multilang-m1-2026-10-04.md §七 (relays, `rebase`).           *)
(* Implementation modelled: crates/rutis-bridge/src/session/rpc.rs,         *)
(* rpc/waits.rs (its six operations), events/hub.rs (synchronous and       *)
(* asynchronous dispatch), runtime/rows.rs (Rust); node/rutis-runtime/     *)
(* src/session.mjs (Node, non-reentrant); python/rutis/rutis/peer.py        *)
(* (Python, reentrant); Go, Swift, Kotlin (a thread or task per call:       *)
(* `Threaded`). The host is a participant too: Rust code (a service call,   *)
(* the host's own call, a synchronous dispatch) runs on `Workers` threads,  *)
(* and an asynchronous dispatch waits inside such an execution.             *)
(* README.md next to this file lists the simplifications and results.      *)
(*                                                                         *)
(* Every runtime has one session with Rust; each direction of a session is *)
(* FIFO. A stacked runtime is single-threaded: a synchronous call blocks   *)
(* its thread, which then pumps incoming frames and runs some of them      *)
(* nested on the same stack. A threaded runtime (Go, Swift, Kotlin) runs    *)
(* every incoming call on its own thread or task: nothing is put aside and *)
(* its stack is not LIFO. Rust is a relay: a call from runtime s to a       *)
(* service (or function) owned by runtime t is forwarded on t's session     *)
(* with the call chain `path` rebased.                                      *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Runtimes,   \* runtime sessions (model values)
    Reentrant,  \* runtimes that run every incoming call while waiting
    Policy,     \* "current" | "A" | "B" | "Bchain" (B without stack order)
    Rebase,     \* TRUE: Rust rewrites `path` as rpc.rs `rebase`; FALSE: verbatim
    MaxRoots,   \* top-level activations (timers, events) over the whole run
    MaxDepth,   \* hops in one call chain
    MaxCalls,   \* calls one activation makes, one after another
    MaxPosts,   \* emits and synchronous dispatches over the whole run
    Threaded,   \* runtimes with a thread (or task) per incoming call: Go, Swift, Kotlin
    Declared,   \* runtimes whose far end declares `sync-wait`; others are outside the check
    EmitChain,  \* TRUE: an emit's listener call carries the emitter's chain; FALSE: it does not
    DispatchMode,\* "none" | "bail" | "waterfall": synchronous event dispatch
    DispatchHop,\* TRUE: Rust records the dispatch as a forwarded call; FALSE: it does not
    HostRoot,   \* TRUE: the host makes its own synchronous calls (a Rust plugin's)
    HostHop,    \* TRUE: such a call with no chain is a hop (#597); FALSE: an entry
    Abandoners, \* runtimes that give up a synchronous call locally, without `cancel`
    Hidden,     \* runtimes that make synchronous calls from another thread, unmarked
    Ends,       \* TRUE: a session may end during the run
    Async,      \* TRUE: an activation may make an asynchronous call
    Awaited,    \* TRUE: an asynchronous call's caller awaits its result
    Signalled,  \* TRUE: that caller says so on the frame (`awaited`, SESS-83);
                \* FALSE: an old caller, whose call Rust reads conservatively
    Workers,    \* host threads that run Rust plugin code (a bounded pool)
    Services,   \* TRUE: a runtime may call a Rust service (the host as a participant)
    Cancellers, \* runtimes that cancel a synchronous call with `cancel`
    KeepMark,   \* TRUE: a cancelled call stays marked until it ends (#561)
    KeepChain,  \* TRUE: a call made after an asynchronous suspension keeps its chain
    Linked,     \* groups of sessions sharing one pump (a Cordis node's connectors)
    Remote      \* sessions on another host: this host does not record their waits

ASSUME Reentrant \subseteq Runtimes
ASSUME Threaded \subseteq Reentrant
ASSUME Declared \subseteq Runtimes
ASSUME Abandoners \subseteq Runtimes
ASSUME Hidden \subseteq Runtimes
ASSUME Cancellers \subseteq Runtimes
ASSUME Workers \in Nat \ {0}
ASSUME Services \in BOOLEAN
ASSUME KeepMark \in BOOLEAN
ASSUME KeepChain \in BOOLEAN
ASSUME Linked \in SUBSET SUBSET Runtimes
ASSUME \A a, b \in Linked : a = b \/ a \cap b = {}
ASSUME \A a \in Linked : Cardinality(a) >= 2
ASSUME Remote \subseteq Runtimes
ASSUME Policy \in {"current", "A", "B", "Bchain"}
ASSUME Rebase \in BOOLEAN
ASSUME EmitChain \in BOOLEAN
ASSUME DispatchMode \in {"none", "bail", "waterfall", "serial", "parallel", "waterfall-async"}
ASSUME DispatchHop \in BOOLEAN
ASSUME HostRoot \in BOOLEAN
ASSUME HostHop \in BOOLEAN
ASSUME Ends \in BOOLEAN
ASSUME Async \in BOOLEAN
ASSUME Awaited \in BOOLEAN
ASSUME Signalled \in BOOLEAN
ASSUME MaxRoots \in Nat /\ MaxDepth \in Nat /\ MaxCalls \in Nat \ {0}
ASSUME MaxPosts \in Nat

VARIABLES
    stack,      \* stack[r]: activations running at r, bottom first (threaded: unordered)
    toRt,       \* toRt[r]: frames Rust sent on r's session, not yet read by r
    deferred,   \* deferred[r]: calls r read and put aside (not on its chain)
    toRust,     \* toRust[r]: frames r sent on its session, not yet read by Rust
    hops,       \* forwarded calls Rust has not answered yet (Rust's own state)
    dispatches, \* event dispatches Rust runs (synchronous or asynchronous)
    hosts,      \* host executions: Rust services and the host's own calls
    hostCtr,    \* hostCtr: last host execution id
    nodeCtr,    \* nodeCtr[r]: last `node:n` id runtime r allocated
    rustCtr,    \* rustCtr[r]: last `rust:n` id Rust allocated on r's session
    roots,      \* top-level activations started so far
    posts,      \* posted calls (an emit's events.call) not answered yet;
                \* `rec`: Rust recorded it as an entry
    postCtr,    \* emits so far
    dispCtr,    \* synchronous dispatches so far
    alive,      \* runtimes whose session has not ended
    ghost       \* observations for the invariants, not part of any process

vars == <<stack, toRt, deferred, toRust, hops, dispatches, hosts, hostCtr,
          nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

NonReentrant == Runtimes \ Reentrant

\* The host as a participant: the destination of a runtime's call into Rust
\* code (a mock's service, a plugin's function). A string, not a model
\* value, so it is no session.
Host == "rust"
ASSUME Host \notin Runtimes

\* The host executions currently holding a worker (dispatches included).
WorkerOf(h) == h.worker
BusyWorkers == {WorkerOf(h) : h \in hosts \cup dispatches} \ {0}
FreeWorkers == (1..Workers) \ BusyWorkers
\* The sessions sharing r's pump (a Cordis node's connectors, `Linked`).
Group(r) == IF \E g \in Linked : r \in g THEN CHOOSE g \in Linked : r \in g ELSE {r}

(***************************************************************************)
(* Call ids and chains.                                                    *)
(* A path entry native to the session it travels on is <<side, n>>         *)
(* ("node:3" = <<"node", 3>>). An entry of another session carries that    *)
(* session's tag: <<s, side, n>> ("s1/node:3"). A global id is always      *)
(* <<session, side, n>>. `truth` fields hold global ids; they are ghost    *)
(* state, used only to state invariants.                                    *)
(***************************************************************************)
InSeq(x, s) == \E i \in 1..Len(s) : s[i] = x
Pos(x, s) == CHOOSE i \in 1..Len(s) : s[i] = x
Last(s) == s[Len(s)]
RemoveAt(s, i) == IF Len(s) <= 1 THEN <<>>
                   ELSE [j \in 1..Len(s) - 1 |-> IF j < i THEN s[j] ELSE s[j + 1]]

Decode(p, s) == [i \in 1..Len(p) |->
                    IF Len(p[i]) = 2 THEN <<s, p[i][1], p[i][2]>> ELSE p[i]]

\* rpc.rs `rebase(path, from, to)`.
RebasePath(p, from, to) ==
    \* A call within one session is not rebased at all (rpc.rs `forwarded`
    \* only rebases a call forwarded from another session).
    IF ~Rebase \/ from = to THEN p
    ELSE [i \in 1..Len(p) |->
            LET e == p[i] IN
            IF Len(e) = 3 /\ e[1] = to THEN <<e[2], e[3]>>
            ELSE IF Len(e) = 3 THEN e
            ELSE <<from, e[1], e[2]>>]


(***************************************************************************)
(* A node of the wait graph: a forwarded call, a posted call, a call the   *)
(* host makes in its own right, a host execution, or a dispatch.           *)
(*   src, sid    the session Rust records as the caller ("" for the host)  *)
(*               and the id the caller gave the call on that session       *)
(*   dst, did    the session the call went to, its id there (a host        *)
(*               execution has dst = Host and its own id in `did`)         *)
(*   chain       the call's chain as Rust reads it (rebased `path`)        *)
(*   truth       its real ancestry, then its own id (ghost)                *)
(*   path        the frame sent to `dst`                                   *)
(*   entry       Rust records it as an entry (nobody waits for it)         *)
(*   real        someone really waits for it (truth)                       *)
(*   maker       the session that really made it ("" = the host, truth)    *)
(*   kind        "hop" | "async" | "entry" | "hidden" | "listener" |       *)
(*               "host" | "service" | "root" | "dispatch"                  *)
(*   under       the node the host makes this call in (or <<>>)            *)
(*   worker      the host worker it holds (0 for none; host nodes and      *)
(*               dispatches only)                                          *)
(***************************************************************************)
Gid(h) == IF h.dst = Host THEN <<Host, "host", h.did>>
          ELSE <<h.dst, IF h.kind = "dispatch" THEN "dispatch" ELSE "rust", h.did>>
\* The call is made out of runtime y (Rust's view, and the truth's).
Out(d, y) == ~d.entry /\ d.src = y
OutTrue(d, y) == d.real /\ d.maker = y
\* A dispatch is a wait of the emitter, not a call that enters a runtime.
Enters(h) == h.kind /= "dispatch"
\* `seq` names the call `g` with no call of `B` after it: what is made under
\* a call nobody waits for (an entry) is not waited for by what that call
\* was made under.
ChainFree(g, seq, B) ==
    InSeq(g, seq) /\ \A i \in (Pos(g, seq) + 1)..Len(seq) : seq[i] \notin B
Gids(S) == {Gid(e) : e \in S}

(***************************************************************************)
(* The runtimes' waiting state, which both graphs read.                    *)
(***************************************************************************)
Top(r) == Last(stack[r])
Waits(r) == {stack[r][i].w : i \in {j \in 1..Len(stack[r]) :
                                       stack[r][j].st \in {"wait", "ready"}}}
Pumping(r) == \E i \in 1..Len(stack[r]) : stack[r][i].st \in {"wait", "ready"}

\* What the runtime checks: the frame's path names a call it waits for.
PathRelated(r, path) == \E w \in Waits(r) : InSeq(<<"node", w>>, path)
\* What is true: the frame really descends from a call r waits for.
TruthRelated(r, truth) == \E w \in Waits(r) : InSeq(<<r, "node", w>>, truth)
\* ... and someone really waits for it along that chain, with no call nobody
\* waits for in between (a posted call is not on it).
Stopped == Gids({d \in hops \cup posts \cup dispatches : ~d.real})
FrameReal(r, f) == \E d \in hops \cup posts : d.dst = r /\ d.did = f.id /\ d.real
TruthWaited(r, truth) ==
    \E w \in Waits(r) : ChainFree(<<r, "node", w>>, truth, Stopped)

\* Where chains stop: at a call nobody waits for. Rust knows only the
\* entries it recorded; the truth knows every call nobody waits for.
RustStop(N) == Gids({e \in N : e.entry})
TrueStop(N) == Gids({e \in N : ~e.real})

RECURSIVE Reach(_, _)
Reach(E, S) == LET S2 == S \cup {e[2] : e \in {x \in E : x[1] \in S}}
               IN IF S2 = S THEN S ELSE Reach(E, S2)
OnCycle(E, n) == n \in Reach(E, {e[2] : e \in {x \in E : x[1] = n}})

InFlight(y, id) == \E i \in 1..Len(toRt[y]) : toRt[y][i].op = "call" /\ toRt[y][i].id = id

\* The real graph, from the runtimes' actual stacks and queues:
\*  - a call waits for the calls made under it (its chain);
\*  - a call that enters a stacked, non-reentrant runtime y whose stack is
\*    not empty, and is not on its chain, waits for every call that runtime
\*    is blocked in or running (it runs only once the stack is empty);
\*  - a call running at stack index i waits for every call made from an
\*    activation at index >= i, and for every incoming call running at an
\*    index > i (LIFO: it resumes only when they return);
\*  - a dispatch waits for the listener calls the host makes under it;
\*  - a host execution waits for the calls it made (its chain), and a queued
\*    one waits for whoever holds a worker;
\*  - a session's frames wait while another session of its pump group waits
\*    (`Linked`); no edge is built for it, the deadlock check sees it.
OnStack(y, d) == \E j \in 1..Len(stack[y]) : stack[y][j].from = d.did
TrueSucc(h, N, new) ==
    LET y == h.dst
        idx == IF y \in Runtimes
               THEN {i \in 1..Len(stack[y]) : stack[y][i].from = h.did /\ h /= new}
               ELSE {}
        waiting == h = new \/ (y \in Runtimes /\
                     (InFlight(y, h.did) \/ \E f \in deferred[y] : f.id = h.did))
        defer == y \in Runtimes /\ waiting /\ h.real /\ Enters(h)
                 /\ y \in NonReentrant /\ y \notin Threaded /\ stack[y] /= <<>>
                 /\ ~PathRelated(y, h.path)
    IN {c \in N : c.real /\ ChainFree(Gid(h), c.truth, TrueStop(N))}
       \cup (IF defer THEN {d \in N : OutTrue(d, y) \/ (d.dst = y /\ OnStack(y, d))} ELSE {})
       \cup {d \in N : Enters(h) /\ OutTrue(d, y)
                       /\ \E i \in idx : \E j \in i..Len(stack[y]) : stack[y][j].w = d.sid}
       \cup {d \in N : Enters(h) /\ d.dst = y /\ d.real /\ y \notin Threaded
                       /\ \E i \in idx : \E j \in (i + 1)..Len(stack[y]) :
                                      stack[y][j].from = d.did}
       \cup {d \in N : d.under = Gid(h)}
       \* A host execution waiting for a worker waits for whoever holds one.
       \cup (IF y = Host /\ h.st = "queued" /\ FreeWorkers = {}
             THEN {d \in hosts \cup dispatches : d.worker /= 0} ELSE {})
TrueCycle(new) ==
    LET N == hops \cup posts \cup dispatches \cup hosts \cup {new}
    IN OnCycle({<<a, b>> \in N \X N : b \in TrueSucc(a, N, new)}, new)

\* Policy B: the graph Rust can build from what it sees itself: the hops,
\* the paths they carried, the order of ids on each session, and which
\* runtimes are reentrant or have one stack (capabilities `reentrant-sync`,
\* `sync-stack`). It cannot see a runtime's stack, but it can infer the part
\* that matters:
\*  - chain: a call waits for the calls whose path names it;
\*  - put aside: a call into a stacked, non-reentrant runtime that has calls
\*    out, none of them on its chain and none made under it, waits for all
\*    of them;
\*  - stack order (not in "Bchain"): a runtime reads its session in order
\*    and stacks what it runs, so a call c waits for every later call d
\*    into the same runtime that has started there (some call out of that
\*    runtime names d in its path). Threaded runtimes have neither edge.
\*  - entries (waits.rs): a posted call nobody waits for, recorded while
\*    some forwarded call is in progress; a boundary of chains (what is
\*    made under it is not waited for by what it was made under), and
\*    stack order holds for it like for a hop.
Rec == {e \in posts : e.rec}
Started(d, N) == \E e \in N : Out(e, d.dst) /\ InSeq(Gid(d), e.chain)
Related(d, y, h) == InSeq(<<y, "node", d.sid>>, h.chain)
RustSucc(h, N) ==
    LET y == h.dst
        busy == \E d \in N : Out(d, y)
        related == \E d \in N : Out(d, y) /\ Related(d, y, h)
        defer == y \in NonReentrant /\ y \notin Threaded /\ busy /\ ~related /\ ~Started(h, N)
    IN {c \in N : ~c.entry /\ ChainFree(Gid(h), c.chain, RustStop(N))}
       \cup (IF defer THEN {d \in N : Out(d, y)} ELSE {})
       \cup (IF Policy = "B" /\ y \notin Threaded /\ Enters(h)
             THEN {d \in N : Enters(d) /\ d.dst = y /\ h.did < d.did /\ Started(d, N)}
             ELSE {})
RustCycle(new) ==
    LET N == hops \cup Rec \cup {new}
    IN OnCycle({<<a, b>> \in N \X N : b \in RustSucc(a, N)}, new)

(***************************************************************************)
(* Runtime side.                                                           *)
(* Activation: [path, truth, depth, st, w, n, from, mk]                    *)
(*   st = "fresh": running, not in a call                                  *)
(*        "wait":  blocked in a synchronous call `node:w`, pumping         *)
(*        "ready": its reply arrived; still pumping until the loop exits   *)
(*   n = calls made so far (at most MaxCalls)                              *)
(*   from = the `rust:n` id of the call it runs, 0 for a top-level one.    *)
(*   mk = that call was received with `sync`, so a call it makes is on a   *)
(*        marked chain (waits.rs).                                         *)
(***************************************************************************)
\* The activations that may act now: the top of a stacked runtime, every
\* running one at a runtime with a thread per call.
Running(r) == {i \in 1..Len(stack[r]) :
                 stack[r][i].st = "fresh" /\ (r \in Threaded \/ i = Len(stack[r]))}
Callable(r) == \E i \in Running(r) : stack[r][i].depth < MaxDepth /\ stack[r][i].n < MaxCalls
CallAt(r) == CHOOSE i \in Running(r) : stack[r][i].depth < MaxDepth /\ stack[r][i].n < MaxCalls
Returnable(r) == \E i \in 1..Len(stack[r]) :
                    stack[r][i].st \in {"fresh", "ready"} /\ (r \in Threaded \/ i = Len(stack[r]))
ReturnAt(r) == CHOOSE i \in 1..Len(stack[r]) :
                 stack[r][i].st \in {"fresh", "ready"} /\ (r \in Threaded \/ i = Len(stack[r]))
Readyable(r) == \E i \in 1..Len(stack[r]) :
                   stack[r][i].st = "ready" /\ stack[r][i].n < MaxCalls
                   /\ (r \in Threaded \/ i = Len(stack[r]))
ReadyAt(r) == CHOOSE i \in 1..Len(stack[r]) :
                stack[r][i].st = "ready" /\ stack[r][i].n < MaxCalls
                /\ (r \in Threaded \/ i = Len(stack[r]))

Activation(r, f) == [path |-> Append(f.path, <<"rust", f.id>>),
                     truth |-> Append(f.truth, <<r, "rust", f.id>>),
                     depth |-> f.depth, st |-> "fresh", w |-> 0, n |-> 0,
                     from |-> f.id, mk |-> f.mk, cancelled |-> FALSE]

\* A call received with `sync` that a `cancel` reached: it keeps its mark
\* until it ends (`KeepMark`), or loses it at once (#561 before).
CancelledActivation(a, id) ==
    IF a.from = id
    THEN [a EXCEPT !.cancelled = TRUE,
                   !.mk = IF KeepMark THEN a.mk ELSE FALSE]
    ELSE a
CancelledFrame(d, id) ==
    IF d.id = id
    THEN [d EXCEPT !.mk = IF KeepMark THEN d.mk ELSE FALSE]
    ELSE d

\* The frame an activation makes for a call out of its runtime: "call" says
\* its caller waits synchronously (`sync: true`), "hcall" says it waits but
\* the frame does not say so (a call from another thread), "acall" says it
\* does not wait.
CallFrame(a, k, t, op) ==
    [op |-> op, id |-> k, path |-> a.path, truth |-> a.truth, depth |-> a.depth,
     target |-> t, from |-> a.from, mk |-> a.mk]
\* The same from a thread of the runtime's own that runs no call of its own
\* (Python's `run_coroutine_threadsafe`): no chain, and no `sync`.
HiddenFrame(a, k, t) ==
    [op |-> "hcall", id |-> k, path |-> <<>>, truth |-> a.truth, depth |-> a.depth,
     target |-> t, from |-> 0, mk |-> FALSE]

\* A synchronous call: the activation blocks until the reply.
Call(r, t) ==
    /\ r \in alive /\ t \in alive /\ t /= r
    /\ Callable(r)
    /\ LET i == CallAt(r) IN
       LET a == stack[r][i] k == nodeCtr[r] + 1 IN
       /\ toRust' = [toRust EXCEPT ![r] = Append(@, CallFrame(a, k, t, "call"))]
       /\ stack' = [stack EXCEPT ![r][i] = [a EXCEPT !.st = "wait", !.w = k, !.n = @ + 1]]
       /\ nodeCtr' = [nodeCtr EXCEPT ![r] = k]
    /\ UNCHANGED <<toRt, deferred, hops, dispatches, hosts, hostCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* A synchronous call whose frame carries no `sync` (Python's
\* `run_coroutine_threadsafe` from another thread): the caller waits, the
\* host does not see the wait.
CallHidden(r, t) ==
    /\ r \in Hidden
    /\ r \in alive /\ t \in alive /\ t /= r
    /\ Callable(r)
    /\ LET i == CallAt(r) IN
       LET a == stack[r][i] k == nodeCtr[r] + 1 IN
       /\ toRust' = [toRust EXCEPT ![r] = Append(@, HiddenFrame(a, k, t))]
       /\ stack' = [stack EXCEPT ![r][i] = [a EXCEPT !.st = "wait", !.w = k, !.n = @ + 1]]
       /\ nodeCtr' = [nodeCtr EXCEPT ![r] = k]
    /\ UNCHANGED <<toRt, deferred, hops, dispatches, hosts, hostCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* A synchronous call into Rust code (a service or function of the host): the
\* runtime waits; the host becomes a participant (`Services`).
CallService(r) ==
    /\ Services
    /\ r \in alive
    /\ Callable(r)
    /\ LET i == CallAt(r) IN
       LET a == stack[r][i] k == nodeCtr[r] + 1 IN
       /\ toRust' = [toRust EXCEPT ![r] = Append(@,
                        [op |-> "call", id |-> k, path |-> a.path, truth |-> a.truth,
                         depth |-> a.depth, target |-> Host, from |-> a.from,
                         mk |-> a.mk])]
       /\ stack' = [stack EXCEPT ![r][i] = [a EXCEPT !.st = "wait", !.w = k, !.n = @ + 1]]
       /\ nodeCtr' = [nodeCtr EXCEPT ![r] = k]
    /\ UNCHANGED <<toRt, deferred, hops, dispatches, hosts, hostCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* An asynchronous call: the activation goes on running. When its caller is
\* on a marked chain, Rust records it as a hop only if the caller awaits its
\* result (`Awaited`, and an old caller without the signal read
\* conservatively); a call nobody awaits is an entry (SESS-83).
CallAsync(r, t) ==
    /\ Async
    /\ r \in alive /\ t \in alive /\ t /= r
    /\ Callable(r)
    /\ LET i == CallAt(r) IN
       LET a == stack[r][i] k == nodeCtr[r] + 1 IN
       /\ toRust' = [toRust EXCEPT ![r] = Append(@, CallFrame(a, k, t, "acall"))]
       /\ stack' = [stack EXCEPT ![r][i] = [a EXCEPT !.n = @ + 1]]
       /\ nodeCtr' = [nodeCtr EXCEPT ![r] = k]
    /\ UNCHANGED <<toRt, deferred, hops, dispatches, hosts, hostCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* The running activation emits an event whose listener is in t: an
\* events.dispatch nobody waits for; the activation goes on running. Rust
\* then sends t the listener's call with the emitter's chain (RustPost),
\* or without one when the bus's task carries none (EmitChain FALSE, #561).
Emit(r, t) ==
    \* One kind of event at a time: a config either emits or dispatches.
    /\ DispatchMode = "none"
    /\ r \in alive /\ t \in alive
    /\ Callable(r)
    /\ postCtr < MaxPosts
    /\ LET i == CallAt(r) IN
       LET a == stack[r][i] k == nodeCtr[r] + 1 IN
       /\ toRust' = [toRust EXCEPT ![r] = Append(@,
                        [op |-> "emit", id |-> k, path |-> a.path,
                         truth |-> a.truth, depth |-> a.depth, target |-> t])]
       /\ nodeCtr' = [nodeCtr EXCEPT ![r] = k]
    /\ postCtr' = postCtr + 1
    /\ UNCHANGED <<stack, toRt, deferred, hops, dispatches, hosts, hostCtr, rustCtr, roots, posts, dispCtr, alive, ghost>>

\* A synchronous event dispatch (events protocol §5.4: `bail`, and a Cordis
\* plugin's `waterfall`): the emitter waits for the whole dispatch; the host
\* calls one listener and waits for it. The listener call is the host's own
\* synchronous call (a hop in waits.rs, #597). A waterfall's calls back into
\* the emitter's process carry the innermost wait's chain, a bail's carry
\* none.
Dispatch(r, t) ==
    /\ DispatchMode \in {"bail", "waterfall"}
    /\ FreeWorkers /= {}
    /\ r \in alive /\ t \in alive
    \* A dispatch from a top-level activity: what a Cordis plugin's `ctx.emit`
    \* and `ctx.bail` are in practice. A dispatch nested in a service call is
    \* the asynchronous one (`HostDispatch`); a synchronous dispatch there is
    \* left out (README, simplifications).
    /\ \E i \in Running(r) : stack[r][i].from = 0
    /\ Callable(r)
    /\ dispCtr < MaxPosts
    /\ LET i == CHOOSE i \in Running(r) : stack[r][i].from = 0 IN
       LET a == stack[r][i]
           k == nodeCtr[r] + 1
           chained == DispatchMode = "waterfall"
           full == Append(a.path, <<"node", k>>)
           \* What the listener call carries: a waterfall's calls back into
           \* the emitter's process carry the innermost wait's chain; a
           \* bail's carry none. A dispatch Rust records itself is a
           \* forwarded call, so its path names the emitter's id.
           carried == IF DispatchHop THEN (IF chained THEN full ELSE << <<"node", k>> >>)
                      ELSE IF chained THEN full ELSE <<>>
           m == rustCtr[t] + 1
           w == CHOOSE w \in FreeWorkers : TRUE
           d == [src |-> r, sid |-> k, dst |-> t, did |-> dispCtr + 1, seq |-> dispCtr + 1,
                 chain |-> Decode(carried, r), truth |-> Append(a.truth, <<r, "node", k>>),
                 path |-> RebasePath(carried, r, t), entry |-> FALSE, real |-> TRUE,
                 maker |-> r, kind |-> "dispatch", rec |-> FALSE, under |-> <<>>,
                 st |-> "wait", worker |-> w, parent |-> <<>>, n |-> 1]
           \* The listener call: nobody waits for it; the dispatcher waits.
           listener == [src |-> "", sid |-> 0, dst |-> t, did |-> m, seq |-> m,
                        chain |-> Decode(carried, r),
                        truth |-> IF chained THEN Append(a.truth, <<r, "node", k>>) ELSE <<>>,
                        path |-> RebasePath(carried, r, t),
                        entry |-> ~HostHop, real |-> TRUE, maker |-> "",
                        kind |-> "listener", rec |-> FALSE, under |-> Gid(d)]
           frame == [op |-> "call", id |-> m, path |-> listener.path,
                     truth |-> IF DispatchHop THEN Append(a.truth, <<r, "node", k>>)
                              ELSE listener.truth,
                     depth |-> a.depth + 1,
                     mk |-> DispatchMode = "bail" /\ t \in Declared]
       IN /\ toRust' = toRust
          /\ nodeCtr' = [nodeCtr EXCEPT ![r] = k]
          /\ rustCtr' = [rustCtr EXCEPT ![t] = m]
          /\ dispCtr' = dispCtr + 1
          /\ stack' = [stack EXCEPT ![r][i] = [a EXCEPT !.st = "wait", !.w = k, !.n = @ + 1]]
          /\ toRt' = [toRt EXCEPT ![t] = Append(@, frame)]
          /\ IF DispatchHop
             \* Rust records the dispatch itself as the forwarded call: the
             \* emitter's id is on the listener's path, so a callback is on
             \* the chain it waits for.
             THEN /\ hops' = hops \cup {[listener EXCEPT !.src = r, !.sid = k, !.under = <<>>,
                                                       !.kind = "hop", !.entry = FALSE,
                                                       !.truth = Append(a.truth, <<r, "node", k>>)]}
                  /\ dispatches' = dispatches
             \* Rust does not record it: the listener call is the host's own.
             ELSE /\ hops' = hops \cup {listener}
                  /\ dispatches' = dispatches \cup {d}
    /\ UNCHANGED <<deferred, roots, posts, postCtr, hosts, hostCtr, alive, ghost>>

\* A host execution: Rust plugin code on a worker thread. `kind` is "root"
\* (the host's own synchronous call, #597) or "service" (a call from a
\* runtime into Rust code). A service that cannot get a worker waits in
\* `st = "queued"`. `cancelled`/`marked` follow a `cancel`: #561 keeps the
\* mark until the call really ends (`KeepMark`).
\*   [kind, dst = Host, did, src, sid, path, truth, depth, st, w, worker,
\*    n, cancelled, marked]

\* The host's own synchronous call into a runtime (a Rust plugin's, a host
\* service's): it has no chain and occupies a worker while it waits; Rust
\* records it as a hop (#597) or, in older versions, as an entry.
HostCall(t) ==
    /\ HostRoot
    /\ t \in alive
    /\ roots < MaxRoots
    /\ FreeWorkers /= {}
    /\ LET m == rustCtr[t] + 1
           id == hostCtr + 1
           w == CHOOSE w \in FreeWorkers : TRUE
           node == [kind |-> "root", dst |-> Host, did |-> id, src |-> "", sid |-> 0,
                    path |-> <<>>, truth |-> <<>>, depth |-> 0, st |-> "wait", w |-> m,
                    worker |-> w, n |-> 1, cancelled |-> FALSE, marked |-> TRUE,
                    entry |-> FALSE, real |-> TRUE, maker |-> "", under |-> <<>>]
           hop == [src |-> "", sid |-> 0, dst |-> t, did |-> m, seq |-> m,
                   chain |-> <<>>, truth |-> <<<<Host, "host", id>>>>, path |-> <<>>,
                   entry |-> ~HostHop, real |-> TRUE, maker |-> "", kind |-> "host",
                   rec |-> FALSE, under |-> Gid(node)]
       IN /\ hops' = hops \cup {hop}
          /\ hosts' = hosts \cup {node}
          /\ hostCtr' = id
          /\ toRt' = [toRt EXCEPT ![t] = Append(@,
                       [op |-> "call", id |-> m, path |-> <<>>,
                        truth |-> <<<<Host, "host", id>>>>, depth |-> 0,
                        mk |-> t \in Declared])]
          /\ rustCtr' = [rustCtr EXCEPT ![t] = m]
          /\ roots' = roots + 1
    /\ UNCHANGED <<stack, deferred, toRust, dispatches, nodeCtr, posts, postCtr, dispCtr, alive, ghost>>

\* A runtime's synchronous call into Rust code: the host becomes a
\* participant. The call is not forwarded to another session, so it is not
\* a hop in waits.rs; it is a host execution, queued until a worker is free.
RustService(s) ==
    /\ Services
    /\ toRust[s] /= <<>>
    /\ Head(toRust[s]).op = "call"
    /\ Head(toRust[s]).target = Host
    /\ LET f == Head(toRust[s])
           id == hostCtr + 1
           node == [kind |-> "service", dst |-> Host, did |-> id, src |-> s,
                    sid |-> f.id, path |-> Append(f.path, <<"node", f.id>>),
                    truth |-> Append(Append(f.truth, <<s, "node", f.id>>),
                                     <<Host, "host", id>>),
                    depth |-> f.depth + 1, st |-> "queued", w |-> 0, worker |-> 0,
                    n |-> 0, cancelled |-> FALSE, marked |-> TRUE,
                    entry |-> FALSE, real |-> TRUE, maker |-> "", under |-> <<>>]
       IN /\ toRust' = [toRust EXCEPT ![s] = Tail(@)]
          /\ hosts' = hosts \cup {node}
          /\ hostCtr' = id
    /\ UNCHANGED <<stack, toRt, deferred, hops, dispatches, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* A queued host execution starts on a free worker.
StartHost(h) ==
    /\ h \in hosts
    /\ h.st = "queued"
    /\ FreeWorkers /= {}
    /\ LET w == CHOOSE w \in FreeWorkers : TRUE IN
       hosts' = (hosts \ {h}) \cup {[h EXCEPT !.st = "run", !.worker = w]}
    /\ UNCHANGED <<stack, toRt, deferred, toRust, hops, dispatches, hostCtr,
                     nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* Rust code calls a runtime synchronously. The call is the host's own
\* (`src` empty): a hop in waits.rs, with the host execution's id on its
\* path. A cancelled call that makes another one while unmarked violates
\* `MarkedWhileRunning` (`before-cancel`).
HostNodeCall(h, t) ==
    /\ h \in hosts
    /\ h.kind = "service"
    /\ h.st = "run"
    /\ h.n < MaxCalls
    /\ h.depth < MaxDepth
    /\ t \in alive
    /\ LET m == rustCtr[t] + 1
           path == Append(RebasePath(h.path, h.src, t), <<Host, "host", h.did>>)
           hop == [src |-> "", sid |-> 0, dst |-> t, did |-> m, seq |-> m,
                   chain |-> Decode(path, t),
                   truth |-> Append(h.truth, <<Host, "host", h.did>>),
                   path |-> path, entry |-> FALSE, real |-> TRUE, maker |-> "",
                   kind |-> "host", rec |-> FALSE, under |-> Gid(h)]
       IN /\ hops' = hops \cup {hop}
          /\ hosts' = (hosts \ {h}) \cup {[h EXCEPT !.st = "wait", !.w = m, !.n = @ + 1]}
          /\ toRt' = [toRt EXCEPT ![t] = Append(@,
                       [op |-> "call", id |-> m, path |-> path,
                        truth |-> hop.truth, depth |-> h.depth + 1,
                        mk |-> t \in Declared])]
          /\ rustCtr' = [rustCtr EXCEPT ![t] = m]
          /\ ghost' = IF h.cancelled /\ ~h.marked
                      THEN [ghost EXCEPT !.unmarked = TRUE] ELSE ghost
    /\ UNCHANGED <<stack, deferred, toRust, dispatches, hostCtr, nodeCtr, roots,
                     posts, postCtr, dispCtr, alive>>

\* Rust code finished: its worker is free and its caller is answered (a
\* late reply, after a cancel, is dropped by the runtime).
HostNodeReturn(h) ==
    /\ h \in hosts
    /\ h.kind = "service"
    /\ h.st = "run"
    /\ hosts' = hosts \ {h}
    /\ toRt' = [toRt EXCEPT ![h.src] = Append(@, [op |-> "ret", id |-> h.sid])]
    /\ UNCHANGED <<stack, deferred, toRust, hops, dispatches, hostCtr, nodeCtr,
                     rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* An asynchronous dispatch (`serial`, `parallel`, a plugin's asynchronous
\* `waterfall`) inside a running host execution: the execution awaits the
\* listener calls, so it does not hold a worker while it waits. The first
\* poll may suspend (the filter) before the first listener call.
HostDispatch(h, t) ==
    /\ DispatchMode \in {"serial", "parallel", "waterfall-async"}
    /\ h \in hosts
    /\ h.kind = "service"
    /\ h.st = "run"
    /\ t \in alive
    /\ dispCtr < MaxPosts
    /\ LET id == dispCtr + 1
           d == [src |-> h.src, sid |-> h.sid, dst |-> t, did |-> id, seq |-> id,
                 chain |-> <<>>, truth |-> Append(h.truth, <<Host, "host", h.did>>),
                 path |-> h.path, entry |-> TRUE, real |-> TRUE, maker |-> "",
                 kind |-> "async", rec |-> hops /= {}, under |-> <<>>,
                 st |-> "filter", worker |-> 0, parent |-> Gid(h), n |-> 0,
                 depth |-> h.depth]
       IN /\ dispatches' = dispatches \cup {d}
          /\ hosts' = (hosts \ {h}) \cup {[h EXCEPT !.st = "wait", !.w = 0]}
          /\ dispCtr' = id
    /\ UNCHANGED <<stack, toRt, deferred, toRust, hops, hostCtr, nodeCtr, rustCtr,
                     roots, posts, postCtr, alive, ghost>>

\* The dispatch's first poll resumes: the filter answered "keep going".
DispatchFilter(d) ==
    /\ d \in dispatches
    /\ d.kind = "async"
    /\ d.st = "filter"
    /\ dispatches' = (dispatches \ {d}) \cup {[d EXCEPT !.st = "run"]}
    /\ UNCHANGED <<stack, toRt, deferred, toRust, hops, hosts, hostCtr, nodeCtr,
                     rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* The dispatch calls one listener asynchronously. With `KeepChain` FALSE
\* the chain is lost across the suspension (#561 before): a call the
\* listener makes back into the waiting chain is put aside.
DispatchStart(d) ==
    /\ d \in dispatches
    /\ d.kind = "async"
    /\ d.st = "run"
    /\ d.dst \in alive
    /\ LET t == d.dst
           m == rustCtr[t] + 1
           path == IF KeepChain
                   THEN Append(Append(RebasePath(d.path, d.src, t), d.parent),
                               <<Host, "dispatch", d.did>>)
                   ELSE <<>>
           post == [src |-> "", sid |-> 0, dst |-> t, did |-> m, seq |-> m,
                    chain |-> Decode(path, t),
                    truth |-> Append(d.truth, <<t, "dispatch", d.did>>),
                    path |-> path, entry |-> TRUE, real |-> TRUE, maker |-> "",
                    kind |-> "listener", rec |-> hops /= {}, under |-> Gid(d)]
           frame == [op |-> "call", id |-> m, path |-> path,
                     truth |-> post.truth, depth |-> d.depth + 1,
                     mk |-> t \in Declared]
       IN /\ posts' = posts \cup {post}
          /\ dispatches' = (dispatches \ {d}) \cup
                           {[d EXCEPT !.st = "wait", !.n = 1, !.chain = Decode(path, t)]}
          /\ toRt' = [toRt EXCEPT ![t] = Append(@, frame)]
          /\ rustCtr' = [rustCtr EXCEPT ![t] = m]
    /\ UNCHANGED <<stack, deferred, toRust, hops, hosts, hostCtr, nodeCtr, roots,
                     postCtr, dispCtr, alive, ghost>>

\* A top-level activation (a timer, an event) starts at an idle runtime.
StartRoot(r) ==
    /\ r \in alive
    /\ roots < MaxRoots
    /\ stack[r] = <<>>
    /\ stack' = [stack EXCEPT ![r] = <<[path |-> <<>>, truth |-> <<>>, depth |-> 0,
                                    st |-> "fresh", w |-> 0, n |-> 0, from |-> 0,
                                    mk |-> FALSE, cancelled |-> FALSE]>>]
    /\ roots' = roots + 1
    /\ UNCHANGED <<toRt, deferred, toRust, hops, dispatches, hosts, hostCtr, nodeCtr, rustCtr, posts, postCtr, dispCtr, alive, ghost>>

\* The pump loop of the top activation exits; it goes on running.
Resume(r) ==
    /\ r \in alive
    /\ Readyable(r)
    /\ LET i == ReadyAt(r) IN
       stack' = [stack EXCEPT ![r][i] = [stack[r][i] EXCEPT !.st = "fresh", !.w = 0]]
    /\ UNCHANGED <<toRt, deferred, toRust, hops, dispatches, hosts, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* Read the next frame: only when idle or pumping (a running activation does
\* not read) — and at a runtime with a thread per call, always. session.mjs
\* `receive`/`#run`, peer.py `_receive`/`_run`: an idle runtime runs any
\* call; a pumping one runs every call if reentrant, otherwise only calls
\* whose path names a call it waits for, and puts the others aside until its
\* stack is empty.
Receive(r) ==
    /\ r \in alive
    /\ toRt[r] /= <<>>
    /\ r \in Threaded \/ stack[r] = <<>> \/ Pumping(r)
    \* A Cordis node's connectors share one pump: while one session waits,
    \* the others read nothing (`Linked`).
    /\ \A s \in Group(r) : s = r \/ ~Pumping(s)
    /\ LET f == Head(toRt[r]) IN
       /\ toRt' = [toRt EXCEPT ![r] = Tail(@)]
       /\ IF f.op = "ret"
          THEN IF \E i \in 1..Len(stack[r]) : stack[r][i].st = "wait" /\ stack[r][i].w = f.id
               THEN \E i \in 1..Len(stack[r]) :
                        /\ stack[r][i].st = "wait" /\ stack[r][i].w = f.id
                        /\ stack' = [stack EXCEPT ![r][i].st = "ready"]
                    /\ UNCHANGED <<deferred, ghost>>
               \* A late reply (the caller gave up, or an asynchronous call):
               \* it is dropped.
               ELSE /\ stack' = stack
                    /\ UNCHANGED <<deferred, ghost>>
          ELSE IF f.op = "cancel"
          \* The caller cancelled the call `f.id` while it was running: the
          \* mark stays until it really ends (#561), unless `KeepMark` is
          \* FALSE (`before-cancel`).
          THEN /\ stack' = [stack EXCEPT ![r] =
                    [i \in 1..Len(stack[r]) |->
                        CancelledActivation(stack[r][i], f.id)]]
               /\ deferred' = [deferred EXCEPT ![r] =
                    {CancelledFrame(d, f.id) : d \in deferred[r]}]
               /\ UNCHANGED ghost
          ELSE IF r \in Threaded \/ stack[r] = <<>> \/ r \in Reentrant \/ PathRelated(r, f.path)
          THEN /\ stack' = [stack EXCEPT ![r] = Append(@, Activation(r, f))]
               /\ deferred' = deferred
               /\ ghost' =
                    LET g == IF stack[r] /= <<>> /\ r \in NonReentrant
                                /\ ~TruthRelated(r, f.truth)
                             THEN [ghost EXCEPT !.misrouted = TRUE] ELSE ghost IN
                    \* A reentrant runtime's service called in the middle of
                    \* its own synchronous call (SYN-9-3: an explicit
                    \* non-guarantee).
                    IF stack[r] /= <<>> /\ r \in Reentrant /\ r \notin Threaded
                    THEN [g EXCEPT !.nested = TRUE] ELSE g
          ELSE /\ deferred' = [deferred EXCEPT ![r] = @ \cup {f}]
               /\ stack' = stack
               \* Put aside although it belongs to a waiting chain.
               /\ ghost' = IF TruthWaited(r, f.truth) /\ FrameReal(r, f)
                           THEN [ghost EXCEPT !.missed = TRUE] ELSE ghost
    /\ UNCHANGED <<toRust, hops, dispatches, hosts, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive>>

\* Calls put aside run once the stack is empty (a stacked runtime only).
RunDeferred(r, f) ==
    /\ r \in alive
    /\ f \in deferred[r]
    /\ stack[r] = <<>>
    /\ r \notin Threaded
    /\ deferred' = [deferred EXCEPT ![r] = @ \ {f}]
    /\ stack' = [stack EXCEPT ![r] = <<Activation(r, f)>>]
    /\ UNCHANGED <<toRt, toRust, hops, dispatches, hosts, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* Only the top of a stacked runtime returns before anything below resumes;
\* at a runtime with a thread per call, an activation returns on its own.
Return(r) ==
    /\ r \in alive
    /\ Returnable(r)
    /\ LET i == ReturnAt(r) IN
       LET a == stack[r][i] seq == stack[r] IN
       /\ stack' = [stack EXCEPT ![r] = IF r \in Threaded THEN RemoveAt(seq, i)
                                            ELSE SubSeq(seq, 1, Len(seq) - 1)]
       /\ toRust' = IF a.from = 0 THEN toRust
                    ELSE [toRust EXCEPT ![r] = Append(@, [op |-> "ret", id |-> a.from])]
    /\ UNCHANGED <<toRt, deferred, hops, dispatches, hosts, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* The caller gives up a synchronous call locally (Go's context, Swift's
\* task, Kotlin's interrupt) and sends no `cancel`: its thread goes on, the
\* callee keeps running, and Rust keeps the call in its table (so it still
\* counts as a call out of the runtime) until the reply arrives.
Abandon(r) ==
    /\ r \in Abandoners
    /\ \E i \in 1..Len(stack[r]) : stack[r][i].st = "wait"
    /\ LET i == CHOOSE i \in 1..Len(stack[r]) : stack[r][i].st = "wait"
           k == stack[r][i].w IN
       /\ stack' = [stack EXCEPT ![r][i] = [stack[r][i] EXCEPT !.st = "fresh", !.w = 0]]
       /\ hops' = {[h EXCEPT !.real = IF h.src = r /\ h.sid = k THEN FALSE ELSE h.real]
                   : h \in hops}
    /\ UNCHANGED <<toRt, deferred, toRust, dispatches, hosts, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* The caller cancels a synchronous call (`cancel`): it stops waiting and
\* the host drops the wait edge, but the callee may already be running, so
\* its mark stays until it really ends (#561).
Cancel(r) ==
    /\ r \in Cancellers
    /\ \E i \in 1..Len(stack[r]) : stack[r][i].st = "wait"
    /\ LET i == CHOOSE i \in 1..Len(stack[r]) : stack[r][i].st = "wait"
           k == stack[r][i].w IN
       /\ toRust' = [toRust EXCEPT ![r] = Append(@, [op |-> "cancel", id |-> k])]
       /\ stack' = [stack EXCEPT ![r][i] = [stack[r][i] EXCEPT !.st = "fresh", !.w = 0]]
    /\ UNCHANGED <<toRt, deferred, hops, dispatches, hosts, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

\* A `cancel` frame: a forwarded call stops being waited for but stays in
\* the table until its reply (which is then dropped); a call into Rust code
\* marks the host execution cancelled, keeping its mark when `KeepMark`.
RustCancel(s) ==
    /\ toRust[s] /= <<>>
    /\ Head(toRust[s]).op = "cancel"
    /\ LET k == Head(toRust[s]).id
           serv == {h \in hosts : h.kind = "service" /\ h.src = s /\ h.sid = k}
           fwd == {h \in hops : h.src = s /\ h.sid = k}
       IN /\ toRust' = [toRust EXCEPT ![s] = Tail(@)]
          /\ IF serv /= {}
             THEN LET h == CHOOSE h \in serv : TRUE IN
                  /\ hosts' = (hosts \ {h}) \cup
                              {[h EXCEPT !.cancelled = TRUE,
                                        !.marked = IF KeepMark THEN h.marked ELSE FALSE]}
                  /\ hops' = hops
                  /\ toRt' = toRt
             ELSE IF fwd /= {}
                  THEN LET h == CHOOSE h \in fwd : TRUE IN
                       /\ hops' = (hops \ {h}) \cup {[h EXCEPT !.real = FALSE]}
                       /\ toRt' = [toRt EXCEPT ![h.dst] = Append(@,
                                    [op |-> "cancel", id |-> h.did])]
                       /\ hosts' = hosts
                  ELSE /\ hops' = hops /\ hosts' = hosts /\ toRt' = toRt
    /\ UNCHANGED <<stack, deferred, dispatches, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

(***************************************************************************)
(* Rust side (rpc.rs `receive`/`execute`, waits.rs `forward`,               *)
(* rows.rs `RowService::invoke`, relay.rs `relay`, events/hub.rs). Rust     *)
(* reads each session in order. A forwarded call runs on the worker thread  *)
(* already pumping the chain, or on a free one (`Workers`); a call into    *)
(* Rust code takes a worker too (`RustService`).                           *)
(***************************************************************************)
RustForward(s) ==
    /\ toRust[s] /= <<>>
    /\ Head(toRust[s]).op \in {"call", "hcall", "acall"}
    /\ Head(toRust[s]).target /= Host
    /\ LET f == Head(toRust[s])
           t == f.target
           \* A call from another host: its caller's wait is in the other
           \* host's table, and the path this host writes carries none of the
           \* caller's ids, so a callback cannot land on the chain here.
           remote == s \in Remote
           full == IF remote THEN <<>> ELSE Append(f.path, <<"node", f.id>>)
           truth == Append(f.truth, <<s, "node", f.id>>)
           m == rustCtr[t] + 1
           sync == f.op \in {"call", "hcall"}
           \* waits.rs `forward`: a call whose chain's last call is marked is
           \* a hop when its caller waits for the result -- synchronously, or
           \* asynchronously and the frame says so (`awaited`, SESS-83; an old
           \* caller says nothing and is read conservatively). A call nobody
           \* waits for is an entry: it bounds the chain made under it but is
           \* no edge. A synchronous call with no marked chain is a hop of the
           \* host's (no caller, #597). A runtime that does not declare
           \* `sync-wait` marks nothing, so its calls stay outside the check.
           \* The model's `acall` never blocks its runtime's stack, so `real`
           \* stays `sync`: the truth graph represents stack waits only, and
           \* the awaited case is left to the implementation's tests.
           marked == f.from /= 0 /\ f.mk
           waits == sync \/ (~sync /\ (IF Signalled THEN Awaited ELSE TRUE))
           kind == IF remote THEN (IF sync THEN "hidden" ELSE "entry")
                   ELSE IF sync /\ (marked \/ (f.op = "call" /\ s \in Declared)) THEN "hop"
                   ELSE IF ~sync /\ marked /\ waits THEN "async"
                   ELSE IF sync THEN "hidden"
                   ELSE "entry"
           new == [src |-> s, sid |-> f.id, dst |-> t, did |-> m, seq |-> m,
                   chain |-> Decode(full, s),
                   truth |-> truth, path |-> RebasePath(full, s, t),
                   entry |-> kind \in {"entry", "hidden"},
                   real |-> sync,
                   maker |-> s, kind |-> kind, rec |-> FALSE, under |-> <<>>]
           \* rpc.rs `receive`: the Rust thread waiting for the last id of
           \* the path that it waits for gets the call.
           waiters == {h \in hops : h.dst = s /\ InSeq(<<"rust", h.did>>, full)}
           chosen == CHOOSE h \in waiters :
                        \A h2 \in waiters : Pos(<<"rust", h2.did>>, full)
                                            <= Pos(<<"rust", h.did>>, full)
           misrouted == waiters /= {} /\ ~InSeq(Gid(chosen), truth)
           refuse == Policy = "A" /\ s \in NonReentrant /\ t \in NonReentrant
           cycle == Policy \in {"B", "Bchain"} /\ RustCycle(new)
       IN
       /\ toRust' = [toRust EXCEPT ![s] = Tail(@)]
       /\ IF t \notin alive
          \* The session ended before the call was forwarded: the caller is
          \* answered with an error.
          THEN /\ toRt' = [toRt EXCEPT ![s] = Append(@,
                             [op |-> "ret", id |-> f.id, outcome |-> "closed"])]
               /\ UNCHANGED <<hops, rustCtr, ghost>>
          ELSE IF refuse \/ cycle
          THEN /\ toRt' = [toRt EXCEPT ![s] = Append(@,
                             [op |-> "ret", id |-> f.id,
                              outcome |-> IF refuse THEN "refused" ELSE "SyncWaitCycle"])]
               /\ UNCHANGED <<hops, rustCtr>>
               /\ ghost' = [ghost EXCEPT
                     !.misrouted = @ \/ misrouted,
                     !.refusedLegit = @ \/ (refuse /\ ~TrueCycle(new)),
                     !.falseCycle = @ \/ (cycle /\ ~TrueCycle(new)),
                     !.refused = @ \/ refuse,
                     !.cycled = @ \/ cycle]
          ELSE /\ hops' = hops \cup {new}
               /\ toRt' = [toRt EXCEPT ![t] = Append(@,
                             [op |-> "call", id |-> m, path |-> new.path,
                              truth |-> truth, depth |-> f.depth + 1,
                              mk |-> t \in Declared])]
               /\ rustCtr' = [rustCtr EXCEPT ![t] = m]
               /\ ghost' = [ghost EXCEPT !.misrouted = @ \/ misrouted]
    /\ UNCHANGED <<stack, deferred, nodeCtr, dispatches, hosts, hostCtr, roots, posts, postCtr, dispCtr, alive>>

\* An emit: Rust sends the listener's call (events.call) to its runtime with
\* the emitter's chain, rebased (or without one: EmitChain FALSE, #561);
\* nobody waits for it. While some forwarded call is in progress Rust
\* records it (`rec`, waits.rs entries).
RustPost(s) ==
    /\ toRust[s] /= <<>>
    /\ Head(toRust[s]).op = "emit"
    /\ LET f == Head(toRust[s])
           t == f.target
           \* What the listener call carries: the emitter's chain, or
           \* nothing when the bus's task carries none (#561).
           full == IF EmitChain THEN Append(f.path, <<"node", f.id>>) ELSE <<>>
           m == rustCtr[t] + 1
           post == [src |-> "", sid |-> 0, dst |-> t, did |-> m, seq |-> m,
                    chain |-> Decode(full, s),
                    truth |-> Append(f.truth, <<s, "node", f.id>>),
                    path |-> RebasePath(full, s, t),
                    entry |-> TRUE, real |-> FALSE, maker |-> "",
                    kind |-> "entry", rec |-> hops /= {}, under |-> <<>>]
       IN /\ toRust' = [toRust EXCEPT ![s] = Tail(@)]
          /\ IF t \notin alive
             \* The listener's session ended: the post goes nowhere, and
             \* nobody waits for it.
             THEN /\ UNCHANGED <<toRt, rustCtr, posts>>
             ELSE /\ toRt' = [toRt EXCEPT ![t] = Append(@,
                             [op |-> "call", id |-> m, path |-> post.path,
                              truth |-> post.truth, depth |-> f.depth + 1,
                              mk |-> t \in Declared])]
                  /\ rustCtr' = [rustCtr EXCEPT ![t] = m]
                  /\ posts' = posts \cup {post}
    /\ UNCHANGED <<stack, deferred, hops, dispatches, hosts, hostCtr, nodeCtr, roots, postCtr, dispCtr, alive, ghost>>

\* The answer to a posted call: nobody waits for it. A listener call of an
\* asynchronous dispatch ends the dispatch, and the host execution that
\* awaited it goes on.
RustAnswered(t) ==
    /\ toRust[t] /= <<>>
    /\ Head(toRust[t]).op = "ret"
    /\ ~\E h \in hops : h.dst = t /\ h.did = Head(toRust[t]).id
    /\ \E e \in posts : e.dst = t /\ e.did = Head(toRust[t]).id
    /\ LET e == CHOOSE e \in posts : e.dst = t /\ e.did = Head(toRust[t]).id
           d == CHOOSE d \in dispatches : Gid(d) = e.under
       IN /\ posts' = posts \ {e}
          /\ toRust' = [toRust EXCEPT ![t] = Tail(@)]
          /\ IF e.under = <<>> \/ ~\E q \in dispatches : Gid(q) = e.under
             THEN UNCHANGED <<dispatches, hosts>>
             ELSE /\ dispatches' = dispatches \ {d}
                  /\ IF d.kind = "async" /\ \E x \in hosts : Gid(x) = d.parent
                     THEN LET x == CHOOSE x \in hosts : Gid(x) = d.parent IN
                          hosts' = (hosts \ {x}) \cup {[x EXCEPT !.st = "run"]}
                     ELSE hosts' = hosts
    /\ UNCHANGED <<stack, deferred, toRt, hops, hostCtr, nodeCtr, rustCtr, roots, postCtr, dispCtr, alive, ghost>>

\* A forwarded call answered: the reply goes back to its caller, unless
\* nobody waits for it any more (an asynchronous call, a caller that gave
\* up locally or cancelled). The call it was made under goes on: a
\* synchronous dispatch's emitter, or the host execution that made it.
RustReturn(t) ==
    /\ toRust[t] /= <<>>
    /\ Head(toRust[t]).op = "ret"
    /\ \E h \in hops : h.dst = t /\ h.did = Head(toRust[t]).id
    /\ LET h == CHOOSE h \in hops : h.dst = t /\ h.did = Head(toRust[t]).id
           d == CHOOSE d \in dispatches : Gid(d) = h.under
           x == CHOOSE x \in hosts : Gid(x) = h.under
       IN /\ hops' = hops \ {h}
          /\ toRt' = IF h.real /\ h.src /= ""
                     THEN [toRt EXCEPT ![h.src] = Append(@,
                              [op |-> "ret", id |-> h.sid, outcome |-> "ok"])]
                     ELSE toRt
          /\ IF h.under = <<>>
             THEN UNCHANGED <<dispatches, hosts, stack>>
             ELSE IF \E q \in dispatches : Gid(q) = h.under
                  THEN /\ dispatches' = dispatches \ {d}
                       /\ hosts' = hosts
                       /\ \E i \in 1..Len(stack[d.src]) :
                              stack[d.src][i].st = "wait" /\ stack[d.src][i].w = d.sid
                              /\ stack' = [stack EXCEPT ![d.src][i].st = "ready"]
                  ELSE IF \E z \in hosts : Gid(z) = h.under
                       THEN /\ dispatches' = dispatches
                            /\ IF x.kind = "root"
                               THEN hosts' = hosts \ {x}
                               ELSE hosts' = (hosts \ {x}) \cup
                                             {[x EXCEPT !.st = "run", !.w = 0]}
                            /\ stack' = stack
                       ELSE UNCHANGED <<dispatches, hosts, stack>>
    /\ toRust' = [toRust EXCEPT ![t] = Tail(@)]
    /\ UNCHANGED <<deferred, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, alive, ghost>>

(***************************************************************************)
(* A session ends: its calls leave the table, and the calls into it fail   *)
(* (SYN-9-30). The runtime itself is gone; its callers are answered with   *)
(* an error, which the model shows as their activation becoming ready.     *)
(***************************************************************************)
End(r) ==
    /\ Ends
    /\ r \in alive
    /\ toRust[r] = <<>>
    /\ hosts = {}
    /\ ~\E h \in hops : h.src = r
    /\ ~\E d \in dispatches : d.src = r \/ d.dst = r
    /\ LET gone == {h \in hops : h.dst = r}
           Release(s) == [i \in 1..Len(stack[s]) |->
                            IF \E h \in gone : h.src = s /\ h.sid = stack[s][i].w
                            THEN [stack[s][i] EXCEPT !.st = "ready", !.w = 0]
                            ELSE stack[s][i]]
       IN /\ hops' = hops \ gone
          /\ stack' = [s \in Runtimes |-> IF s = r THEN <<>> ELSE Release(s)]
          /\ toRt' = [toRt EXCEPT ![r] = <<>>]
          /\ deferred' = [deferred EXCEPT ![r] = {}]
          /\ alive' = alive \ {r}
    /\ UNCHANGED <<toRust, dispatches, hosts, hostCtr, nodeCtr, rustCtr, roots, posts, postCtr, dispCtr, ghost>>

(***************************************************************************)
Quiet == /\ hops = {}
         /\ posts = {}
         /\ dispatches = {}
         /\ hosts = {}
         /\ \A r \in alive : /\ stack[r] = <<>> /\ deferred[r] = {}
                             /\ toRt[r] = <<>> /\ toRust[r] = <<>>

\* Everything finished: stutter, so TLC's deadlock check reports only
\* states where unfinished calls cannot progress.
Done == Quiet /\ UNCHANGED vars

Init ==
    /\ stack = [r \in Runtimes |-> <<>>]
    /\ toRt = [r \in Runtimes |-> <<>>]
    /\ deferred = [r \in Runtimes |-> {}]
    /\ toRust = [r \in Runtimes |-> <<>>]
    /\ hops = {}
    /\ dispatches = {}
    /\ hosts = {}
    /\ hostCtr = 0
    /\ nodeCtr = [r \in Runtimes |-> 0]
    /\ rustCtr = [r \in Runtimes |-> 0]
    /\ roots = 0
    /\ posts = {}
    /\ postCtr = 0
    /\ dispCtr = 0
    /\ alive = Runtimes
    /\ ghost = [misrouted |-> FALSE, missed |-> FALSE, refusedLegit |-> FALSE,
                falseCycle |-> FALSE, refused |-> FALSE, cycled |-> FALSE,
                nested |-> FALSE, unmarked |-> FALSE]

Next ==
    \/ \E r \in Runtimes :
          \/ StartRoot(r)
          \/ Return(r)
          \/ Resume(r)
          \/ End(r)
          \/ Abandon(r)
          \/ Cancel(r)
          \/ \E t \in Runtimes : Call(r, t)
          \/ \E t \in Runtimes : CallHidden(r, t)
          \/ \E t \in Runtimes : CallAsync(r, t)
          \/ CallService(r)
          \/ \E t \in Runtimes : Emit(r, t)
          \/ \E t \in Runtimes : Dispatch(r, t)
          \/ Receive(r)
          \/ \E f \in deferred[r] : RunDeferred(r, f)
          \/ RustForward(r)
          \/ RustService(r)
          \/ RustReturn(r)
          \/ RustCancel(r)
          \/ RustPost(r)
          \/ RustAnswered(r)
          \/ \E d \in dispatches : DispatchFilter(d) \/ DispatchStart(d)
          \/ \E h \in hosts : StartHost(h) \/ HostNodeReturn(h)
          \/ \E h \in hosts : \E t \in Runtimes : HostNodeCall(h, t)
          \/ \E h \in hosts : \E t \in Runtimes : HostDispatch(h, t)
    \/ \E t \in Runtimes : HostCall(t)
    \/ Done

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* Properties. "No deadlock" is TLC's deadlock check: while calls are       *)
(* unfinished some step is enabled (Done covers only the finished state).   *)
(***************************************************************************)
\* A call runs as part of a waiting chain (in a runtime, or on a Rust
\* pumping thread) only if it really belongs to that chain: a callback
\* reaches the caller that is waiting for it, not one with an equal id.
RoutingSound == ~ghost.misrouted

\* A non-reentrant runtime never puts aside a call of its own chain.
ChainRecognised == ~ghost.missed

\* Policy B: SyncWaitCycle only when the waits really form a cycle.
NoFalseCycle == ~ghost.falseCycle

\* Policy A: a call is legitimate when forwarding it closes no wait cycle;
\* no legitimate call is refused.
NoLegitimateRefused == ~ghost.refusedLegit

\* Reachability checks (expected to be violated): the policy acts at all.
NeverRefused == ~ghost.refused
NeverCycle == ~ghost.cycled
\* SYN-9-3, expected to be violated: a reentrant runtime's service is
\* called in the middle of its own synchronous call.
NeverNested == ~ghost.nested
\* #561: a synchronous call received by Rust code stays marked until it
\* really ends, so a call it forwards after a cancel is still a hop.
\* `before-cancel` removes the mark at the cancel and violates this.
MarkedWhileRunning == ~ghost.unmarked
==============================================================================
