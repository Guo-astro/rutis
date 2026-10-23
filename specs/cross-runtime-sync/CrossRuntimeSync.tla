--------------------------- MODULE CrossRuntimeSync ---------------------------
(***************************************************************************)
(* Synchronous calls between language runtimes, relayed by the Rust host.  *)
(*                                                                         *)
(* Design: docs/design-multilang-runtimes-2026-10-03.md §五 (forwarding,   *)
(* path rewriting) and §九 (deadlock policy, open);                         *)
(* docs/design-multilang-m1-2026-10-04.md §七 (relays, `rebase`).           *)
(* Implementation modelled: crates/rutis-bridge/src/session/rpc.rs and      *)
(* rpc/relay.rs, runtime/rows.rs (Rust); node/rutis-runtime/src/session.mjs *)
(* (Node, non-reentrant); python/rutis/rutis/peer.py (Python, reentrant).   *)
(* README.md next to this file lists the simplifications and results.      *)
(*                                                                         *)
(* Every runtime has one session with Rust; each direction of a session is *)
(* FIFO. A runtime is single-threaded: a synchronous call blocks its        *)
(* thread, which then pumps incoming frames and runs some of them nested on *)
(* the same stack. Rust is a relay: a call from runtime s to a service (or  *)
(* function) owned by runtime t is forwarded on t's session with the call   *)
(* chain `path` rebased.                                                    *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Runtimes,   \* runtime sessions (model values)
    Reentrant,  \* runtimes that run every incoming call while waiting
    Policy,     \* "current" | "A" | "B" | "Bchain" (B without stack order)
    Rebase,     \* TRUE: Rust rewrites `path` as rpc.rs `rebase`; FALSE: verbatim
    MaxRoots,   \* top-level activations (timers, events) over the whole run
    MaxDepth,   \* hops in one call chain
    MaxCalls,   \* synchronous calls one activation makes, one after another
    MaxPosts    \* emits: calls Rust then sends that nobody waits for (events.call)

ASSUME Reentrant \subseteq Runtimes
ASSUME Policy \in {"current", "A", "B", "Bchain"}
ASSUME Rebase \in BOOLEAN
ASSUME MaxRoots \in Nat /\ MaxDepth \in Nat /\ MaxCalls \in Nat \ {0}
ASSUME MaxPosts \in Nat

VARIABLES
    stack,     \* stack[r]: activations on r's thread, bottom first
    toRt,      \* toRt[r]: frames Rust sent on r's session, not yet read by r
    deferred,  \* deferred[r]: calls r read and put aside (not on its chain)
    toRust,    \* toRust[r]: frames r sent on its session, not yet read by Rust
    hops,      \* forwarded calls Rust has not answered yet (Rust's own state)
    nodeCtr,   \* nodeCtr[r]: last `node:n` id runtime r allocated
    rustCtr,   \* rustCtr[r]: last `rust:n` id Rust allocated on r's session
    roots,     \* top-level activations started so far
    posts,     \* posted calls (an emit's events.call) not answered yet;
               \* `rec`: Rust recorded it as an entry
    postCtr,   \* emits so far
    ghost      \* observations for the invariants, not part of any process

vars == <<stack, toRt, deferred, toRust, hops, nodeCtr, rustCtr, roots, posts, postCtr, ghost>>

NonReentrant == Runtimes \ Reentrant

(***************************************************************************)
(* Call ids and chains.                                                    *)
(* A path entry native to the session it travels on is <<side, n>>         *)
(* ("node:3" = <<"node", 3>>). An entry of another session carries that    *)
(* session's tag: <<s, side, n>> ("s1/node:3"). A global id is always      *)
(* <<session, side, n>>. `truth` fields hold global ids; they are ghost     *)
(* state, used only to state invariants.                                    *)
(***************************************************************************)
InSeq(x, s) == \E i \in 1..Len(s) : s[i] = x
Pos(x, s) == CHOOSE i \in 1..Len(s) : s[i] = x
Last(s) == s[Len(s)]

Decode(p, s) == [i \in 1..Len(p) |->
                    IF Len(p[i]) = 2 THEN <<s, p[i][1], p[i][2]>> ELSE p[i]]

\* rpc.rs `rebase(path, from, to)`.
RebasePath(p, from, to) ==
    IF ~Rebase THEN p
    ELSE [i \in 1..Len(p) |->
            LET e == p[i] IN
            IF Len(e) = 3 /\ e[1] = to THEN <<e[2], e[3]>>
            ELSE IF Len(e) = 3 THEN e
            ELSE <<from, e[1], e[2]>>]

(***************************************************************************)
(* Runtime side.                                                           *)
(* Activation: [path, truth, depth, st, w, n, from]                        *)
(*   st = "fresh": running, not in a call                                  *)
(*        "wait":  blocked in a synchronous call `node:w`, pumping         *)
(*        "ready": its reply arrived; still pumping until the loop exits   *)
(*   n = calls made so far (at most MaxCalls)                              *)
(*   from = the `rust:n` id of the call it runs, 0 for a top-level one.    *)
(***************************************************************************)
Top(r) == Last(stack[r])
WithTop(r, a) == [stack EXCEPT ![r] = [@ EXCEPT ![Len(@)] = a]]

\* Ids in the runtime's waiting list (Node `#waiting`, Python `_waiting`).
Waits(r) == {stack[r][i].w : i \in {j \in 1..Len(stack[r]) :
                                       stack[r][j].st \in {"wait", "ready"}}}
Pumping(r) == stack[r] /= <<>> /\ Top(r).st \in {"wait", "ready"}

\* What the runtime checks: the frame's path names a call it waits for.
PathRelated(r, path) == \E w \in Waits(r) : InSeq(<<"node", w>>, path)
\* What is true: the frame really descends from a call r waits for.
TruthRelated(r, truth) == \E w \in Waits(r) : InSeq(<<r, "node", w>>, truth)

Activation(r, f) == [path |-> Append(f.path, <<"rust", f.id>>),
                     truth |-> Append(f.truth, <<r, "rust", f.id>>),
                     depth |-> f.depth, st |-> "fresh", w |-> 0, n |-> 0, from |-> f.id]

StartRoot(r) ==
    /\ roots < MaxRoots
    /\ stack[r] = <<>>
    /\ stack' = [stack EXCEPT ![r] = <<[path |-> <<>>, truth |-> <<>>, depth |-> 0,
                                        st |-> "fresh", w |-> 0, n |-> 0, from |-> 0]>>]
    /\ roots' = roots + 1
    /\ UNCHANGED <<toRt, deferred, toRust, hops, nodeCtr, rustCtr, posts, postCtr, ghost>>

\* A synchronous call (host service, or a function owned by t) from the
\* running activation. Calls inside one runtime do not cross Rust.
Call(r, t) ==
    /\ t /= r
    /\ stack[r] /= <<>>
    /\ Top(r).st = "fresh"
    /\ Top(r).depth < MaxDepth
    /\ Top(r).n < MaxCalls
    /\ LET k == nodeCtr[r] + 1 IN
       /\ toRust' = [toRust EXCEPT ![r] = Append(@,
                        [op |-> "call", id |-> k, path |-> Top(r).path,
                         truth |-> Top(r).truth, depth |-> Top(r).depth, target |-> t])]
       /\ stack' = WithTop(r, [Top(r) EXCEPT !.st = "wait", !.w = k, !.n = @ + 1])
       /\ nodeCtr' = [nodeCtr EXCEPT ![r] = k]
    /\ UNCHANGED <<toRt, deferred, hops, rustCtr, roots, posts, postCtr, ghost>>

\* The running activation emits an event whose listener is in t: an
\* events.dispatch nobody waits for; the activation goes on running. Rust
\* then sends t the listener's call with the emitter's chain (RustPost).
Emit(r, t) ==
    /\ stack[r] /= <<>>
    /\ Top(r).st = "fresh"
    /\ Top(r).depth < MaxDepth
    /\ postCtr < MaxPosts
    /\ LET k == nodeCtr[r] + 1 IN
       /\ toRust' = [toRust EXCEPT ![r] = Append(@,
                        [op |-> "emit", id |-> k, path |-> Top(r).path,
                         truth |-> Top(r).truth, depth |-> Top(r).depth, target |-> t])]
       /\ nodeCtr' = [nodeCtr EXCEPT ![r] = k]
    /\ postCtr' = postCtr + 1
    /\ UNCHANGED <<stack, toRt, deferred, hops, rustCtr, roots, posts, ghost>>

