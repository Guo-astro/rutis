--------------------------------- MODULE EventsTable ---------------------------------
(**************************************************************************************)
(* The logical bus of one host and two runtimes: registration, withdrawal,            *)
(* `events.elsewhere`, dispatches on the local fast path and through the host, a      *)
(* row's `rows.load` reply, a row ending or loading again, a session ending and a     *)
(* new one starting, a prefix entry of the host, and a cross-process call whose      *)
(* receiver then dispatches (the §4.4 step 3 causality).                             *)
(*                                                                                    *)
(* Design: docs/design-events-protocol-2026-10-18.md §3 (scope is a descriptor),      *)
(* §4.1–§4.3 (registration, `events.on` / `events.off`, positions, prefixes),         *)
(* §4.4 (what a dispatch must see and must not), §4.5 (elsewhere, the full set first, *)
(* after a session ends it is empty), §5.1–§5.3 (snapshot, filter, fast path, via     *)
(* host), §5.4 (`emit` starts each call after the one before started), §7.1 (a row's  *)
(* listeners leave with it), §7.2 (a session ends).                                   *)
(* Implementation: crates/rutis/src/bus/table.rs (`select`, `Entry::start`),          *)
(* crates/rutis/src/bus.rs (`start`, `snapshot`),                                     *)
(* crates/rutis-bridge/src/events/hub.rs (`SessionEvents::on`, `off`, `row_ended`,    *)
(* `row_unloading`, `dispatch`), crates/rutis-bridge/src/session/rpc.rs (the frame    *)
(* order of `events.*` and `rows.ended`).                                             *)
(*                                                                                    *)
(* Simplifications (README.md, "Simplified"): one name, no scope, no values; a        *)
(* runtime's frames to the host and the host's to it are FIFO, each at most           *)
(* `QueueBound` deep; times are ghost sets ("what had happened"), not a clock.        *)
(* The prefix entry is the host's own Rust listener; runtimes never register one.     *)
(* A cross-process call is one frame from one runtime to another.                     *)
(* README.md lists the configurations and the results; expected.txt is the oracle.    *)
(**************************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC, EventsHost

CONSTANTS
    Runtimes,     \* the runtime sessions (model values: r1, r2)
    HostId,       \* the host's own process (a model value, not a runtime)
    Rows,         \* each runtime's rows ("a", "b")
    QueueBound,   \* frames one direction may hold
    MaxDispatch,  \* dispatches the whole run may issue
    MaxGen,       \* session generations (1, or 2 when a session may restart)
    MaxLoads,     \* registrations of one row in one session
    HostOwn,      \* the host may register its own Rust listener
    HostPrefix,   \* the host may register a prefix listener
    Withdraw,     \* a runtime may withdraw a listener (`events.off`)
    Unload,       \* the host may unload a row (`rows.unload`)
    EndRows,      \* a runtime may report a row ended (`rows.ended`)
    Reload,       \* a row may load again after it ended or unloaded
    Restart,      \* a session may end (and, with EndBeforeRestart, start again)
    EndBeforeRestart, \* a session first ends; only then may it restart
    AllowCrossCall, \* a runtime may call another, which then may dispatch
    PrependOn,    \* a listener of PrependRow goes in front
    PrependRow,   \* which row's listener (`PrependOn` only)
    Mutation      \* "none" or a deliberate bug (README.md, "Mutations")

ASSUME Runtimes /= {}
ASSUME Rows /= {}
ASSUME HostId \notin Runtimes
ASSUME QueueBound \in Nat \ {0}
ASSUME MaxDispatch \in Nat
ASSUME MaxGen \in {1, 2}
ASSUME MaxLoads \in Nat \ {0}
ASSUME Restart => MaxGen > 1
ASSUME Mutation \in {"none", "outOfOrder", "staleFast", "localOrder",
                     "loadReplyEarly", "freshFast", "replyBeforeElsewhere",
                     "prefixMissed", "dispatchFirst", "dupLocal"}

Entries == Runtimes \X Rows \X (1..MaxGen) \X (1..MaxLoads)

(* The host's own listener and its prefix listener: entries of another shape, *)
(* owned by the host.                                                         *)
HostEntry == <<HostId, HostId, 0, 0>>

HostPrefixEntry == <<HostId, HostId, 1, 0>>

EventName == "session/created"

PrefixText == "session/"

(* §4.3 / §9: a prefix entry matches every name it starts; every other entry  *)
(* here is the exact name.                                                    *)
EntryMatches(e, name) ==
    IF e = HostPrefixEntry
    THEN IsPrefixOf(PrefixText, name)
    ELSE IF e = HostEntry \/ e \in Entries
         THEN name = EventName
         ELSE FALSE

(* §4.3: a `prepend` listener takes a position before every other.            *)
Prepend ==
    IF PrependOn
    THEN { <<rt, PrependRow, g, l>> : rt \in Runtimes, g \in 1..MaxGen,
                                         l \in 1..MaxLoads }
    ELSE {}

OwnOf(rt, g) == { <<rt, r, g, l>> : r \in Rows, l \in 1..MaxLoads }

EntryOwner(e) == e[1]

EntryRow(e) == e[2]

EntryGen(e) == e[3]

EntryLoad(e) == e[4]

-------------------------------------------------------------------------------
(* Helpers                                                                    *)
-------------------------------------------------------------------------------

RECURSIVE RemoveAt(_, _)
RemoveAt(seq, i) ==
    IF seq = <<>> THEN <<>>
    ELSE IF i = 1 THEN Tail(seq)
    ELSE <<Head(seq)>> \o RemoveAt(Tail(seq), i - 1)

Before(seq, e, f) ==
    \E i, j \in 1..Len(seq): seq[i] = e /\ seq[j] = f /\ i < j

NoDup(seq) == \A i, j \in 1..Len(seq): i # j => seq[i] # seq[j]

RECURSIVE KeepIn(_, _)
KeepIn(seq, keep) ==
    IF seq = <<>> THEN <<>>
    ELSE IF Head(seq) \in keep
         THEN <<Head(seq)>> \o KeepIn(Tail(seq), keep)
         ELSE KeepIn(Tail(seq), keep)

(* §4.3 / §5.1: the host's matching snapshot.                                 *)
SnapshotOf(seq) ==
    KeepIn(seq, { e \in Elements(seq) : EntryMatches(e, EventName) })

-------------------------------------------------------------------------------
(* State                                                                      *)
-------------------------------------------------------------------------------

VARIABLES
    tbl,          \* the host's table: the entries, in position order
    hostOn,       \* the host's own Rust listener is in the table
    hostPrefixOn, \* the host's prefix listener is in the table
    ch,           \* runtime -> host: frames, FIFO, per session
    ack,          \* host -> runtime: frames (elsewhere, calls), FIFO
    local,        \* runtime -> its own live entries, in its call order
    gone,         \* runtime -> the entries it knows it withdrew
    rowst,        \* runtime -> row -> "loading" | "loaded" | "unloading" | "ended"
    loads,        \* runtime -> row -> which registration of the row (1, 2, ...)
    issuedInLoad, \* runtime -> row -> registrations issued while loading
    acceptedInLoad,\* runtime -> row -> those the host has accepted
    heard,        \* runtime -> row -> what the host had accepted at the reply
    replied,      \* runtime -> row -> `rows.load` has been answered
    toReply,      \* runtime -> entries whose `events.on` is accepted, not replied
    owe,          \* runtime -> the host owes it an elsewhere frame
    repliedTo,    \* runtime -> entries whose `events.on` reply reached it (ghost)
    heardCausal,  \* runtime -> entries whose replies crossed a call to it
    sess,         \* runtime -> "fresh" | "live" | "ended"
    gen,          \* runtime -> its session's generation
    elsewhere,    \* runtime -> the elsewhere set it has applied (one name)
    lastElse,     \* runtime -> 0: nothing queued; 1: TRUE; 2: FALSE
    nextD,        \* the next dispatch id
    everOn,       \* ghost: registrations issued so far
    everOff,      \* ghost: withdrawals made anywhere so far
    path,         \* dispatch -> "fast" | "host"
    snap,         \* dispatch -> the entries still to start, in call order
    seen,         \* dispatch -> the entries its snapshot held (ghost)
    order,        \* dispatch -> the entries it started, in order (ghost)
    onThen,       \* dispatch -> everOn when it was issued (ghost)
    offThen,      \* dispatch -> everOff when it was issued (ghost)
    offAtSnap,    \* dispatch -> everOff when its snapshot was taken (ghost)
    issuedOn,     \* dispatch -> the registrations its issuer knew at issue (ghost)
    issuedOff,    \* dispatch -> the withdrawals its issuer knew at issue (ghost)
    causalOn,     \* dispatch -> registrations known through a call (ghost)
    prefixAtSnap, \* dispatch -> the host's prefix entry was there at snapshot (ghost)
    freshAt       \* dispatch -> its session was fresh at issue (ghost)

vars == <<tbl, hostOn, hostPrefixOn, ch, ack, local, gone, rowst, issuedInLoad,
          acceptedInLoad, heard, replied, toReply, owe, repliedTo, heardCausal,
          sess, gen, loads, elsewhere, lastElse, nextD, everOn, everOff, path, snap,
          seen, order, onThen, offThen, offAtSnap, issuedOn, issuedOff, causalOn,
          prefixAtSnap, freshAt>>

Dom == 1..(nextD - 1)                 \* the dispatches issued so far

(* The runtimes are interchangeable, so TLC may explore one order for all.     *)
Perms == Permutations(Runtimes)

Handled == 1..MaxDispatch

(* The features that need the live elsewhere updates and the on reply: a cross *)
(* call, a prefix entry of the host, or an explicit end before a restart.      *)
LiveElsewhere == AllowCrossCall \/ HostPrefix \/ EndBeforeRestart

NowElsewhere(rt) ==
    \E e \in Elements(tbl):
        e \notin OwnOf(rt, gen[rt]) /\ EntryMatches(e, EventName)

Alive(e) ==
    CASE e = HostEntry -> hostOn
      [] e = HostPrefixEntry -> hostPrefixOn
      [] OTHER -> /\ e \in Elements(local[e[1]])
                  /\ EntryGen(e) = gen[e[1]]
                  /\ EntryLoad(e) = loads[e[1]][e[2]]

InTable(e) == e \in Elements(tbl)

EntryOf(rt, r) == <<rt, r, gen[rt], loads[rt][r]>>

-------------------------------------------------------------------------------
(* Frames                                                                     *)
-------------------------------------------------------------------------------

OnFrame(rt, r) ==
    [k |-> "on", e |-> EntryOf(rt, r), inLoad |-> rowst[rt][r] = "loading"]

OffFrame(rt, r) == [k |-> "off", e |-> EntryOf(rt, r)]

EndedFrame(rt, r) == [k |-> "ended", e |-> EntryOf(rt, r)]

DispFrame(d) == [k |-> "disp", d |-> d]

ElsewhereFrame(v) == [k |-> "elsewhere", v |-> v]

(* §4.4 step 3: one process calls a service in another; the receiver may then *)
(* dispatch, and what the caller already had a reply for is causal.          *)
CallFrame(from, to, r, carry) ==
    [k |-> "call", from |-> from, to |-> to, r |-> r, carry |-> carry]

CallToReceiver(f) ==
    [k |-> "call", from |-> f.from, to |-> f.to, r |-> f.r, carry |-> f.carry]

-------------------------------------------------------------------------------
(* What the runtimes do                                                       *)
-------------------------------------------------------------------------------

(* §4.2: a listener is registered during a row's apply; it enters the         *)
(* runtime's own order at once (program order), and its `events.on` goes to   *)
(* the host. §4.3: `prepend` goes in front everywhere.                        *)
Register(rt, r) ==
    /\ sess[rt] = "live"
    /\ rowst[rt][r] \in {"loading", "loaded"}
    (* `dupLocal`: the runtime's list takes a second registration of the same *)
    (* listener (`insert` appends to it and nothing rejects the copy), so one *)
    (* dispatch starts two calls of it.                                      *)
    /\ (Mutation = "dupLocal" \/ EntryOf(rt, r) \notin Elements(local[rt]))
    /\ EntryOf(rt, r) \notin gone[rt]
    /\ Len(ch[rt]) < QueueBound
    /\ LET e == EntryOf(rt, r)
           ordered ==
               IF Mutation = "localOrder" \/ e \notin Prepend
               THEN Append(local[rt], e)
               ELSE <<e>> \o local[rt]
       IN /\ local' = [local EXCEPT ![rt] = ordered]
          /\ ch' = [ch EXCEPT ![rt] = Append(@, OnFrame(rt, r))]
          /\ issuedInLoad' =
                 IF rowst[rt][r] = "loading"
                 THEN [issuedInLoad EXCEPT ![rt][r] = @ + 1]
                 ELSE issuedInLoad
          /\ everOn' = everOn \cup {e}
          /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ack, gone, rowst,
                         acceptedInLoad, heard, replied, toReply, owe, repliedTo,
                         heardCausal, sess, gen, elsewhere, lastElse, nextD,
                         everOff, path, snap, seen, order, onThen, offThen,
                         offAtSnap, issuedOn, issuedOff, causalOn, prefixAtSnap,
                         freshAt>>

(* `events.off`: the runtime knows the listener is gone before its frame       *)
(* reaches the host (§4.4 step 4).                                            *)
WithdrawEntry(rt, r) ==
    /\ Withdraw
    /\ sess[rt] = "live"
    /\ EntryOf(rt, r) \in Elements(local[rt])
    /\ Len(ch[rt]) < QueueBound
    /\ LET e == EntryOf(rt, r)
       IN /\ local' = [local EXCEPT ![rt] = KeepIn(@, Elements(local[rt]) \ {e})]
          /\ ch' = [ch EXCEPT ![rt] = Append(@, OffFrame(rt, r))]
          /\ gone' = [gone EXCEPT ![rt] = @ \cup {e}]
          /\ everOff' = everOff \cup {e}
          /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ack, rowst, issuedInLoad,
                         acceptedInLoad, heard, replied, toReply, owe, repliedTo,
                         heardCausal, sess, gen, elsewhere, lastElse, nextD,
                         everOn, path, snap, seen, order, onThen, offThen,
                         offAtSnap, issuedOn, issuedOff, causalOn, prefixAtSnap,
                         freshAt>>

(* §7.1: the runtime reports that a row ended; it counts as a withdrawal it    *)
(* knows.                                                                     *)
EndRow(rt, r) ==
    /\ EndRows
    /\ sess[rt] = "live"
    /\ rowst[rt][r] = "loaded"
    /\ Len(ch[rt]) < QueueBound
    /\ LET e == EntryOf(rt, r)
       IN /\ rowst' = [rowst EXCEPT ![rt][r] = "ended"]
          /\ local' = [local EXCEPT ![rt] = KeepIn(@, Elements(local[rt]) \ {e})]
          /\ ch' = [ch EXCEPT ![rt] = Append(@, EndedFrame(rt, r))]
          /\ gone' = [gone EXCEPT ![rt] = @ \cup {e}]
          /\ everOff' = everOff \cup {e}
          /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ack, issuedInLoad,
                         acceptedInLoad, heard, replied, toReply, owe, repliedTo,
                         heardCausal, sess, gen, elsewhere, lastElse, nextD,
                         everOn, path, snap, seen, order, onThen, offThen,
                         offAtSnap, issuedOn, issuedOff, causalOn, prefixAtSnap,
                         freshAt>>

(* §4.3 / §7.1: a row that ended or was unloaded may load again in the same   *)
(* session; its entries are new registrations (same id, new position).       *)
ReloadRow(rt, r) ==
    /\ Reload
    /\ sess[rt] = "live"
    /\ rowst[rt][r] \in {"ended", "unloading"}
    /\ loads[rt][r] < MaxLoads
    /\ EntryOf(rt, r) \notin Elements(tbl)
    /\ LET e == EntryOf(rt, r)
       IN /\ rowst' = [rowst EXCEPT ![rt][r] = "loading"]
          /\ loads' = [loads EXCEPT ![rt][r] = @ + 1]
          /\ gone' = [gone EXCEPT ![rt] = @ \ {e}]
          /\ issuedInLoad' = [issuedInLoad EXCEPT ![rt][r] = 0]
          /\ acceptedInLoad' = [acceptedInLoad EXCEPT ![rt][r] = 0]
          /\ heard' = [heard EXCEPT ![rt][r] = 0]
          /\ replied' = [replied EXCEPT ![rt][r] = FALSE]
          /\ UNCHANGED <<tbl, hostOn, hostPrefixOn, ch, ack, local, toReply, owe,
                         repliedTo, heardCausal, sess, gen, elsewhere, lastElse,
                         nextD, everOn, everOff, path, snap, seen, order, onThen,
                         offThen, offAtSnap, issuedOn, issuedOff, causalOn,
                         prefixAtSnap, freshAt>>

(* §4.5: a session applies the elsewhere set, in order; the first full set    *)
(* is what mounts a fresh session.                                           *)
ApplyElsewhere(rt) ==
    /\ sess[rt] \in {"fresh", "live"}
    /\ (sess[rt] = "fresh" => ch[rt] = <<>>)
    /\ ack[rt] /= <<>>
    /\ Head(ack[rt]).k = "elsewhere"
    /\ elsewhere' = [elsewhere EXCEPT ![rt] = Head(ack[rt]).v]
    /\ ack' = [ack EXCEPT ![rt] = Tail(@)]
    /\ sess' = [sess EXCEPT ![rt] = "live"]
    /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ch, local, gone, rowst,
                   issuedInLoad, acceptedInLoad, heard, replied, toReply, owe,
                   repliedTo, heardCausal, gen, lastElse, nextD, everOn, everOff,
                   path, snap, seen, order, onThen, offThen, offAtSnap, issuedOn,
                   issuedOff, causalOn, prefixAtSnap, freshAt>>

(* The fast path (§5.2, §5.5): a runtime dispatches itself when the name is    *)
(* in no elsewhere set. Its snapshot is its own entries in call order; every   *)
(* call starts at once (`emit`), checking the entry is still there first.      *)
DispatchFast(rt) ==
    /\ \/ /\ sess[rt] = "live"
          /\ ~elsewhere[rt]
       \/ Mutation = "freshFast"
    /\ nextD <= MaxDispatch
    /\ LET d == nextD
           snapshot ==
               IF Mutation = "staleFast"
               THEN KeepIn(tbl, OwnOf(rt, gen[rt]))
               ELSE local[rt]
       IN /\ path' = [path EXCEPT ![d] = "fast"]
          /\ snap' = [snap EXCEPT ![d] = snapshot]
          /\ seen' = [seen EXCEPT ![d] = [taken |-> TRUE, es |-> Elements(snapshot)]]
          /\ order' = [order EXCEPT ![d] = <<>>]
          /\ onThen' = [onThen EXCEPT ![d] = everOn]
          /\ offThen' = [offThen EXCEPT ![d] = everOff]
          /\ offAtSnap' = [offAtSnap EXCEPT ![d] = everOff]
          /\ issuedOn' = [issuedOn EXCEPT ![d] = Elements(local[rt])]
          /\ issuedOff' = [issuedOff EXCEPT ![d] = gone[rt]]
          /\ causalOn' = [causalOn EXCEPT ![d] = heardCausal[rt]]
          /\ prefixAtSnap' = [prefixAtSnap EXCEPT ![d] = FALSE]
          /\ freshAt' = [freshAt EXCEPT ![d] = sess[rt] = "fresh"]
          /\ nextD' = d + 1
          /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ch, ack, local, gone, rowst,
                         issuedInLoad, acceptedInLoad, heard, replied, toReply,
                         owe, repliedTo, heardCausal, sess, gen, elsewhere,
                         lastElse, everOn, everOff>>

(* One call of a fast-path dispatch: skip an entry that is gone, as if it had   *)
(* never been in the snapshot (§5.1 step 3). The staleFast mutant asks the      *)
(* host's table instead of the runtime's own list.                            *)
FastStart(rt) ==
    \E d \in Dom:
      /\ path[d] = "fast"
      /\ snap[d] /= <<>>
      /\ LET e == Head(snap[d])
             keep == IF Mutation = "staleFast" THEN InTable(e) ELSE Alive(e)
         IN /\ IF keep
               THEN order' = [order EXCEPT ![d] = Append(@, e)]
               ELSE order' = order
            /\ snap' = [snap EXCEPT ![d] = Tail(@)]
            /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ch, ack, local, gone,
                           rowst, issuedInLoad, acceptedInLoad, heard, replied,
                           toReply, owe, repliedTo, heardCausal, sess, gen,
                           elsewhere, lastElse, nextD, everOn, everOff, path,
                           seen, onThen, offThen, offAtSnap, issuedOn, issuedOff,
                           causalOn, prefixAtSnap, freshAt>>

(* §5.2: the name is elsewhere, so the runtime sends `events.dispatch`; the    *)
(* host takes the snapshot of its whole table.                                 *)
DispatchViaHost(rt) ==
    /\ sess[rt] = "live"
    /\ elsewhere[rt]
    /\ nextD <= MaxDispatch
    /\ Len(ch[rt]) < QueueBound
    /\ LET d == nextD
       IN /\ ch' = [ch EXCEPT ![rt] = Append(@, DispFrame(d))]
          /\ path' = [path EXCEPT ![d] = "host"]
          /\ onThen' = [onThen EXCEPT ![d] = everOn]
          /\ offThen' = [offThen EXCEPT ![d] = everOff]
          /\ issuedOn' = [issuedOn EXCEPT ![d] = Elements(local[rt])]
          /\ issuedOff' = [issuedOff EXCEPT ![d] = gone[rt]]
          /\ causalOn' = [causalOn EXCEPT ![d] = heardCausal[rt]]
          /\ prefixAtSnap' = [prefixAtSnap EXCEPT ![d] = FALSE]
          /\ freshAt' = [freshAt EXCEPT ![d] = FALSE]
          /\ nextD' = d + 1
          /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ack, local, gone, rowst,
                         issuedInLoad, acceptedInLoad, heard, replied, toReply,
                         owe, repliedTo, heardCausal, sess, gen, elsewhere,
                         lastElse, everOn, everOff, snap, seen, order, offAtSnap>>

(* §4.4 step 3: the plugin in `from` that got its `events.on` reply calls a    *)
(* service in `to`; the frame carries the replies the caller has seen.        *)
CrossCall(from, to, r) ==
    /\ AllowCrossCall
    /\ from # to
    /\ sess[from] = "live"
    /\ sess[to] = "live"
    /\ EntryOf(from, r) \in repliedTo[from]
    /\ Len(ch[from]) < QueueBound
    /\ ch' = [ch EXCEPT ![from] =
                Append(@, CallFrame(from, to, r, repliedTo[from]))]
    /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ack, local, gone, rowst,
                   issuedInLoad, acceptedInLoad, heard, replied, toReply, owe,
                   repliedTo, heardCausal, sess, gen, elsewhere, lastElse,
                   nextD, everOn, everOff, path, snap, seen, order, onThen,
                   offThen, offAtSnap, issuedOn, issuedOff, causalOn,
                   prefixAtSnap, freshAt>>

(* The receiver reads the call: everything the caller had heard of is now     *)
(* causal for a dispatch the call may cause.                                  *)
ReceiveCall(rt) ==
    /\ sess[rt] = "live"
    /\ ack[rt] /= <<>>
    /\ Head(ack[rt]).k = "call"
    /\ Head(ack[rt]).to = rt
    /\ heardCausal' = [heardCausal EXCEPT ![rt] =
                         @ \cup Head(ack[rt]).carry]
    /\ ack' = [ack EXCEPT ![rt] = Tail(@)]
    /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ch, local, gone, rowst,
                   issuedInLoad, acceptedInLoad, heard, replied, toReply, owe,
                   repliedTo, sess, gen, elsewhere, lastElse, nextD, everOn,
                   everOff, path, snap, seen, order, onThen, offThen, offAtSnap,
                   issuedOn, issuedOff, causalOn, prefixAtSnap, freshAt>>

-------------------------------------------------------------------------------
(* What the host does                                                         *)
-------------------------------------------------------------------------------

(* The host's own Rust listener (§4.2, §4.4 step 5).                          *)
RegisterHost ==
    /\ HostOwn
    /\ ~hostOn
    /\ everOn' = everOn \cup {HostEntry}
    /\ hostOn' = TRUE
    /\ tbl' = Append(tbl, HostEntry)
    /\ UNCHANGED <<loads, hostPrefixOn, ch, ack, local, gone, rowst, issuedInLoad,
                   acceptedInLoad, heard, replied, toReply, owe, repliedTo,
                   heardCausal, sess, gen, elsewhere, lastElse, nextD, everOff,
                   path, snap, seen, order, onThen, offThen, offAtSnap,
                   issuedOn, issuedOff, causalOn, prefixAtSnap, freshAt>>

(* §4.2 / §4.3: the host's own prefix listener. Runtimes never register one.   *)
RegisterHostPrefix ==
    /\ HostPrefix
    /\ ~hostPrefixOn
    /\ everOn' = everOn \cup {HostPrefixEntry}
    /\ hostPrefixOn' = TRUE
    /\ tbl' = Append(tbl, HostPrefixEntry)
    /\ UNCHANGED <<loads, hostOn, ch, ack, local, gone, rowst, issuedInLoad,
                   acceptedInLoad, heard, replied, toReply, owe, repliedTo,
                   heardCausal, sess, gen, elsewhere, lastElse, nextD, everOff,
                   path, snap, seen, order, onThen, offThen, offAtSnap,
                   issuedOn, issuedOff, causalOn, prefixAtSnap, freshAt>>

(* The frame a runtime sent, at index `i` of its queue. `outOfOrder` may take  *)
(* any of them (#587 before its fix); `dispatchFirst` lets an                 *)
(* `events.dispatch` pass the frames its session sent before it, so the host *)
(* takes its snapshot before the `events.on` of an entry the issuer already   *)
(* knew. Otherwise only the first.                                           *)
HostTakes(rt, i) ==
    IF Mutation = "outOfOrder"
    THEN TRUE
    ELSE IF Mutation = "dispatchFirst"
         THEN i = 1 \/ ch[rt][i].k = "disp"
         ELSE i = 1

HostOn(rt, i) ==
    /\ HostTakes(rt, i)
    /\ i \in 1..Len(ch[rt])
    /\ ch[rt][i].k = "on"
    /\ LET f == ch[rt][i]
           e == f.e
           \* §7.1: an `events.on` for a row that is unloading or gone is
           \* answered as usual but not kept.
           kept == rowst[e[1]][e[2]] \in {"loading", "loaded"}
                   /\ e = EntryOf(e[1], e[2])
                   /\ e \notin Elements(tbl)
       IN /\ tbl' = IF kept
                     THEN IF e \in Prepend THEN <<e>> \o tbl ELSE Append(tbl, e)
                     ELSE tbl
          /\ ch' = [ch EXCEPT ![rt] = RemoveAt(@, i)]
          /\ acceptedInLoad' =
                 IF f.inLoad
                 THEN [acceptedInLoad EXCEPT ![rt][EntryRow(e)] = @ + 1]
                 ELSE acceptedInLoad
          /\ toReply' =
                 IF LiveElsewhere
                 THEN [toReply EXCEPT ![rt] = @ \cup {e}]
                 ELSE toReply
          /\ owe' =
                 IF kept /\ LiveElsewhere
                 THEN [q \in Runtimes |->
                          owe[q] \/ (q # rt /\ sess[q] # "ended")]
                 ELSE owe
          /\ UNCHANGED <<loads, hostOn, hostPrefixOn, ack, local, gone, rowst,
                         issuedInLoad, heard, replied, repliedTo, heardCausal,
                         sess, gen, elsewhere, lastElse, nextD, everOn, everOff,
                         path, snap, seen, order, onThen, offThen, offAtSnap,
                         issuedOn, issuedOff, causalOn, prefixAtSnap, freshAt>>

HostOff(rt, i) ==
    /\ HostTakes(rt, i)
    /\ i \in 1..Len(ch[rt])
    /\ ch[rt][i].k = "off"
    /\ LET e == ch[rt][i].e
       IN /\ tbl' = KeepIn(tbl, Elements(tbl) \ {e})
          /\ ch' = [ch EXCEPT ![rt] = RemoveAt(@, i)]
          /\ everOff' = everOff \cup {e}
          /\ UNCHANGED <<loads, hostOn, hostPrefixOn, ack, local, gone, rowst,
                         issuedInLoad, acceptedInLoad, heard, replied, toReply,
                         owe, repliedTo, heardCausal, sess, gen, elsewhere,
                         lastElse, nextD, everOn, path, snap, seen, order,
                         onThen, offThen, offAtSnap, issuedOn, issuedOff,
                         causalOn, prefixAtSnap, freshAt>>

HostEnded(rt, i) ==
    /\ HostTakes(rt, i)
    /\ i \in 1..Len(ch[rt])
    /\ ch[rt][i].k = "ended"
    /\ LET e == ch[rt][i].e
       IN /\ tbl' = KeepIn(tbl, Elements(tbl) \ {e})
          /\ ch' = [ch EXCEPT ![rt] = RemoveAt(@, i)]
          /\ everOff' = everOff \cup {e}
          /\ UNCHANGED <<loads, hostOn, hostPrefixOn, ack, local, gone, rowst,
                         issuedInLoad, acceptedInLoad, heard, replied, toReply,
                         owe, repliedTo, heardCausal, sess, gen, elsewhere,
                         lastElse, nextD, everOn, path, snap, seen, order,
                         onThen, offThen, offAtSnap, issuedOn, issuedOff,
                         causalOn, prefixAtSnap, freshAt>>

HostDispatch(rt) ==
    /\ \E i \in 1..Len(ch[rt]):
         /\ HostTakes(rt, i)
         /\ ch[rt][i].k = "disp"
         /\ LET d == ch[rt][i].d
                snapshot ==
                    IF Mutation = "prefixMissed"
                    THEN KeepIn(tbl, Elements(tbl) \ {HostPrefixEntry})
                    ELSE SnapshotOf(tbl)
            IN /\ snap' = [snap EXCEPT ![d] = snapshot]
               /\ seen' = [seen EXCEPT ![d] =
                             [taken |-> TRUE, es |-> Elements(snapshot)]]
               /\ order' = [order EXCEPT ![d] = <<>>]
               /\ offAtSnap' = [offAtSnap EXCEPT ![d] = everOff]
               /\ prefixAtSnap' = [prefixAtSnap EXCEPT ![d] =
                                     HostPrefixEntry \in Elements(tbl)]
               /\ ch' = [ch EXCEPT ![rt] = RemoveAt(@, i)]
               /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ack, local, gone,
                              rowst, issuedInLoad, acceptedInLoad, heard,
                              replied, toReply, owe, repliedTo, heardCausal,
                              sess, gen, elsewhere, lastElse, nextD, everOn,
                              everOff, path, onThen, offThen, issuedOn,
                              issuedOff, causalOn, freshAt>>

(* One call of a host dispatch: the entry must still be in the table (§5.1     *)
(* step 3). Calls of a runtime's entry are its `events.call`; started counts   *)
(* here, at the dispatcher (§5.4).                                            *)
HostStart ==
    \E d \in Dom:
      /\ path[d] = "host"
      /\ snap[d] /= <<>>
      /\ LET e == Head(snap[d])
         IN /\ IF InTable(e)
               THEN order' = [order EXCEPT ![d] = Append(@, e)]
               ELSE order' = order
            /\ snap' = [snap EXCEPT ![d] = Tail(@)]
            /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ch, ack, local, gone,
                           rowst, issuedInLoad, acceptedInLoad, heard, replied,
                           toReply, owe, repliedTo, heardCausal, sess, gen,
                           elsewhere, lastElse, nextD, everOn, everOff, path,
                           seen, onThen, offThen, offAtSnap, issuedOn, issuedOff,
                           causalOn, prefixAtSnap, freshAt>>

(* §4.4 step 3: the host may reply `events.on` only after it queued the        *)
(* elsewhere this registration causes for every other session. `replyBefore`  *)
(* Elsewhere` is the behaviour before the guarantee was implemented.          *)
OnReply(rt) ==
    /\ LiveElsewhere
    /\ toReply[rt] /= {}
    /\ \/ Mutation = "replyBeforeElsewhere"
       \/ \A q \in Runtimes \ {rt}: ~owe[q]
    /\ repliedTo' = [repliedTo EXCEPT ![rt] = @ \cup toReply[rt]]
    /\ toReply' = [toReply EXCEPT ![rt] = {}]
    /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ch, ack, local, gone, rowst,
                   issuedInLoad, acceptedInLoad, heard, replied, owe,
                   heardCausal, sess, gen, elsewhere, lastElse, nextD, everOn,
                   everOff, path, snap, seen, order, onThen, offThen, offAtSnap,
                   issuedOn, issuedOff, causalOn, prefixAtSnap, freshAt>>

(* The host forwards the call to the other session; its queue is FIFO, so an   *)
(* elsewhere frame queued before the reply stays before the call.             *)
HostCall(rt, i) ==
    /\ HostTakes(rt, i)
    /\ i \in 1..Len(ch[rt])
    /\ ch[rt][i].k = "call"
    /\ ch[rt][i].from = rt
    /\ Len(ack[ch[rt][i].to]) < QueueBound
    /\ LET f == ch[rt][i]
       IN /\ ack' = [ack EXCEPT ![f.to] = Append(@, CallToReceiver(f))]
          /\ ch' = [ch EXCEPT ![rt] = RemoveAt(@, i)]
          /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, local, gone, rowst,
                         issuedInLoad, acceptedInLoad, heard, replied, toReply,
                         owe, repliedTo, heardCausal, sess, gen, elsewhere,
                         lastElse, nextD, everOn, everOff, path, snap, seen,
                         order, onThen, offThen, offAtSnap, issuedOn, issuedOff,
                         causalOn, prefixAtSnap, freshAt>>

(* §4.4 step 2: `rows.load` is answered only after the registrations its apply *)
(* made have been accepted.                                                   *)
LoadReply(rt, r) ==
    /\ rowst[rt][r] = "loading"
    /\ \/ Mutation = "loadReplyEarly"
       \/ acceptedInLoad[rt][r] = issuedInLoad[rt][r]
    /\ rowst' = [rowst EXCEPT ![rt][r] = "loaded"]
    /\ heard' = [heard EXCEPT ![rt][r] = acceptedInLoad[rt][r]]
    /\ replied' = [replied EXCEPT ![rt][r] = TRUE]
    /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ch, ack, local, gone,
                   issuedInLoad, acceptedInLoad, toReply, owe, repliedTo,
                   heardCausal, sess, gen, elsewhere, lastElse, nextD, everOn,
                   everOff, path, snap, seen, order, onThen, offThen, offAtSnap,
                   issuedOn, issuedOff, causalOn, prefixAtSnap, freshAt>>

(* §7.1 step 1: the host unloads a row; its listeners leave at once.           *)
UnloadRow(rt, r) ==
    /\ Unload
    /\ rowst[rt][r] = "loaded"
    /\ LET e == EntryOf(rt, r)
       IN /\ rowst' = [rowst EXCEPT ![rt][r] = "unloading"]
          /\ local' = [local EXCEPT ![rt] = KeepIn(@, Elements(local[rt]) \ {e})]
          /\ gone' = [gone EXCEPT ![rt] = @ \cup {e}]
          /\ tbl' = KeepIn(tbl, Elements(tbl) \ {e})
          /\ everOff' = everOff \cup {e}
          /\ UNCHANGED <<loads, hostOn, hostPrefixOn, ch, ack, issuedInLoad,
                         acceptedInLoad, heard, replied, toReply, owe, repliedTo,
                         heardCausal, sess, gen, elsewhere, lastElse, nextD,
                         everOn, path, snap, seen, order, onThen, offThen,
                         offAtSnap, issuedOn, issuedOff, causalOn, prefixAtSnap,
                         freshAt>>

(* §4.5: at the start of a session the host queues the full elsewhere set      *)
(* first; afterwards only changes.                                            *)
SendElsewhere(rt) ==
    /\ \/ LiveElsewhere /\ owe[rt]
       \/ lastElse[rt] = 0
       \/ (lastElse[rt] = 1) # NowElsewhere(rt)
    /\ Len(ack[rt]) < QueueBound
    /\ \/ sess[rt] = "live"
       \/ /\ sess[rt] = "fresh"
          /\ Len(ack[rt]) = 0
    /\ ack' = [ack EXCEPT ![rt] = Append(@, ElsewhereFrame(NowElsewhere(rt)))]
    /\ lastElse' = [lastElse EXCEPT ![rt] = IF NowElsewhere(rt) THEN 1 ELSE 2]
    /\ owe' = [owe EXCEPT ![rt] = FALSE]
    /\ UNCHANGED <<loads, tbl, hostOn, hostPrefixOn, ch, local, gone, rowst,
                   issuedInLoad, acceptedInLoad, heard, replied, toReply,
                   repliedTo, heardCausal, sess, gen, elsewhere, nextD, everOn,
                   everOff, path, snap, seen, order, onThen, offThen, offAtSnap,
                   issuedOn, issuedOff, causalOn, prefixAtSnap, freshAt>>

(* §7.2: the session ends. Its entries leave the table, its frames are         *)
(* dropped; a dispatch already holding a snapshot must not start a call.      *)
EndSession(rt) ==
    /\ EndBeforeRestart
    /\ sess[rt] = "live"
    /\ LET old == OwnOf(rt, gen[rt])
       IN /\ tbl' = KeepIn(tbl, Elements(tbl) \ old)
          /\ everOff' = everOff \cup old
          /\ sess' = [sess EXCEPT ![rt] = "ended"]
          /\ ch' = [ch EXCEPT ![rt] = <<>>]
          /\ ack' = [ack EXCEPT ![rt] = <<>>]
          /\ local' = [local EXCEPT ![rt] = <<>>]
          /\ toReply' = [toReply EXCEPT ![rt] = {}]
          /\ owe' = [owe EXCEPT ![rt] = FALSE]
          /\ heardCausal' = [heardCausal EXCEPT ![rt] = {}]
          /\ elsewhere' = [elsewhere EXCEPT ![rt] = FALSE]
          /\ lastElse' = [lastElse EXCEPT ![rt] = 0]
          /\ rowst' = [rowst EXCEPT ![rt] = [r \in Rows |-> "ended"]]
          /\ UNCHANGED <<loads, hostOn, hostPrefixOn, gone, issuedInLoad,
                         acceptedInLoad, heard, replied, repliedTo, gen, nextD,
                         everOn, path, snap, seen, order, onThen, offThen,
                         offAtSnap, issuedOn, issuedOff, causalOn, prefixAtSnap,
                         freshAt>>

(* The session ends and a new one begins: the old entries leave the table, its *)
(* frames are dropped, the runtime is fresh again and must apply the full      *)
(* elsewhere set before it dispatches (§4.5, §7.2). Only with no snapshot      *)
(* still starting, so no call of the old session starts in the new one.       *)
RestartSession(rt) ==
    /\ Restart
    /\ gen[rt] < MaxGen
    /\ \/ EndBeforeRestart /\ sess[rt] = "ended"
       \/ ~EndBeforeRestart /\ sess[rt] = "live"
    /\ \A d \in Dom: snap[d] = <<>>
    /\ LET old == OwnOf(rt, gen[rt])
       IN /\ tbl' = KeepIn(tbl, Elements(tbl) \ old)
          /\ everOff' = everOff \cup old
          /\ gen' = [gen EXCEPT ![rt] = @ + 1]
          /\ sess' = [sess EXCEPT ![rt] = "fresh"]
          /\ ch' = [ch EXCEPT ![rt] = <<>>]
          /\ ack' = [ack EXCEPT ![rt] = <<>>]
          /\ local' = [local EXCEPT ![rt] = <<>>]
          /\ gone' = [gone EXCEPT ![rt] = {}]
          /\ rowst' = [rowst EXCEPT ![rt] = [r \in Rows |-> "loading"]]
          /\ loads' = [loads EXCEPT ![rt] = [r \in Rows |-> 1]]
          /\ issuedInLoad' = [issuedInLoad EXCEPT ![rt] = [r \in Rows |-> 0]]
          /\ acceptedInLoad' = [acceptedInLoad EXCEPT ![rt] =
                                  [r \in Rows |-> 0]]
          /\ heard' = [heard EXCEPT ![rt] = [r \in Rows |-> 0]]
          /\ replied' = [replied EXCEPT ![rt] = [r \in Rows |-> FALSE]]
          /\ toReply' = [toReply EXCEPT ![rt] = {}]
          /\ owe' = [owe EXCEPT ![rt] = FALSE]
          /\ repliedTo' = [repliedTo EXCEPT ![rt] = {}]
          /\ heardCausal' = [heardCausal EXCEPT ![rt] = {}]
          /\ elsewhere' = [elsewhere EXCEPT ![rt] = FALSE]
          /\ lastElse' = [lastElse EXCEPT ![rt] = 0]
          /\ UNCHANGED <<hostOn, hostPrefixOn, nextD, everOn, path, snap, seen,
                         order, onThen, offThen, offAtSnap, issuedOn, issuedOff,
                         causalOn, prefixAtSnap, freshAt>>

(* Nothing is left to do: every dispatch started all its calls, every          *)
(* registration got its reply, and no frame is queued. A stuttering step, so   *)
(* the model has no dead end of its own.                                      *)
Done ==
    /\ \A d \in Dom: snap[d] = <<>>
    /\ ch = [rt \in Runtimes |-> <<>>]
    /\ ack = [rt \in Runtimes |-> <<>>]
    /\ (LiveElsewhere => (\A rt \in Runtimes: toReply[rt] = {} /\ ~owe[rt]))
    /\ UNCHANGED vars

Next ==
    \/ \E rt \in Runtimes, r \in Rows: Register(rt, r)
    \/ \E rt \in Runtimes, r \in Rows: WithdrawEntry(rt, r)
    \/ \E rt \in Runtimes, r \in Rows: EndRow(rt, r)
    \/ \E rt \in Runtimes, r \in Rows: ReloadRow(rt, r)
    \/ \E rt \in Runtimes: ApplyElsewhere(rt)
    \/ \E rt \in Runtimes: DispatchFast(rt)
    \/ \E rt \in Runtimes: FastStart(rt)
    \/ \E rt \in Runtimes: DispatchViaHost(rt)
    \/ \E from, to \in Runtimes, r \in Rows: CrossCall(from, to, r)
    \/ \E rt \in Runtimes: ReceiveCall(rt)
    \/ RegisterHost
    \/ RegisterHostPrefix
    \/ \E rt \in Runtimes: \E i \in 1..Len(ch[rt]): HostOn(rt, i)
    \/ \E rt \in Runtimes: \E i \in 1..Len(ch[rt]): HostOff(rt, i)
    \/ \E rt \in Runtimes: \E i \in 1..Len(ch[rt]): HostEnded(rt, i)
    \/ \E rt \in Runtimes: HostDispatch(rt)
    \/ \E rt \in Runtimes: \E i \in 1..Len(ch[rt]): HostCall(rt, i)
    \/ HostStart
    \/ \E rt \in Runtimes: OnReply(rt)
    \/ \E rt \in Runtimes, r \in Rows: LoadReply(rt, r)
    \/ \E rt \in Runtimes, r \in Rows: UnloadRow(rt, r)
    \/ \E rt \in Runtimes: SendElsewhere(rt)
    \/ \E rt \in Runtimes: EndSession(rt)
    \/ \E rt \in Runtimes: RestartSession(rt)
    \/ Done

Init ==
    /\ tbl = <<>>
    /\ hostOn = FALSE
    /\ hostPrefixOn = FALSE
    /\ ch = [rt \in Runtimes |-> <<>>]
    /\ ack = [rt \in Runtimes |-> <<>>]
    /\ local = [rt \in Runtimes |-> <<>>]
    /\ gone = [rt \in Runtimes |-> {}]
    /\ rowst = [rt \in Runtimes |-> [r \in Rows |-> "loading"]]
    /\ loads = [rt \in Runtimes |-> [r \in Rows |-> 1]]
    /\ issuedInLoad = [rt \in Runtimes |-> [r \in Rows |-> 0]]
    /\ acceptedInLoad = [rt \in Runtimes |-> [r \in Rows |-> 0]]
    /\ heard = [rt \in Runtimes |-> [r \in Rows |-> 0]]
    /\ replied = [rt \in Runtimes |-> [r \in Rows |-> FALSE]]
    /\ toReply = [rt \in Runtimes |-> {}]
    /\ owe = [rt \in Runtimes |-> FALSE]
    /\ repliedTo = [rt \in Runtimes |-> {}]
    /\ heardCausal = [rt \in Runtimes |-> {}]
    /\ sess = [rt \in Runtimes |-> "fresh"]
    /\ gen = [rt \in Runtimes |-> 1]
    /\ elsewhere = [rt \in Runtimes |-> FALSE]
    /\ lastElse = [rt \in Runtimes |-> 0]
    /\ nextD = 1
    /\ everOn = {}
    /\ everOff = {}
    /\ path = [d \in Handled |-> "fast"]
    /\ snap = [d \in Handled |-> <<>>]
    /\ seen = [d \in Handled |-> [taken |-> FALSE, es |-> {}]]
    /\ order = [d \in Handled |-> <<>>]
    /\ onThen = [d \in Handled |-> {}]
    /\ offThen = [d \in Handled |-> {}]
    /\ offAtSnap = [d \in Handled |-> {}]
    /\ issuedOn = [d \in Handled |-> {}]
    /\ issuedOff = [d \in Handled |-> {}]
    /\ causalOn = [d \in Handled |-> {}]
    /\ prefixAtSnap = [d \in Handled |-> FALSE]
    /\ freshAt = [d \in Handled |-> FALSE]

Spec == Init /\ [][Next]_vars

-------------------------------------------------------------------------------
(* Properties (issue #613, "性质")                                            *)
-------------------------------------------------------------------------------

(* §4.3 / §5.4: anywhere two dispatches start the same two entries, the order   *)
(* is the same.                                                               *)
OneOrder ==
    \A d1, d2 \in Dom:
      \A e, f \in Elements(order[d1]) \cap Elements(order[d2]):
        e # f => Before(order[d1], e, f) = Before(order[d2], e, f)

(* §4.4 steps 1 and 3: a dispatch whose issuer knew a registration, of an      *)
(* entry not withdrawn anywhere by its snapshot, holds it.                     *)
RegisteredThenSeen ==
    \A d \in Dom:
      seen[d].taken =>
        \A e \in issuedOn[d]:
          e \notin offAtSnap[d] => e \in seen[d].es

(* §4.4 step 3: a dispatch caused by a cross-process call holds every          *)
(* registration whose reply the caller had seen before the call.              *)
CausallyAfterCallSees ==
    \A d \in Dom:
      seen[d].taken =>
        \A e \in causalOn[d]:
          e \notin offAtSnap[d] => e \in seen[d].es

(* §4.4 step 4, §7.1: an entry its issuer knew withdrawn at issue starts no    *)
(* call of that dispatch.                                                      *)
WithdrawnStaysOut ==
    \A d \in Dom:
      \A e \in issuedOff[d]: e \notin Elements(order[d])

(* §7.1, §7.2: at most one call of an entry per dispatch.                     *)
OneCallPerEntry ==
    \A d \in Dom: NoDup(order[d])

(* §4.4 step 2: a row is Running only after the listeners its apply registered *)
(* were accepted.                                                              *)
RunningRowHeard ==
    \A rt \in Runtimes, r \in Rows:
      replied[rt][r] => heard[rt][r] = issuedInLoad[rt][r]

(* §4.5: no dispatch before a new session applied the full elsewhere set.      *)
NoDispatchBeforeElsewhere ==
    \A d \in Dom: ~freshAt[d]

(* §4.3 / §5.1: a prefix entry in the host's table at snapshot time is in the  *)
(* host's snapshot; a host that matches by exact name only loses it.          *)
PrefixEntrySeen ==
    \A d \in Dom:
      prefixAtSnap[d] => HostPrefixEntry \in seen[d].es

(* The protocol does not promise this (issue #613, "先定下的一点"): every       *)
(* dispatch sees every entry registered before it was issued, which a fast     *)
(* path with a stale elsewhere set, or a dispatch from another session that    *)
(* arrived before its `events.on`, cannot do.  Expected violated.             *)
SeesAll ==
    \A d \in Dom:
      seen[d].taken =>
        \A e \in onThen[d] \ offThen[d]:
          e \in seen[d].es

=======================================================================================
