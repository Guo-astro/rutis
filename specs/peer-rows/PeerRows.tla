--------------------------- MODULE PeerRows ----------------------------
(***************************************************************************)
(* Node-hosted rows (peer rows): the far end's rows run here as a          *)
(* session's rows, under the key the peer loads each one by. KeyPerLoad    *)
(* writes the key rule: FALSE is the old `PeerRow`'s instance key, the     *)
(* same across restarts, so a notice must carry the generation it belongs  *)
(* to; TRUE is a `run` row's key, new on every load, so the key itself     *)
(* names the load (decided for #1100: TRUE; see README, "The key"). Two    *)
(* control sessions of the same peer (the old one breaking, a new one      *)
(* taking over) with the host's per-session table of what it loaded, and   *)
(* what a late notification of the old session must not do to the new      *)
(* generation.                                                             *)
(*                                                                         *)
(* Design: docs/design-remote-plugins-2026-10-03.md §4.4 (a new session is *)
(* answered only after the old lease's rows are cleaned; a late old        *)
(* session instruction must not act on the new lease) and §5.1–§5.3 (the   *)
(* host row, `plugins.load` / `plugins.unload`, and the peer's rows);      *)
(* docs/design-remote-session-2026-10-28.md §4.2 (a replacement unloads    *)
(* the old session first) and §4.3 (the row of a connection waits for its  *)
(* session, runs on it, stops with it); docs/formal-trace.md §3.0 (the     *)
(* session and row lifecycle).                                             *)
(* Implementation: crates/rutis-bridge/src/session/plugin/rows.rs          *)
(* (`Server.loaded` is the session's own table, a key already loaded is    *)
(* refused, and `follow` reports one ending, none once the session is      *)
(* cancelled), crates/rutis-bridge/src/session/plugin/services.rs (a       *)
(* session's imports, withdrawn with the row that provided them),          *)
(* crates/rutis-bridge/src/runtime/process.rs (`on_row_ended` /            *)
(* `on_row_failed` by key, `unload_row`, `forget_exports`), and            *)
(* crates/rutis-bridge/src/session/plugin/run.rs (a run row stops with its *)
(* session and runs again on the next one).                                *)
(* README.md next to this file lists the simplifications.                  *)
(*                                                                         *)
(* Sessions are the peer's control connections, opened in order. A row's   *)
(* generation names one load. Under a stable key (`KeyPerLoad = FALSE`,    *)
(* the old `PeerRow`) the key is the row's instance, the same in every     *)
(* session, so a notification carries the generation whose registration    *)
(* the host had loaded, and the peer must match it rather than the key     *)
(* alone. Under a key per load (`KeyPerLoad = TRUE`, a current `run` row)  *)
(* the key is the generation: a notice of an old load names a key no load  *)
(* here has. Each direction of a session is FIFO and delayed               *)
(* arbitrarily long; a session that broke is not read any more, but a      *)
(* notification already written may still reach the row late (`PLateEnded` *)
(* and `PLateFailed`).                                                     *)
(*                                                                         *)
(* The key: #1100 decided the second rule — a remote row takes             *)
(* a new key on every load (`<plugin>#<n>` in `run.rs:168`,                *)
(* `<instance>.<generation>` in a loader row, `runtime.rs:722-727`), so    *)
(* a notice of an old load names a key no row here has. `KeyPerLoad =      *)
(* TRUE` writes that rule; `peer-fresh-keys` checks it, with the           *)
(* `lateEnding` policy (match by the key alone) that is unsafe under a     *)
(* bare instance key. The first rule stays in the model: it is what the    *)
(* old `PeerRow` did, and what a key without a generation would have to    *)
(* satisfy. See README, "The key".                                         *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Rows,        \* the peer's rows hosted here, one instance each ("a", "b")
    MaxSessions, \* control sessions the peer may open (1 + reconnects)
    MaxGens,     \* loads of one row across all its sessions
    Faults,      \* what the environment may do: "lose"
    Failures,    \* a hosted row may fail (rows.failed)
    SelfEnds,    \* a hosted row may end on its own (rows.ended)
    KeyPerLoad,  \* TRUE: a key per load (a `run` row); FALSE: the instance
    Mutation     \* "none", or a deliberate bug (README, "Mutations")

ASSUME Rows /= {}
ASSUME MaxSessions \in Nat \ {0}
ASSUME MaxGens \in Nat \ {0}
ASSUME Faults \subseteq {"lose"}
ASSUME Failures \in BOOLEAN
ASSUME SelfEnds \in BOOLEAN
ASSUME KeyPerLoad \in BOOLEAN
ASSUME Mutation \in {"none", "acceptBeforeClean", "lateEnding", "keepRows",
                     "serviceResidue"}

Sessions == 1..MaxSessions
Gens == 1..MaxGens
Keys == Sessions \X Rows \X Gens
NoGen == 0
NoSvc == <<0, 0>>

\* The key a notice of generation g names: a key per load is that generation;
\* a stable key is the row, the same for every generation.
RowKey(r, g) == IF KeyPerLoad THEN <<r, g>> ELSE r

VARIABLES
    pst,          \* the peer: "connecting" | "greeting" | "ready" | "waiting"
    pconn,        \* the session the peer is on, 0 when none
    pdial,        \* sessions the peer opened
    prow,         \* per row: "idle" | "loading" | "loaded" | "unloading" | "gone"
    pkey,         \* per row: the generation of its current (or last) load
    pgen,         \* per row: generations used
    pEndedFrom,   \* ghost: the generation whose ending ended the row
    pFailedFrom,  \* ghost: the generation whose failure restarted the row
    svc,          \* per row: the imported service's <<session, generation>>
    late,         \* left over from an ended session: <<kind, row, generation>>
    cur,          \* the host: the session it serves, 0 when none
    pick,         \* the session whose hello it took up, awaiting the answer
    rside,        \* per session at the host: "none" | "open" | "closed"
    answered,     \* per session: the host answered its hello
    ended,        \* per session: the host ended it
    broke,        \* per session: the channel is gone
    loaded,       \* per session at the host, per row: the generation it loaded
    sentFail,     \* generations whose `rows.failed` the host sent
    clean,        \* ghost: how often a row's generation was disposed
    toH,          \* per session: frames to the host, FIFO
    toP           \* per session: frames to the peer, FIFO

vars == <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom, pFailedFrom, svc,
          late, cur, pick, rside, answered, ended, broke, loaded, sentFail,
          clean, toH, toP>>

Init ==
    /\ pst = "connecting"
    /\ pconn = 0
    /\ pdial = 0
    /\ prow = [r \in Rows |-> "idle"]
    /\ pkey = [r \in Rows |-> NoGen]
    /\ pgen = [r \in Rows |-> 0]
    /\ pEndedFrom = [r \in Rows |-> NoGen]
    /\ pFailedFrom = [r \in Rows |-> NoGen]
    /\ svc = [r \in Rows |-> NoSvc]
    /\ late = {}
    /\ cur = 0
    /\ pick = 0
    /\ rside = [s \in Sessions |-> "none"]
    /\ answered = [s \in Sessions |-> FALSE]
    /\ ended = [s \in Sessions |-> FALSE]
    /\ broke = [s \in Sessions |-> FALSE]
    /\ loaded = [s \in Sessions |-> [r \in Rows |-> NoGen]]
    /\ sentFail = {}
    /\ clean = [k \in Keys |-> 0]
    /\ toH = [s \in Sessions |-> <<>>]
    /\ toP = [s \in Sessions |-> <<>>]

\* A side that closed a session reads nothing more on it.
CanRead(s) == cur = s /\ rside[s] = "open"

\* The session ended and nothing of it is loaded any more.
Swept(s) == ended[s] /\ \A r \in Rows : loaded[s][r] = NoGen

\* What an ended session had written but the peer has not read.
Frames(s) == {toP[s][i] : i \in 1..Len(toP[s])}
Report(f) == IF f[1] = "ended" \/ f[1] = "failed"
            THEN {<<f[1], f[2], f[3]>>} ELSE {}
Reports(s) == UNION {Report(f) : f \in Frames(s)}

(***************************************************************************)
(* The peer: its rows and the control session it is on.                    *)
(***************************************************************************)

\* The peer opens its next session and greets.
PDial ==
    /\ pst \in {"connecting", "waiting"}
    /\ pdial < MaxSessions
    /\ LET n == pdial + 1 IN
       /\ pdial' = n
       /\ pconn' = n
       /\ pst' = "greeting"
       /\ toH' = [toH EXCEPT ![n] = Append(@, <<"hello">>)]
       /\ UNCHANGED <<prow, pkey, pgen, pEndedFrom, pFailedFrom, svc, late,
                      cur, pick, rside, answered, ended, broke, loaded,
                      sentFail, clean, toP>>

PGreeted ==
    /\ pst = "greeting"
    /\ pconn > 0
    /\ toP[pconn] /= <<>>
    /\ Head(toP[pconn]) = <<"greeted">>
    /\ pst' = "ready"
    /\ toP' = [toP EXCEPT ![pconn] = Tail(@)]
    /\ UNCHANGED <<pconn, pdial, prow, pkey, pgen, pEndedFrom, pFailedFrom,
                   svc, late, cur, pick, rside, answered, ended, broke,
                   loaded, sentFail, clean, toH>>

\* Load a row on the session the peer is on: the generation is what this load
\* is called; the key is the generation (`KeyPerLoad`) or the instance (the
\* same after every restart).
PLoad(r) ==
    /\ pst = "ready"
    /\ pconn > 0
    /\ prow[r] = "idle"
    /\ pgen[r] < MaxGens
    /\ LET g == pgen[r] + 1 IN
       /\ pgen' = [pgen EXCEPT ![r] = g]
       /\ pkey' = [pkey EXCEPT ![r] = g]
       /\ pEndedFrom' = [pEndedFrom EXCEPT ![r] = NoGen]
       /\ pFailedFrom' = [pFailedFrom EXCEPT ![r] = NoGen]
       /\ prow' = [prow EXCEPT ![r] = "loading"]
       /\ toH' = [toH EXCEPT ![pconn] = Append(@, <<"load", r, g>>)]
       /\ UNCHANGED <<pst, pconn, pdial, svc, late, cur, pick, rside, answered,
                      ended, broke, loaded, sentFail, clean, toP>>

PLoaded(r) ==
    /\ pst = "ready"
    /\ pconn > 0
    /\ toP[pconn] /= <<>>
    /\ LET f == Head(toP[pconn]) IN
       /\ f[1] = "loaded" /\ f[2] = r
       /\ LET g == f[3] IN
          /\ toP' = [toP EXCEPT ![pconn] = Tail(@)]
          /\ prow' = IF prow[r] = "loading" /\ pkey[r] = g
                     THEN [prow EXCEPT ![r] = "loaded"] ELSE prow
          /\ UNCHANGED <<pst, pconn, pdial, pkey, pgen, pEndedFrom, pFailedFrom, svc,
                         late, cur, pick, rside, answered, ended, broke,
                         loaded, sentFail, clean, toH>>

\* The row is disposed here (the loader removed it): unload it there.
PUnload(r) ==
    /\ pst = "ready"
    /\ pconn > 0
    /\ prow[r] = "loaded"
    /\ prow' = [prow EXCEPT ![r] = "unloading"]
    /\ toH' = [toH EXCEPT ![pconn] = Append(@, <<"unload", r>>)]
    /\ UNCHANGED <<pst, pconn, pdial, pkey, pgen, pEndedFrom, pFailedFrom, svc, late,
                   cur, pick, rside, answered, ended, broke, loaded,
                   sentFail, clean, toP>>

PUnloaded(r) ==
    /\ pst = "ready"
    /\ pconn > 0
    /\ toP[pconn] /= <<>>
    /\ LET f == Head(toP[pconn]) IN
       /\ f[1] = "unloaded" /\ f[2] = r
       /\ toP' = [toP EXCEPT ![pconn] = Tail(@)]
       /\ prow' = IF prow[r] = "unloading"
                  THEN [prow EXCEPT ![r] = "idle"] ELSE prow
       /\ UNCHANGED <<pst, pconn, pdial, pkey, pgen, pEndedFrom, pFailedFrom, svc,
                      late, cur, pick, rside, answered, ended, broke,
                      loaded, sentFail, clean, toH>>

\* `rows.ended`: the row ended there on its own. Only the generation it
\* belongs to ends; a late ending of an earlier generation does not. (Under
\* a stable key the key alone does not tell them apart; under a key per load
\* it does — `PLateEnded` writes the difference.)
PEnded(r) ==
    /\ pst = "ready"
    /\ pconn > 0
    /\ toP[pconn] /= <<>>
    /\ LET f == Head(toP[pconn]) IN
       /\ f[1] = "ended" /\ f[2] = r
       /\ LET g == f[3]
              acts == pkey[r] = g /\ prow[r] # "gone"
          IN /\ toP' = [toP EXCEPT ![pconn] = Tail(@)]
             /\ prow' = IF acts THEN [prow EXCEPT ![r] = "gone"] ELSE prow
             /\ pEndedFrom' = IF acts
                              THEN [pEndedFrom EXCEPT ![r] = g]
                              ELSE pEndedFrom
             /\ UNCHANGED <<pst, pconn, pdial, pkey, pgen, pFailedFrom, svc, late,
                            cur, pick, rside, answered, ended, broke,
                            loaded, sentFail, clean, toH>>

\* `rows.failed`: the row's code failed. The row stays loaded there; the
\* peer unloads the generation and loads a new one after. A failure of an
\* earlier generation is ignored.
PFailed(r) ==
    /\ pst = "ready"
    /\ pconn > 0
    /\ toP[pconn] /= <<>>
    /\ LET f == Head(toP[pconn]) IN
       /\ f[1] = "failed" /\ f[2] = r
       /\ LET g == f[3]
              acts == pkey[r] = g /\ prow[r] = "loaded"
          IN /\ toP' = [toP EXCEPT ![pconn] = Tail(@)]
             /\ prow' = IF acts
                        THEN [prow EXCEPT ![r] = "unloading"] ELSE prow
             /\ pFailedFrom' = IF acts
                               THEN [pFailedFrom EXCEPT ![r] = g]
                               ELSE pFailedFrom
             /\ toH' = IF acts
                       THEN [toH EXCEPT
                               ![pconn] = Append(@, <<"unload", r>>)]
                       ELSE toH
             /\ UNCHANGED <<pst, pconn, pdial, pkey, pgen, pEndedFrom, svc, late,
                            cur, pick, rside, answered, ended, broke,
                            loaded, sentFail, clean>>

\* The session broke: its rows stop and wait for the next one; what it
\* imported goes with it. Notifications the session had not delivered yet
\* may still reach their row (`PLateEnded`, `PLateFailed`).
PLoss(s) ==
    /\ pconn = s
    /\ pst \in {"greeting", "ready"}
    /\ broke[s]
    /\ pst' = "waiting"
    /\ pconn' = 0
    /\ prow' = [r \in Rows |-> IF prow[r] = "gone" THEN "gone" ELSE "idle"]
    /\ svc' = IF Mutation = "serviceResidue"
              THEN svc
              ELSE [r \in Rows |-> IF svc[r][1] = s THEN NoSvc ELSE svc[r]]
    /\ late' = late \cup Reports(s)
    /\ toH' = [toH EXCEPT ![s] = <<>>]
    /\ toP' = [toP EXCEPT ![s] = <<>>]
    /\ UNCHANGED <<pdial, pkey, pgen, pEndedFrom, pFailedFrom, cur, pick,
                   rside, answered, ended, broke, loaded, sentFail, clean>>

\* An ending already written on the session that broke, delivered after
\* the row moved on. The protocol ignores it: it is not this generation's.
\* The mutation acts on the row that is loaded now if the notice names its
\* key: under a stable key every generation's notice names it (`lateEnding`
\* ends the new generation); under a key per load only this load's does, and
\* a late notice of an earlier one names a key no row has.
PLateEnded(r, g) ==
    /\ <<"ended", r, g>> \in late
    /\ late' = late \ {<<"ended", r, g>>}
    /\ LET acts == Mutation = "lateEnding"
                  /\ prow[r] = "loaded"
                  /\ RowKey(r, g) = RowKey(r, pkey[r])
       IN /\ prow' = IF acts THEN [prow EXCEPT ![r] = "gone"] ELSE prow
          /\ pEndedFrom' = IF acts
                           THEN [pEndedFrom EXCEPT ![r] = g] ELSE pEndedFrom
    /\ UNCHANGED <<pst, pconn, pdial, pkey, pgen, pFailedFrom, svc, cur, pick,
                   rside, answered, ended, broke, loaded, sentFail, clean,
                   toH, toP>>

\* The same for a failure: the protocol ignores it, the mutation restarts
\* the row that is there now.
PLateFailed(r, g) ==
    /\ <<"failed", r, g>> \in late
    /\ late' = late \ {<<"failed", r, g>>}
    /\ LET acts == Mutation = "lateEnding"
                  /\ prow[r] = "loaded"
                  /\ pconn > 0
                  /\ RowKey(r, g) = RowKey(r, pkey[r])
          IN /\ prow' = IF acts THEN [prow EXCEPT ![r] = "unloading"] ELSE prow
             /\ pFailedFrom' = IF acts
                               THEN [pFailedFrom EXCEPT ![r] = g]
                               ELSE pFailedFrom
             /\ toH' = IF acts
                       THEN [toH EXCEPT
                               ![pconn] = Append(@, <<"unload", r>>)]
                       ELSE toH
    /\ UNCHANGED <<pst, pconn, pdial, pkey, pgen, pEndedFrom, svc, cur, pick,
                   rside, answered, ended, broke, loaded, sentFail, clean,
                   toP>>

(***************************************************************************)
(* The host: the session it serves and the rows loaded in each session.    *)
(***************************************************************************)

\* The host takes up a new connection's hello. It closes the old session
\* and starts cleaning it; the answer waits for that (`HAnswer`).
HTakeUp(s) ==
    /\ pick = 0
    /\ rside[s] = "none"
    /\ toH[s] /= <<>>
    /\ Head(toH[s]) = <<"hello">>
    /\ LET old == cur IN
       /\ IF old = 0
          THEN (/\ rside' = [rside EXCEPT ![s] = "open"]
                /\ ended' = ended
                /\ toP' = toP
                /\ late' = late)
          ELSE (/\ rside' = [rside EXCEPT ![s] = "open", ![old] = "closed"]
                /\ ended' = [ended EXCEPT ![old] = TRUE]
                /\ toP' = [toP EXCEPT ![old] = <<>>]
                /\ late' = late \cup Reports(old))
       /\ toH' = [toH EXCEPT ![s] = Tail(@)]
       /\ pick' = s
       /\ cur' = 0
       /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                      pFailedFrom, svc, answered, broke, loaded, sentFail,
                      clean>>

\* The answer to the new hello: only once every earlier session's rows are
\* cleaned, so no two sessions' rows are ever loaded together.
HAnswer(s) ==
    /\ pick = s
    /\ (Mutation = "acceptBeforeClean"
        \/ \A p \in Sessions : p < s => Swept(p))
    /\ pick' = 0
    /\ cur' = s
    /\ answered' = [answered EXCEPT ![s] = TRUE]
    /\ toP' = [toP EXCEPT ![s] = Append(@, <<"greeted">>)]
    /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                   pFailedFrom, svc, late, rside, ended, broke, loaded,
                   sentFail, clean, toH>>

\* The session ended (a break the host saw, or closed by the takeover).
HEndSession(s) ==
    /\ broke[s]
    /\ (rside[s] = "open" \/ pick = s)
    /\ rside' = [rside EXCEPT ![s] = "closed"]
    /\ ended' = [ended EXCEPT ![s] = TRUE]
    /\ pick' = IF pick = s THEN 0 ELSE pick
    /\ cur' = IF cur = s THEN 0 ELSE cur
    /\ late' = late \cup Reports(s)
    /\ toP' = [toP EXCEPT ![s] = <<>>]
    /\ toH' = [toH EXCEPT ![s] = <<>>]
    /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                   pFailedFrom, svc, answered, broke, loaded, sentFail,
                   clean>>

\* `rows.load`: the row is keyed by its instance (`KeyPerLoad = FALSE`) or by
\* the load (`TRUE`); a second load of the same key in one session is refused
\* (not modelled: no correct peer sends one).
HReadLoad(r, s) ==
    /\ CanRead(s)
    /\ toH[s] /= <<>>
    /\ LET f == Head(toH[s]) IN
       /\ f[1] = "load" /\ f[2] = r
       /\ LET g == f[3]
              fresh == loaded[s][r] = NoGen
          IN /\ toH' = [toH EXCEPT ![s] = Tail(@)]
             /\ loaded' = IF fresh
                          THEN [loaded EXCEPT ![s][r] = g] ELSE loaded
             /\ svc' = IF fresh
                       THEN [svc EXCEPT ![r] = <<s, g>>] ELSE svc
             /\ toP' = IF fresh
                       THEN [toP EXCEPT ![s] = Append(@, <<"loaded", r, g>>)]
                       ELSE toP
             /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                            pFailedFrom, late, cur, pick, rside, answered,
                            ended, broke, sentFail, clean>>

\* `rows.unload`: idempotent. A row still loaded is disposed once and its
\* service withdrawn; one already gone is only answered.
HReadUnload(r, s) ==
    /\ CanRead(s)
    /\ toH[s] /= <<>>
    /\ LET f == Head(toH[s]) IN
       /\ f[1] = "unload" /\ f[2] = r
       /\ LET g == loaded[s][r]
              there == g # NoGen
          IN /\ toH' = [toH EXCEPT ![s] = Tail(@)]
             /\ toP' = [toP EXCEPT
                          ![s] = Append(@, <<"unloaded", r, g>>)]
             /\ IF there
                THEN (/\ loaded' = [loaded EXCEPT ![s][r] = NoGen]
                      /\ svc' = IF Mutation = "serviceResidue"
                                THEN svc
                                ELSE [svc EXCEPT ![r] = NoSvc]
                      /\ clean' = [clean EXCEPT ![<<s, r, g>>] = @ + 1])
                ELSE (/\ loaded' = loaded
                      /\ svc' = svc
                      /\ clean' = clean)
             /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                            pFailedFrom, late, cur, pick, rside, answered,
                            ended, broke, sentFail>>

\* The row ends on its own there (a plugin disposing its own fiber): gone,
\* and its ending is sent while the session is up.
HEndSelf(r, s) ==
    /\ SelfEnds
    /\ rside[s] = "open"
    /\ answered[s]
    /\ loaded[s][r] # NoGen
    /\ LET g == loaded[s][r] IN
       /\ loaded' = [loaded EXCEPT ![s][r] = NoGen]
       /\ svc' = IF Mutation = "serviceResidue"
                 THEN svc ELSE [svc EXCEPT ![r] = NoSvc]
       /\ clean' = [clean EXCEPT ![<<s, r, g>>] = @ + 1]
       /\ toP' = [toP EXCEPT ![s] = Append(@, <<"ended", r, g>>)]
       /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                      pFailedFrom, late, cur, pick, rside, answered, ended,
                      broke, sentFail, toH>>

\* The row's code left an unhandled error: it stays loaded (the peer
\* decides), and the failure goes out once per generation.
HFail(r, s) ==
    /\ Failures
    /\ rside[s] = "open"
    /\ answered[s]
    /\ loaded[s][r] # NoGen
    /\ <<s, r, loaded[s][r]>> \notin sentFail
    /\ sentFail' = sentFail \cup {<<s, r, loaded[s][r]>>}
    /\ toP' = [toP EXCEPT ![s] = Append(@, <<"failed", r, loaded[s][r]>>)]
    /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                   pFailedFrom, svc, late, cur, pick, rside, answered,
                   ended, broke, loaded, clean, toH>>

\* The session ended: its rows are disposed, once each, and their services
\* withdrawn.
HSweep(r, s) ==
    /\ Mutation /= "keepRows"
    /\ ended[s]
    /\ loaded[s][r] # NoGen
    /\ LET g == loaded[s][r] IN
       /\ loaded' = [loaded EXCEPT ![s][r] = NoGen]
       /\ svc' = IF Mutation = "serviceResidue"
                 THEN svc ELSE [svc EXCEPT ![r] = NoSvc]
       /\ clean' = [clean EXCEPT ![<<s, r, g>>] = @ + 1]
       /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                      pFailedFrom, late, cur, pick, rside, answered, ended,
                      broke, sentFail, toH, toP>>

\* The environment breaks a session that exists.
Break(s) ==
    /\ "lose" \in Faults
    /\ ~broke[s]
    /\ s <= pdial
    /\ rside[s] /= "closed"
    /\ broke' = [broke EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<pst, pconn, pdial, prow, pkey, pgen, pEndedFrom,
                   pFailedFrom, svc, late, cur, pick, rside, answered,
                   ended, loaded, sentFail, clean, toH, toP>>

(***************************************************************************)
(* The specification.                                                      *)
(***************************************************************************)
Next ==
    \/ PDial
    \/ PGreeted
    \/ \E r \in Rows :
         PLoad(r) \/ PUnload(r) \/ PLoaded(r) \/ PUnloaded(r)
         \/ PEnded(r) \/ PFailed(r)
    \/ \E r \in Rows, g \in Gens : PLateEnded(r, g) \/ PLateFailed(r, g)
    \/ \E s \in Sessions :
         PLoss(s) \/ HTakeUp(s) \/ HAnswer(s) \/ HEndSession(s) \/ Break(s)
    \/ \E r \in Rows, s \in Sessions :
         HReadLoad(r, s) \/ HReadUnload(r, s) \/ HEndSelf(r, s)
         \/ HFail(r, s) \/ HSweep(r, s)

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)

\* A row's instance runs in one session at a time: the new session's load
\* is taken only after the old session's rows are gone.
OneInstanceAtATime ==
    \A r \in Rows :
        Cardinality({s \in Sessions : loaded[s][r] # NoGen}) <= 1

\* The peer ends or restarts a row only for the generation the row is on:
\* a late ending or failure of an earlier generation changes nothing. With
\* a key per load the key alone says which generation; with a stable key the
\* notice must carry it (the `lateEnding` mutation matches the key only).
OwnGenerationOnly ==
    \A r \in Rows :
        /\ (pEndedFrom[r] = NoGen \/ pEndedFrom[r] = pkey[r])
        /\ (pFailedFrom[r] = NoGen \/ pFailedFrom[r] = pkey[r])

\* A row's service is imported only while the generation providing it is
\* loaded: the import does not outlive its row.
ServicesFollowRows ==
    \A r \in Rows :
        svc[r] = NoSvc
        \/ (\E s \in Sessions : svc[r] = <<s, loaded[s][r]>> /\ loaded[s][r] # NoGen)

\* A row's generation is disposed once: the unload, the row's own end and
\* the session's sweep do not dispose the same generation twice.
CleanupOnce ==
    \A k \in Keys : clean[k] <= 1

\* A session that ended ends with its rows cleaned.
EndedRowsCleaned ==
    \A s \in Sessions : (ended[s] ~> Swept(s))
===============================================================================