\* The pump loop of the top activation exits; it goes on running.
Resume(r) ==
    /\ stack[r] /= <<>>
    /\ Top(r).st = "ready"
    /\ Top(r).n < MaxCalls
    /\ stack' = WithTop(r, [Top(r) EXCEPT !.st = "fresh", !.w = 0])
    /\ UNCHANGED <<toRt, deferred, toRust, hops, nodeCtr, rustCtr, roots, posts, postCtr, ghost>>

\* Read the next frame: only when idle or pumping (a running activation
\* does not read). session.mjs `receive`/`#run`, peer.py `_receive`/`_run`:
\* an idle runtime runs any call; a pumping one runs every call if
\* reentrant, otherwise only calls whose path names a call it waits for,
\* and puts the others aside until its stack is empty.
Receive(r) ==
    /\ toRt[r] /= <<>>
    /\ stack[r] = <<>> \/ Pumping(r)
    /\ LET f == Head(toRt[r]) IN
       /\ toRt' = [toRt EXCEPT ![r] = Tail(@)]
       /\ IF f.op = "ret"
          THEN /\ \E i \in 1..Len(stack[r]) :
                    /\ stack[r][i].st = "wait" /\ stack[r][i].w = f.id
                    /\ stack' = [stack EXCEPT ![r][i].st = "ready"]
               /\ UNCHANGED <<deferred, ghost>>
          ELSE IF stack[r] = <<>> \/ r \in Reentrant \/ PathRelated(r, f.path)
          THEN /\ stack' = [stack EXCEPT ![r] = Append(@, Activation(r, f))]
               /\ deferred' = deferred
               \* Taken as part of a waiting chain it does not belong to.
               /\ ghost' = IF stack[r] /= <<>> /\ r \in NonReentrant
                              /\ ~TruthRelated(r, f.truth)
                           THEN [ghost EXCEPT !.misrouted = TRUE] ELSE ghost
          ELSE /\ deferred' = [deferred EXCEPT ![r] = @ \cup {f}]
               /\ stack' = stack
               \* Put aside although it belongs to a waiting chain.
               /\ ghost' = IF TruthRelated(r, f.truth)
                           THEN [ghost EXCEPT !.missed = TRUE] ELSE ghost
    /\ UNCHANGED <<toRust, hops, nodeCtr, rustCtr, roots, posts, postCtr>>

