--------------------------- MODULE CallRelay ----------------------------
(***************************************************************************)
(* A call through a relay: A holds a reference that M forwarded from C,    *)
(* calls it through M, cancels, and gives the reference back. Each hop     *)
(* resolves its cancel with an acknowledgement (protocol 5, #1099).        *)
(*                                                                         *)
(* Three participants over two sessions, A--M and M--C. The object lives   *)
(* at C; C granted it to M, so M holds an import; M granted a relay of it  *)
(* to A, so M exports an object of its own whose body is that import. A    *)
(* call on the relay is forwarded by M to C with a new call number in M's  *)
(* session with C; M keeps the incoming call (`awaiting`) until C answers, *)
(* and then answers A with A's own number.                                 *)
(*                                                                         *)
(* The model answers the relay part of #626 (SESS-59, SESS-54, and         *)
(* rules M1-7.1-1 / M1-7.1-2 of docs/formal-trace/rules/protocol-        *)
(* session.md):                                                            *)
(*  - A cancel reaches C. Reading `cancel` for a call M still forwards     *)
(*    drops the forwarded request, and dropping it sends `cancel` on M's    *)
(*    session with C (`CancelOnDrop` on the `request_async` future the      *)
(*    relay spawned). A cancel for a call M already answered is not passed *)
(*    on: nothing at C is left to cancel (SESS-58), and the caller's own   *)
(*    reply may still be in flight to A.                                   *)
(*  - A release reaches C. A releasing its last grant of the relay drops   *)
(*    the export entry at M, which drops the relay object, which drops the *)
(*    import it held, which writes `release {reference, count}` on M's     *)
(*    session with C. The count is the import's, so it travels the chain   *)
(*    without a new frame kind (M1-7.1-2).                                 *)
(*  - Each hop resolves its cancel with an acknowledgement (#1099): M     *)
(*    acknowledges A (or answers, if A's cancel crossed the answer), and C *)
(*    acknowledges M. The acknowledgements drain both cancelled tables; a  *)
(*    reply after one is the mutation `relay-mut-ack-then-reply`.          *)
(*                                                                         *)
(* Design: docs/protocol-session.md §5.4 (SESS-41, SESS-42), §6.1         *)
(* (SESS-47, SESS-51), §6.2 (SESS-54), §7 (SESS-56..SESS-61);             *)
(* docs/formal-trace.md §3.4; docs/guarantees.md                          *)
(* `bridge/multihop-cancel-and-release`; docs/formal-trace/rules/         *)
(* protocol-session.md M1-7.1-1, M1-7.1-2; issue #626; #1099 for the      *)
(* cancelled table.                                                       *)
(* Implementation: crates/rutis-bridge/src/session/rpc/relay.rs (`relay`   *)
(* and `encode_import`), crates/rutis-bridge/src/session/rpc.rs           *)
(* (`Frame::Cancel`, `Connection::cancel`, `CancelOnDrop`, `reply_target`, *)
(* `Import`'s `Drop` writing the release), and `calls.cancelled` /        *)
(* `calls.awaiting`. README.md next to this file lists the               *)
(* simplifications.                                                        *)
(*                                                                         *)
(* Each session is a pair of FIFO channels delayed arbitrarily long, so a  *)
(* cancel may overtake the reply it was meant to cancel and a release may  *)
(* cross a reply. No close is modelled here: `CallLifecycle.tla` covers    *)
(* it, and the relay's release on a close has a shape of its own (the      *)
(* write is refused by the closed session, so the owner keeps the grant    *)
(* until its own session ends; README, "未覆盖").                          *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Calls,      \* the calls A may start ("c1", "c2"), in no fixed order
    MaxCancels, \* how often A may cancel in the session (1)
    Mutation,   \* "none", or a deliberate bug (README, "Mutations")
    Replies,    \* after a cancel: "always" or "never" (SESS-61)
    Acks        \* whether each hop acknowledges a cancel it resolves (#1099)

ASSUME Calls /= {}
ASSUME MaxCancels \in Nat
ASSUME Mutation \in {"none", "cancelStopsAtRelay", "releaseLeaks"}
ASSUME Replies \in {"always", "never"}
ASSUME Acks \in BOOLEAN

VARIABLES
    acall,      \* per call at A: "idle" | "waiting" | "settled" |
                \*   "cancelled"
    anext,      \* the number A gives its next call
    aid,        \* per call: the number A gave it (0 before it started)
    aTable,     \* per call: A's table for the reply: "none" | "waiting" |
                \*   "cancelled"
    aHeld,      \* grants of the relay A holds
    aReleased,  \* ghost: grant units A gave back
    cancels,    \* how often A cancelled
    toM,        \* A -> M: <<"invoke", call, number>>, <<"cancel", call>>,
                \*   <<"release", count>>
    toA,        \* M -> A: <<"return", call, number>>
    toC,        \* M -> C: <<"call", call, number>>, <<"cancel", call>>,
                \*   <<"release", count>>
    toM2,       \* C -> M (named after its reader): <<"return", call, number>>
    minc,       \* per call at M: what the incoming table holds: "none" |
                \*   "queued" | "running" | "done" | "dropped"
    mfwd,       \* per call at M: the forwarded call: "none" | "waiting" |
                \*   "settled" | "cancelled"
    mfid,       \* per call: the number M gave the forward (0 before)
    mtable,     \* per call: M's table for the forwarded reply: "none" |
                \*   "waiting" | "cancelled"
    mnext,      \* the number M gives its next forwarded call
    mRelay,     \* grants of the relay M exported to A
    mRelayBack, \* ghost: grant units M took back from A
    mImport,    \* grants of the object M holds (the relay's import)
    mImportBack,\* ghost: grant units M gave back to C
    mOrphans,   \* per call: late replies M discarded
    ccall,      \* per call at C: "none" | "queued" | "running" | "done" |
                \*   "dropped"
    cseq,       \* per call: the number C read with the call
    ccancelled, \* per call: a cancel reached C while it was running the call
    cGrants,    \* grants of the object C exports
    cReleased,  \* ghost: grant units C gave back
    aOrphans,   \* per call: late replies A discarded
    settles,    \* per call: how often it was settled at A
    wasOutstanding, \* ghost: per call, M read A's cancel while still
                \*   forwarding it
    reachedC,   \* ghost: per call, C read a cancel for it
    mStray      \* ghost: a reply arrived after M's acknowledgement

vars == <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM, toA,
          toC, toM2, minc, mfwd, mfid, mtable, mnext, mRelay, mRelayBack,
          mImport, mImportBack, mOrphans, ccall, cseq, ccancelled, cGrants,
          cReleased, aOrphans, settles, wasOutstanding, reachedC, mStray>>

Init ==
    /\ acall = [c \in Calls |-> "idle"]
    /\ anext = 1
    /\ aid = [c \in Calls |-> 0]
    /\ aTable = [c \in Calls |-> "none"]
    /\ aHeld = 1
    /\ aReleased = 0
    /\ cancels = 0
    /\ toM = <<>>
    /\ toA = <<>>
    /\ toC = <<>>
    /\ toM2 = <<>>
    /\ minc = [c \in Calls |-> "none"]
    /\ mfwd = [c \in Calls |-> "none"]
    /\ mfid = [c \in Calls |-> 0]
    /\ mtable = [c \in Calls |-> "none"]
    /\ mnext = 1
    /\ mRelay = 1
    /\ mRelayBack = 0
    /\ mImport = 1
    /\ mImportBack = 0
    /\ mOrphans = [c \in Calls |-> 0]
    /\ ccall = [c \in Calls |-> "none"]
    /\ cseq = [c \in Calls |-> 0]
    /\ ccancelled = [c \in Calls |-> FALSE]
    /\ cGrants = 1
    /\ cReleased = 0
    /\ aOrphans = [c \in Calls |-> 0]
    /\ settles = [c \in Calls |-> 0]
    /\ wasOutstanding = [c \in Calls |-> FALSE]
    /\ reachedC = [c \in Calls |-> FALSE]
    /\ mStray = FALSE

(***************************************************************************)
(* The four channels. A frame goes out as it is made; each direction keeps *)
(* its order and is delayed arbitrarily long.                              *)
(***************************************************************************)

SendToM(f) == toM' = Append(toM, f)
SendToA(f) == toA' = Append(toA, f)
SendToC(f) == toC' = Append(toC, f)
SendFromC(f) == toM2' = Append(toM2, f)

(***************************************************************************)
(* A: starting a call on the relay, reading the answer, cancelling, and    *)
(* giving the reference back.                                              *)
(***************************************************************************)

\* A call takes the next number. It names the relay, so A must hold a grant
\* of it.
AStart(c) ==
    /\ acall[c] = "idle"
    /\ aHeld > 0
    /\ acall' = [acall EXCEPT ![c] = "waiting"]
    /\ aid' = [aid EXCEPT ![c] = anext]
    /\ aTable' = [aTable EXCEPT ![c] = "waiting"]
    /\ anext' = anext + 1
    /\ SendToM(<<"invoke", c, anext>>)
    /\ UNCHANGED <<aHeld, aReleased, cancels, toA, toC, toM2, minc, mfwd, mfid,
                   mtable, mnext, mRelay, mRelayBack, mImport, mImportBack,
                   mOrphans, ccall, cseq, ccancelled, cGrants, cReleased,
                   aOrphans, settles, wasOutstanding, reachedC, mStray>>

\* The answer to a call A still waits for: settled once.
ADeliverLive(c) ==
    /\ toA /= <<>>
    /\ Head(toA)[1] = "return"
    /\ Head(toA)[3] = aid[c]
    /\ acall[c] = "waiting"
    /\ toA' = Tail(toA)
    /\ acall' = [acall EXCEPT ![c] = "settled"]
    /\ aTable' = [aTable EXCEPT ![c] = "none"]
    /\ settles' = [settles EXCEPT ![c] = @ + 1]
    /\ UNCHANGED <<anext, aid, aHeld, aReleased, cancels, toM, toC, toM2, minc,
                   mfwd, mfid, mtable, mnext, mRelay, mRelayBack, mImport,
                   mImportBack, mOrphans, ccall, cseq, ccancelled, cGrants,
                   cReleased, aOrphans, wasOutstanding, reachedC, mStray>>

\* The answer to a call A already cancelled: discarded, the session goes on.
ADeliverLate(c) ==
    /\ toA /= <<>>
    /\ Head(toA)[1] = "return"
    /\ Head(toA)[3] = aid[c]
    /\ acall[c] = "cancelled"
    /\ aTable[c] = "cancelled"
    /\ toA' = Tail(toA)
    /\ aTable' = [aTable EXCEPT ![c] = "none"]
    /\ aOrphans' = [aOrphans EXCEPT ![c] = @ + 1]
    /\ UNCHANGED <<acall, anext, aid, aHeld, aReleased, cancels, toM, toC, toM2,
                   minc, mfwd, mfid, mtable, mnext, mRelay, mRelayBack,
                   mImport, mImportBack, mOrphans, ccall, cseq, ccancelled,
                   cGrants, cReleased, settles, wasOutstanding, reachedC, mStray>>

\* M acknowledged the cancel: A's entry goes. An ack whose entry is gone
\* crossed the answer that cleared it and changes nothing (#1099).
AReadAck(c) ==
    /\ toA /= <<>>
    /\ Head(toA) = <<"ack", c>>
    /\ acall[c] = "cancelled"
    /\ aTable[c] \in {"cancelled", "none"}
    /\ toA' = Tail(toA)
    /\ aTable' = IF aTable[c] = "cancelled"
                 THEN [aTable EXCEPT ![c] = "none"] ELSE aTable
    /\ UNCHANGED <<acall, anext, aid, aHeld, aReleased, cancels, toM, toC,
                   toM2, minc, mfwd, mfid, mtable, mnext, mRelay, mRelayBack,
                   mImport, mImportBack, mOrphans, ccall, cseq, ccancelled,
                   cGrants, cReleased, aOrphans, settles, wasOutstanding,
                   reachedC, mStray>>

\* A gives the call up. The cancel goes out on A's session with M; it is M's
\* job to pass it on while it still forwards the call.
ACancel(c) ==
    /\ acall[c] = "waiting"
    /\ cancels < MaxCancels
    /\ cancels' = cancels + 1
    /\ acall' = [acall EXCEPT ![c] = "cancelled"]
    /\ aTable' = [aTable EXCEPT ![c] = "cancelled"]
    /\ settles' = [settles EXCEPT ![c] = @ + 1]
    /\ SendToM(<<"cancel", c>>)
    /\ UNCHANGED <<anext, aid, aHeld, aReleased, toA, toC, toM2, minc, mfwd,
                   mfid, mtable, mnext, mRelay, mRelayBack, mImport,
                   mImportBack, mOrphans, ccall, cseq, ccancelled, cGrants,
                   cReleased, aOrphans, wasOutstanding, reachedC, mStray>>

\* A gives a grant of the relay back: the count is what A holds now.
ARelease(n) ==
    /\ n \in 1..aHeld
    /\ aHeld' = aHeld - n
    /\ aReleased' = aReleased + n
    /\ SendToM(<<"release", n>>)
    /\ UNCHANGED <<acall, anext, aid, aTable, cancels, toA, toC, toM2, minc,
                   mfwd, mfid, mtable, mnext, mRelay, mRelayBack, mImport,
                   mImportBack, mOrphans, ccall, cseq, ccancelled, cGrants,
                   cReleased, aOrphans, settles, wasOutstanding, reachedC, mStray>>

(***************************************************************************)
(* M, the relay: admitting the call, forwarding it, reading the owner's    *)
(* answer, passing a cancel on, and dropping the relay when its last grant *)
(* goes.                                                                   *)
(***************************************************************************)

\* M takes the call; it stays queued until an executor runs it.
MReadInvoke(c) ==
    /\ toM /= <<>>
    /\ Head(toM)[1] = "invoke"
    /\ Head(toM)[2] = c
    /\ minc[c] = "none"
    /\ minc' = [minc EXCEPT ![c] = "queued"]
    /\ toM' = Tail(toM)
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toA,
                   toC, toM2, mfwd, mfid, mtable, mnext, mRelay, mRelayBack,
                   mImport, mImportBack, mOrphans, ccall, cseq, ccancelled,
                   cGrants, cReleased, aOrphans, settles, wasOutstanding,
                   reachedC, mStray>>

\* M runs it on the background executor and forwards it: a call of M's own
\* on the session with C, with M's number, and an entry in M's table for the
\* answer (`awaiting`).
MForward(c) ==
    /\ minc[c] = "queued"
    /\ minc' = [minc EXCEPT ![c] = "running"]
    /\ mfwd' = [mfwd EXCEPT ![c] = "waiting"]
    /\ mfid' = [mfid EXCEPT ![c] = mnext]
    /\ mtable' = [mtable EXCEPT ![c] = "waiting"]
    /\ mnext' = mnext + 1
    /\ SendToC(<<"call", c, mnext>>)
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toM2, mRelay, mRelayBack, mImport, mImportBack,
                   mOrphans, ccall, cseq, ccancelled, cGrants, cReleased,
                   aOrphans, settles, wasOutstanding, reachedC, mStray>>

\* C's answer to the forwarded call: M settles its own call and answers A
\* with A's number.
MDeliverLive(c) ==
    /\ toM2 /= <<>>
    /\ Head(toM2)[1] = "return"
    /\ Head(toM2)[2] = c
    /\ Head(toM2)[3] = mfid[c]
    /\ mfwd[c] = "waiting"
    /\ toM2' = Tail(toM2)
    /\ mfwd' = [mfwd EXCEPT ![c] = "settled"]
    /\ mtable' = [mtable EXCEPT ![c] = "none"]
    /\ minc' = [minc EXCEPT ![c] = "done"]
    /\ SendToA(<<"return", c, aid[c]>>)
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toC, mfid, mnext, mRelay, mRelayBack, mImport,
                   mImportBack, mOrphans, ccall, cseq, ccancelled, cGrants,
                   cReleased, aOrphans, settles, wasOutstanding, reachedC, mStray>>

\* C's answer to a call M cancelled while forwarding it: discarded, counted.
MDeliverLate(c) ==
    /\ toM2 /= <<>>
    /\ Head(toM2)[1] = "return"
    /\ Head(toM2)[2] = c
    /\ Head(toM2)[3] = mfid[c]
    /\ mfwd[c] = "cancelled"
    /\ mtable[c] = "cancelled"
    /\ toM2' = Tail(toM2)
    /\ mtable' = [mtable EXCEPT ![c] = "none"]
    /\ mOrphans' = [mOrphans EXCEPT ![c] = @ + 1]
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toC, minc, mfwd, mfid, mnext, mRelay, mRelayBack,
                   mImport, mImportBack, ccall, cseq, ccancelled, cGrants,
                   cReleased, aOrphans, settles, wasOutstanding, reachedC, mStray>>

\* C acknowledged the cancel: M's entry for the forwarded call goes. An ack
\* whose entry is gone crossed the answer that cleared it (#1099).
MReadAck(c) ==
    /\ toM2 /= <<>>
    /\ Head(toM2) = <<"ack", c>>
    /\ mfwd[c] = "cancelled"
    /\ mtable[c] \in {"cancelled", "none"}
    /\ toM2' = Tail(toM2)
    /\ mtable' = IF mtable[c] = "cancelled"
                  THEN [mtable EXCEPT ![c] = "none"] ELSE mtable
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toC, minc, mfwd, mfid, mnext, mRelay, mRelayBack,
                   mImport, mImportBack, mOrphans, ccall, cseq, ccancelled,
                   cGrants, cReleased, aOrphans, settles, wasOutstanding,
                   reachedC, mStray>>

\* A reply C wrote after acknowledging M's cancel: M has no entry for it and
\* never answers A. A conforming callee never sends one
\* (`relay-mut-ack-then-reply`); the ghost records it.
MDeliverStray(c) ==
    /\ toM2 /= <<>>
    /\ Head(toM2)[1] = "return"
    /\ Head(toM2)[2] = c
    /\ Head(toM2)[3] = mfid[c]
    /\ mfwd[c] = "cancelled"
    /\ mtable[c] = "none"
    /\ toM2' = Tail(toM2)
    /\ mStray' = TRUE
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toC, minc, mfwd, mfid, mtable, mnext, mRelay,
                   mRelayBack, mImport, mImportBack, mOrphans, ccall, cseq,
                   ccancelled, cGrants, cReleased, aOrphans, settles,
                   wasOutstanding, reachedC>>

\* A's cancel arrives. A call still queued here never runs and is dropped; a
\* call already answered is not touched (the cancel crossed its answer); a
\* call still being forwarded is cancelled, and dropping the forwarded
\* request sends `cancel` on to C (SESS-59). The mutation drops the entry
\* without passing the cancel on.
MReadCancel(c) ==
    /\ toM /= <<>>
    /\ Head(toM) = <<"cancel", c>>
    /\ toM' = Tail(toM)
    /\ LET acted == minc[c] \in {"queued", "running"} \/ mfwd[c] = "waiting"
       IN /\ IF minc[c] = "queued" THEN minc' = [minc EXCEPT ![c] = "dropped"]
             ELSE IF minc[c] = "running" THEN minc' = [minc EXCEPT ![c] = "done"]
             ELSE UNCHANGED minc
          /\ wasOutstanding' = [wasOutstanding EXCEPT
                                   ![c] = (mfwd[c] = "waiting")]
          /\ IF mfwd[c] = "waiting"
             THEN /\ mfwd' = [mfwd EXCEPT ![c] = "cancelled"]
                  /\ mtable' = [mtable EXCEPT ![c] = "cancelled"]
                  /\ IF Mutation = "cancelStopsAtRelay"
                     THEN UNCHANGED toC
                     ELSE SendToC(<<"cancel", c>>)
             ELSE /\ UNCHANGED <<mfwd, mtable, toC>>
          \* M had the call: it acknowledges A and answers no more. A reply
          \* written before this cancel was read is already on its way.
          /\ IF Acks /\ acted
             THEN SendToA(<<"ack", c>>)
             ELSE UNCHANGED toA
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels,
                   toM2, mfid, mnext, mRelay, mRelayBack, mImport,
                   mImportBack, mOrphans, ccall, cseq, ccancelled, cGrants,
                   cReleased, aOrphans, settles, reachedC, mStray>>

\* A's release arrives: the relay's grants go down. When the last one is
\* gone the export entry is dropped, the relay object with it, and the import
\* it held: dropping the import writes `release {reference, count}` on M's
\* session with C, with the count the import holds. The mutation drops the
\* relay without giving the import back.
MReadRelease(n) ==
    /\ toM /= <<>>
    /\ Head(toM) = <<"release", n>>
    /\ n \in 1..mRelay
    /\ toM' = Tail(toM)
    /\ mRelay' = mRelay - n
    /\ mRelayBack' = mRelayBack + n
    /\ IF mRelay - n = 0
       THEN IF Mutation = "releaseLeaks"
            THEN UNCHANGED <<mImport, mImportBack, toC>>
            ELSE /\ SendToC(<<"release", mImport>>)
                 /\ mImportBack' = mImportBack + mImport
                 /\ mImport' = 0
       ELSE UNCHANGED <<mImport, mImportBack, toC>>
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toA,
                   toM2, minc, mfwd, mfid, mtable, mnext, mOrphans, ccall,
                   cseq, ccancelled, cGrants, cReleased, aOrphans, settles,
                   wasOutstanding, reachedC, mStray>>

(***************************************************************************)
(* C, the owner: taking the forwarded call, running it, answering, and     *)
(* reading a cancel or a release.                                          *)
(***************************************************************************)

CReadCall(c) ==
    /\ toC /= <<>>
    /\ Head(toC)[1] = "call"
    /\ Head(toC)[2] = c
    /\ ccall[c] = "none"
    /\ ccall' = [ccall EXCEPT ![c] = "queued"]
    /\ cseq' = [cseq EXCEPT ![c] = Head(toC)[3]]
    /\ toC' = Tail(toC)
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toM2, minc, mfwd, mfid, mtable, mnext, mRelay,
                   mRelayBack, mImport, mImportBack, mOrphans, ccancelled,
                   cGrants, cReleased, aOrphans, settles, wasOutstanding,
                   reachedC, mStray>>

CRun(c) ==
    /\ ccall[c] = "queued"
    /\ ccall' = [ccall EXCEPT ![c] = "running"]
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toC, toM2, minc, mfwd, mfid, mtable, mnext, mRelay,
                   mRelayBack, mImport, mImportBack, mOrphans, cseq,
                   ccancelled, cGrants, cReleased, aOrphans, settles,
                   wasOutstanding, reachedC, mStray>>

\* The method answers; a running call answers also after a cancel when the
\* rule says so (`Replies = "always"`). A queued one is dropped instead
\* (CReadCancel), and one cancelled while running is dropped without an
\* answer when the rule says `"never"` (CDrop).
CReply(c) ==
    /\ ccall[c] = "running"
    /\ Replies = "always" \/ ~ccancelled[c]
    /\ ccall' = [ccall EXCEPT ![c] = "done"]
    /\ SendFromC(<<"return", c, cseq[c]>>)
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toC, minc, mfwd, mfid, mtable, mnext, mRelay,
                   mRelayBack, mImport, mImportBack, mOrphans, cseq,
                   ccancelled, cGrants, cReleased, aOrphans, settles,
                   wasOutstanding, reachedC, mStray>>

CDrop(c) ==
    /\ Replies = "never"
    /\ ccall[c] = "running"
    /\ ccancelled[c]
    /\ ccall' = [ccall EXCEPT ![c] = "dropped"]
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toC, toM2, minc, mfwd, mfid, mtable, mnext, mRelay,
                   mRelayBack, mImport, mImportBack, mOrphans, cseq,
                   ccancelled, cGrants, cReleased, aOrphans, settles,
                   wasOutstanding, reachedC, mStray>>

\* A cancel arrives. One still queued is dropped without an answer; one
\* already running keeps running and answers, or is dropped by the rule;
\* one already answered is not touched (SESS-58). `reachedC` records that the
\* cancel made both hops.
CReadCancel(c) ==
    /\ toC /= <<>>
    /\ Head(toC) = <<"cancel", c>>
    /\ toC' = Tail(toC)
    /\ ccall' = IF ccall[c] = "queued" THEN [ccall EXCEPT ![c] = "dropped"]
               ELSE ccall
    /\ ccancelled' = IF Replies = "never" /\ ccall[c] = "running"
                     THEN [ccancelled EXCEPT ![c] = TRUE] ELSE ccancelled
    /\ reachedC' = [reachedC EXCEPT ![c] = TRUE]
    \* C had the call: it acknowledges M and answers no more.
    /\ IF Acks /\ ccall[c] \in {"queued", "running"}
       THEN SendFromC(<<"ack", c>>)
       ELSE UNCHANGED toM2
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, minc, mfwd, mfid, mtable, mnext, mRelay,
                   mRelayBack, mImport, mImportBack, mOrphans, cseq, cGrants,
                   cReleased, aOrphans, settles, wasOutstanding, mStray>>

\* A release arrives: the grants it names are given up.
CReadRelease(n) ==
    /\ toC /= <<>>
    /\ Head(toC) = <<"release", n>>
    /\ n \in 1..cGrants
    /\ toC' = Tail(toC)
    /\ cGrants' = cGrants - n
    /\ cReleased' = cReleased + n
    /\ UNCHANGED <<acall, anext, aid, aTable, aHeld, aReleased, cancels, toM,
                   toA, toM2, minc, mfwd, mfid, mtable, mnext, mRelay,
                   mRelayBack, mImport, mImportBack, mOrphans, ccall, cseq,
                   ccancelled, aOrphans, settles, wasOutstanding, reachedC, mStray>>

(***************************************************************************)
(* The specification.                                                      *)
(***************************************************************************)

\* Nothing is left to do: no frame waits anywhere, no call is in flight at
\* any of the three participants.
Quiescent ==
    /\ toM = <<>>
    /\ toA = <<>>
    /\ toC = <<>>
    /\ toM2 = <<>>
    /\ \A c \in Calls : acall[c] # "waiting"
    /\ \A c \in Calls : minc[c] \notin {"queued", "running"}
    /\ \A c \in Calls : mfwd[c] # "waiting"
    /\ \A c \in Calls : ccall[c] \notin {"queued", "running"}

\* No frame is left and M forwards nothing any more: a cancel M still had to
\* pass on has been read by C. C's own call may still be running when the
\* cancel never got there (the mutation), which is the point.
CancelSettled ==
    /\ toM = <<>>
    /\ toA = <<>>
    /\ toC = <<>>
    /\ toM2 = <<>>
    /\ \A c \in Calls : acall[c] # "waiting"
    /\ \A c \in Calls : mfwd[c] # "waiting"

Done ==
    /\ Quiescent
    /\ UNCHANGED vars

Next ==
    \/ \E c \in Calls : AStart(c) \/ ADeliverLive(c) \/ ADeliverLate(c)
         \/ AReadAck(c) \/ ACancel(c) \/ MReadInvoke(c) \/ MForward(c)
         \/ MDeliverLive(c) \/ MDeliverLate(c) \/ MReadAck(c)
         \/ MDeliverStray(c) \/ MReadCancel(c) \/ CReadCall(c) \/ CRun(c)
         \/ CReply(c) \/ CDrop(c) \/ CReadCancel(c)
    \/ \E n \in 1..aHeld : ARelease(n)
    \/ \E n \in 1..mRelay : MReadRelease(n)
    \/ \E n \in 1..cGrants : CReadRelease(n)
    \/ Done

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)

\* A cancel for a call M was still forwarding reaches C: M passes it on, and
\* C reads it before the sessions are quiet. A cancel for a call M already
\* answered is not passed on — there is nothing left at C to cancel.
CancelReaches ==
    CancelSettled => \A c \in Calls : wasOutstanding[c] => reachedC[c]

\* The relay gives the original reference back when its last grant goes:
\* while A holds none of the relay, the relay has dropped the import and C
\* holds no grant either (M1-7.1-2). The mutation that drops the relay
\* without giving the import back leaves C's export behind.
RelayReleasesImport ==
    Quiescent => (aHeld = 0 => (mImport = 0 /\ cGrants = 0))

\* A grant count travels the chain and is never counted twice: what A gave
\* back is at least what M took back, which is at least what C gave back —
\* and no hop gives back more than the one before it handed it.
GrantsConserved ==
    /\ aHeld + aReleased = 1
    /\ mRelay + mRelayBack = 1
    /\ mImport + mImportBack = 1
    /\ cGrants + cReleased = 1
    /\ mRelayBack <= aReleased
    /\ cReleased <= mImportBack

\* A call is settled at most once at A.
SettlesAtMostOnce == \A c \in Calls : settles[c] <= 1

\* A late reply is discarded at either hop: the cancelled call keeps the
\* outcome it got, and each call has at most one reply to discard per hop.
LateReplyDropped ==
    \A c \in Calls : (mOrphans[c] <= 1) /\ (aOrphans[c] <= 1)

\* A relayed call still settles on the caller's side: the answer comes back
\* through M.
WaitingSettles ==
    \A c \in Calls : (acall[c] = "waiting") ~> (acall[c] \in {"settled",
                                                             "cancelled"})

\* Each hop's cancelled table drains: the acknowledgement resolves the
\* entries (#1099). Without acknowledgements (`relay-gap-cancel-table`) the
\* call each hop cancelled while queued is answered by nobody, so entries
\* stay — the same gap as on the caller's side (`CancelledEntriesDrain` in
\* `CallLifecycle.tla`), one hop further along.
RelayCancelledEntriesDrain ==
    Quiescent => /\ \A c \in Calls : aTable[c] # "cancelled"
                 /\ \A c \in Calls : mtable[c] # "cancelled"

\* No acknowledgement is followed by a reply: a conforming callee resolves
\* each cancel once (`relay-mut-ack-then-reply` is the mutation).
NoStrayReply == ~mStray

=============================================================================