\* Calls put aside run once the stack is empty.
RunDeferred(r, f) ==
    /\ f \in deferred[r]
    /\ stack[r] = <<>>
    /\ deferred' = [deferred EXCEPT ![r] = @ \ {f}]
    /\ stack' = [stack EXCEPT ![r] = <<Activation(r, f)>>]
    /\ UNCHANGED <<toRt, toRust, hops, nodeCtr, rustCtr, roots, posts, postCtr, ghost>>

\* Only the top of the stack runs; it returns before anything below resumes.
Return(r) ==
    /\ stack[r] /= <<>>
    /\ Top(r).st \in {"fresh", "ready"}
    /\ stack' = [stack EXCEPT ![r] = SubSeq(@, 1, Len(@) - 1)]
    /\ toRust' = IF Top(r).from = 0 THEN toRust
                 ELSE [toRust EXCEPT ![r] = Append(@, [op |-> "ret", id |-> Top(r).from])]
    /\ UNCHANGED <<toRt, deferred, hops, nodeCtr, rustCtr, roots, posts, postCtr, ghost>>

(***************************************************************************)
(* Wait graphs. Nodes are forwarded calls (hops); an edge c -> d means c   *)
(* cannot finish before d does. A cycle is a deadlock.                     *)
(***************************************************************************)
Gid(h) == <<h.dst, "rust", h.did>>
\* A call out of runtime y: a hop (an entry has no caller).
Out(d, y) == ~d.entry /\ d.src = y
\* `seq` names the call `g` with no call of `B` after it: what is made under
\* a call nobody waits for (an entry) is not waited for by what that call
\* was made under.
ChainFree(g, seq, B) ==
    InSeq(g, seq) /\ \A i \in (Pos(g, seq) + 1)..Len(seq) : seq[i] \notin B
Gids(S) == {Gid(e) : e \in S}

RECURSIVE Reach(_, _)
Reach(E, S) == LET S2 == S \cup {e[2] : e \in {x \in E : x[1] \in S}}
               IN IF S2 = S THEN S ELSE Reach(E, S2)
OnCycle(E, n) == n \in Reach(E, {e[2] : e \in {x \in E : x[1] = n}})

InFlight(y, id) == \E i \in 1..Len(toRt[y]) : toRt[y][i].op = "call" /\ toRt[y][i].id = id

\* The real graph, from the runtimes' actual stacks and queues:
\*  - a call waits for the calls made under it (its chain);
\*  - a call not started at a non-reentrant runtime whose stack is not
\*    empty, and not on its chain, waits for every call that runtime is
\*    blocked in (it runs only once the stack is empty);
\*    and for every incoming call running on that stack;
\*  - a call running at stack index i waits for every call made from an
\*    activation at index >= i, and for every incoming call running at an
\*    index > i (LIFO: it resumes only when they return).
OnStack(y, d) == \E j \in 1..Len(stack[y]) : stack[y][j].from = d.did
TrueSucc(h, N, new) ==
    LET y == h.dst
        idx == {i \in 1..Len(stack[y]) : stack[y][i].from = h.did /\ h /= new}
        waiting == h = new \/ InFlight(y, h.did) \/ \E f \in deferred[y] : f.id = h.did
        defer == waiting /\ y \in NonReentrant /\ stack[y] /= <<>>
                 /\ ~PathRelated(y, h.path)
    IN {c \in N : ~c.entry /\ ChainFree(Gid(h), c.truth, Gids(posts))}
       \cup (IF defer THEN {d \in N : Out(d, y) \/ (d.dst = y /\ OnStack(y, d))} ELSE {})
       \cup {d \in N : Out(d, y) /\ \E i \in idx : \E j \in i..Len(stack[y]) :
                                      stack[y][j].w = d.sid}
       \cup {d \in N : d.dst = y /\ \E i \in idx : \E j \in (i + 1)..Len(stack[y]) :
                                      stack[y][j].from = d.did}
TrueCycle(new) ==
    LET N == hops \cup posts \cup {new}
    IN OnCycle({<<a, b>> \in N \X N : b \in TrueSucc(a, N, new)}, new)

\* Policy B: the graph Rust can build from what it sees itself: the hops,
\* the paths they carried, the order of ids on each session, and which
\* runtimes are reentrant (capability `reentrant-sync`). It cannot see a
\* runtime's stack, but it can infer the part that matters:
\*  - chain: a call waits for the calls whose path names it;
\*  - put aside: a call into a non-reentrant runtime that has calls out,
\*    none of them on its chain and none made under it, waits for all of
\*    them;
\*  - stack order (not in "Bchain"): a runtime reads its session in order
\*    and stacks what it runs, so a call c waits for every later call d
\*    into the same runtime that has started there (some call out of that
\*    runtime names d in its path). Had c returned first, its reply would
\*    have reached Rust before any frame made under d.
\*  - entries (waits.rs): a posted call nobody waits for, recorded while
\*    some forwarded call is in progress; a boundary of chains (what is
\*    made under it is not waited for by what it was made under), and
\*    stack order holds for it like for a hop.
Rec == {e \in posts : e.rec}
Started(d, N) == \E e \in N : Out(e, d.dst) /\ InSeq(Gid(d), e.chain)
RustSucc(h, N) ==
    LET y == h.dst
        busy == \E d \in N : Out(d, y)
        related == \E d \in N : Out(d, y) /\ InSeq(<<y, "node", d.sid>>, h.chain)
        defer == y \in NonReentrant /\ busy /\ ~related /\ ~Started(h, N)
    IN {c \in N : ~c.entry /\ ChainFree(Gid(h), c.chain, Gids(Rec))}
       \cup (IF defer THEN {d \in N : Out(d, y)} ELSE {})
       \cup (IF Policy = "B"
             THEN {d \in N : d.dst = y /\ h.did < d.did /\ Started(d, N)}
             ELSE {})
RustCycle(new) ==
    LET N == hops \cup Rec \cup {new}
    IN OnCycle({<<a, b>> \in N \X N : b \in RustSucc(a, N)}, new)

(***************************************************************************)
(* Rust side (rpc.rs `receive`/`execute`, rows.rs `RowService::invoke`,    *)
(* relay.rs `relay`). Rust reads each session in order. It runs a          *)
(* forwarded call on the thread already pumping the chain, or on a new     *)
(* one; Rust threads are not a bounded resource here (README).             *)
(***************************************************************************)
RustForward(s) ==
    /\ toRust[s] /= <<>>
    /\ Head(toRust[s]).op = "call"
    /\ LET f == Head(toRust[s])
           t == f.target
           full == Append(f.path, <<"node", f.id>>)
           truth == Append(f.truth, <<s, "node", f.id>>)
           m == rustCtr[t] + 1
           new == [src |-> s, sid |-> f.id, dst |-> t, did |-> m,
                   chain |-> Decode(full, s), truth |-> truth,
                   path |-> RebasePath(full, s, t), entry |-> FALSE, rec |-> TRUE]
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
       /\ IF refuse \/ cycle
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
                              truth |-> truth, depth |-> f.depth + 1])]
               /\ rustCtr' = [rustCtr EXCEPT ![t] = m]
               /\ ghost' = [ghost EXCEPT !.misrouted = @ \/ misrouted]
    /\ UNCHANGED <<stack, deferred, nodeCtr, roots, posts, postCtr>>

\* An emit: Rust sends the listener's call (events.call) to its runtime with
\* the emitter's chain, rebased; nobody waits for it. While some forwarded
\* call is in progress Rust records it (`rec`, waits.rs entries).
RustPost(s) ==
    /\ toRust[s] /= <<>>
    /\ Head(toRust[s]).op = "emit"
    /\ LET f == Head(toRust[s])
           t == f.target
           full == Append(f.path, <<"node", f.id>>)
           m == rustCtr[t] + 1
           post == [src |-> "", sid |-> 0, dst |-> t, did |-> m,
                    chain |-> Decode(full, s), truth |-> Append(f.truth, <<s, "node", f.id>>),
                    path |-> IF s = t THEN full ELSE RebasePath(full, s, t),
                    entry |-> TRUE, rec |-> hops /= {}]
       IN /\ toRust' = [toRust EXCEPT ![s] = Tail(@)]
          /\ toRt' = [toRt EXCEPT ![t] = Append(@,
                         [op |-> "call", id |-> m, path |-> post.path,
                          truth |-> post.truth, depth |-> f.depth + 1])]
          /\ rustCtr' = [rustCtr EXCEPT ![t] = m]
          /\ posts' = posts \cup {post}
    /\ UNCHANGED <<stack, deferred, hops, nodeCtr, roots, postCtr, ghost>>

\* The answer to a posted call: nobody waits for it.
RustAnswered(t) ==
    /\ toRust[t] /= <<>>
    /\ Head(toRust[t]).op = "ret"
    /\ ~\E h \in hops : h.dst = t /\ h.did = Head(toRust[t]).id
    /\ posts' = {e \in posts : ~(e.dst = t /\ e.did = Head(toRust[t]).id)}
    /\ toRust' = [toRust EXCEPT ![t] = Tail(@)]
    /\ UNCHANGED <<stack, deferred, toRt, hops, nodeCtr, rustCtr, roots, postCtr, ghost>>

RustReturn(t) ==
    /\ toRust[t] /= <<>>
    /\ Head(toRust[t]).op = "ret"
    /\ \E h \in hops :
          /\ h.dst = t /\ h.did = Head(toRust[t]).id
          /\ hops' = hops \ {h}
          /\ toRt' = [toRt EXCEPT ![h.src] = Append(@,
                         [op |-> "ret", id |-> h.sid, outcome |-> "ok"])]
    /\ toRust' = [toRust EXCEPT ![t] = Tail(@)]
    /\ UNCHANGED <<stack, deferred, nodeCtr, rustCtr, roots, posts, postCtr, ghost>>

(***************************************************************************)
Quiet == /\ hops = {} /\ posts = {}
         /\ \A r \in Runtimes : /\ stack[r] = <<>> /\ deferred[r] = {}
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
    /\ nodeCtr = [r \in Runtimes |-> 0]
    /\ rustCtr = [r \in Runtimes |-> 0]
    /\ roots = 0
    /\ posts = {}
    /\ postCtr = 0
    /\ ghost = [misrouted |-> FALSE, missed |-> FALSE, refusedLegit |-> FALSE,
                falseCycle |-> FALSE, refused |-> FALSE, cycled |-> FALSE]

Next ==
    \/ \E r \in Runtimes :
          \/ StartRoot(r)
          \/ Return(r)
          \/ Resume(r)
          \/ \E t \in Runtimes : Call(r, t)
          \/ \E t \in Runtimes : Emit(r, t)
          \/ Receive(r)
          \/ \E f \in deferred[r] : RunDeferred(r, f)
          \/ RustForward(r)
          \/ RustReturn(r)
          \/ RustPost(r)
          \/ RustAnswered(r)
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
==============================================================================
