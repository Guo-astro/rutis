--------------------------- MODULE PeerHosted ----------------------------
(***************************************************************************)
(* A host's side of the row contract: `plugins.load`, `plugins.update` and  *)
(* `plugins.unload` of the rows a far end asks for, a load the far end      *)
(* cancels while it is in flight, and an unload that arrives before its     *)
(* load. The question is the doubt moved from #625: does a cancelled or     *)
(* overtaken load leave a plugin running here that nobody will unload?      *)
(*                                                                         *)
(* Design: docs/design-remote-plugins-2026-10-03.md §5.1 (the host loads    *)
(* the far end's rows below its own session) and §5.2 (`plugins.load`,      *)
(* `plugins.update`, `plugins.unload`). Implementation:                     *)
(* crates/rutis-bridge/src/session/plugin/rows.rs (`Server.load` inserts    *)
(* the key before it awaits the fiber, removes and disposes it when the     *)
(* apply failed; `unload` removes and disposes, and a key that is not       *)
(* there gets a tombstone, so a load that follows is refused, #1101), and   *)
(* crates/rutis-bridge/src/session/plugin/run.rs (`RowHandle::drop` aborts  *)
(* the load and disposes the row's fiber once it is mounted).               *)
(*                                                                         *)
(* Simplified (README.md): one key and a catalog of one; describe is one  *)
(* step, a load is two (accepted, then applied), an unload is one; the     *)
(* session is open or closed; the far end either unloads what it cancels   *)
(* or (the mutation) does not.                                             *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Keys,       \* the keys the far end loads here ("a")
    Catalog,    \* the plugins this host can load (a subset of Keys)
    MaxLoads,   \* loads of one key in this run
    MaxUpdates, \* updates this run
    Mutation    \* "none", or a deliberate bug (README, "Mutations")

ASSUME Keys /= {}
ASSUME Catalog \subseteq Keys
ASSUME MaxLoads \in Nat
ASSUME MaxUpdates \in Nat
ASSUME Mutation \in {"none", "cancelNoUnload", "noTombstone", "updateLoading",
                     "wrongDescribe", "loadMissing"}

VARIABLES
    session,    \* the session: "open" | "closed"
    row,        \* per key at the far end: "idle" | "loading" | "loaded" | "cancelled"
    loading,    \* keys this host accepted and has not applied yet
    loaded,     \* keys this host applied and has not disposed
    tomb,       \* keys unloaded while loading: apply is followed by dispose
    orphan,     \* ghost: keys loaded for a far end that no longer wants them
    schema,     \* per key: "unknown" | "found" | "missing"
    described,  \* keys the far end asked about
    loads,      \* per key: how many loads it has had
    updates,    \* updates this host accepted
    badUpdate,  \* ghost: an update of a key that was not loaded
    badLoad     \* ghost: a load without a schema that says it is here

vars == <<session, row, loading, loaded, tomb, orphan, schema, described,
          loads, updates, badUpdate, badLoad>>

Init ==
    /\ session = "open"
    /\ row = [k \in Keys |-> "idle"]
    /\ loading = {}
    /\ loaded = {}
    /\ tomb = {}
    /\ orphan = {}
    /\ schema = [k \in Keys |-> "unknown"]
    /\ described = {}
    /\ loads = [k \in Keys |-> 0]
    /\ updates = 0
    /\ badUpdate = FALSE
    /\ badLoad = FALSE

(***************************************************************************)
(* The far end: loads its row, cancels it while loading, unloads it.       *)
(***************************************************************************)

\* `plugins.describe` / `rows.schema`: what this host has. A plugin it
\* does not contain is Missing (NotFound).
Describe(k) ==
    /\ session = "open"
    /\ schema' = [schema EXCEPT ![k] =
                     IF Mutation = "wrongDescribe" \/ k \in Catalog
                     THEN "found" ELSE "missing"]
    /\ described' = described \cup {k}
    /\ UNCHANGED <<session, row, loading, loaded, tomb, orphan, loads,
                   updates, badUpdate, badLoad>>

\* `plugins.load`: the row starts loading here. A key unloaded before it
\* was loaded is refused (its tombstone is remembered); the `noTombstone`
\* mutation forgets the tombstone and loads it anyway (#625). A plugin the
\* far end did not describe as present is not loaded (`loadMissing` does).
Load(k) ==
    /\ session = "open"
    /\ row[k] = "idle"
    /\ loads[k] < MaxLoads
    /\ (Mutation = "noTombstone" \/ k \notin tomb)
    /\ (Mutation = "loadMissing" \/ schema[k] = "found")
    /\ row' = [row EXCEPT ![k] = "loading"]
    /\ loading' = loading \cup {k}
    /\ loads' = [loads EXCEPT ![k] = @ + 1]
    /\ badLoad' = (badLoad \/ schema[k] # "found")
    /\ UNCHANGED <<session, loaded, tomb, orphan, schema, described, updates,
                   badUpdate>>

\* The apply finished. A key with a tombstone (unloaded while loading) is
\* disposed at once; one whose far end cancelled it without an unload (the
\* `cancelNoUnload` mutation) stays loaded here and is the orphan.
FinishLoad(k) ==
    /\ session = "open"
    /\ k \in loading
    /\ loading' = loading \ {k}
    /\ IF k \in tomb
       THEN /\ loaded' = loaded
            /\ orphan' = orphan \ {k}
            /\ row' = IF row[k] = "loading"
                      THEN [row EXCEPT ![k] = "idle"] ELSE row
       ELSE IF row[k] # "loading"
            THEN /\ loaded' = loaded \cup {k}
                 /\ orphan' = orphan \cup {k}
                 /\ row' = row
            ELSE /\ loaded' = loaded \cup {k}
                 /\ orphan' = orphan
                 /\ row' = [row EXCEPT ![k] = "loaded"]
    /\ UNCHANGED <<session, tomb, schema, described, loads, updates,
                   badUpdate, badLoad>>

\* The far end drops the row while it loads. It sends `rows.unload` (which
\* either finds the key loaded, or arrives while the apply is still out and
\* becomes a tombstone); `cancelNoUnload` is the doubt: no disposer was
\* registered, so nothing is sent.
CancelLoad(k) ==
    /\ session = "open"
    /\ row[k] = "loading"
    /\ row' = [row EXCEPT ![k] = "cancelled"]
    /\ IF Mutation = "cancelNoUnload"
       THEN UNCHANGED <<tomb, loaded, orphan>>
       ELSE IF k \in loaded
            THEN /\ loaded' = loaded \ {k}
                 /\ orphan' = orphan \ {k}
                 /\ tomb' = tomb
            ELSE /\ tomb' = tomb \cup {k}
                 /\ UNCHANGED <<loaded, orphan>>
    /\ UNCHANGED <<session, loading, schema, described, loads, updates,
                   badUpdate, badLoad>>

\* `plugins.unload`: a loaded row is disposed; one still loading gets a
\* tombstone. An unload before its load (the `noTombstone` mutation) is
\* forgotten, so the later load inserts the row: the other half of #625.
Unload(k) ==
    /\ session = "open"
    /\ row[k] \in {"loading", "loaded"}
    /\ row' = [row EXCEPT ![k] = "idle"]
    /\ IF k \in loaded
       THEN /\ loaded' = loaded \ {k}
            /\ orphan' = orphan \ {k}
            /\ tomb' = tomb
       ELSE IF k \in loading
            THEN IF Mutation = "noTombstone"
                 THEN UNCHANGED <<tomb, loaded, orphan>>
                 ELSE /\ tomb' = tomb \cup {k}
                      /\ UNCHANGED <<loaded, orphan>>
            ELSE UNCHANGED <<tomb, loaded, orphan>>
    /\ UNCHANGED <<session, loading, schema, described, loads, updates,
                   badUpdate, badLoad>>

(***************************************************************************)
(* `plugins.update`: only a loaded row has a config to update.             *)
(***************************************************************************)

Update(k) ==
    /\ session = "open"
    /\ row[k] = "loaded"
    /\ k \in loaded
    /\ updates < MaxUpdates
    /\ updates' = updates + 1
    /\ UNCHANGED <<session, row, loading, loaded, tomb, orphan, schema,
                   described, loads, badUpdate, badLoad>>

\* The `updateLoading` mutation: an update reaches a row whose apply is
\* still out, so the config it updates is the one being built.
UpdateEarly(k) ==
    /\ Mutation = "updateLoading"
    /\ session = "open"
    /\ row[k] = "loading"
    /\ updates < MaxUpdates
    /\ updates' = updates + 1
    /\ badUpdate' = TRUE
    /\ UNCHANGED <<session, row, loading, loaded, tomb, orphan, schema,
                   described, loads, badLoad>>

(***************************************************************************)
(* The session ends: every row here is disposed, once, and no row of it is *)
(* left for the next session.                                              *)
(***************************************************************************)

Close ==
    /\ session = "open"
    /\ session' = "closed"
    /\ row' = [k \in Keys |-> "idle"]
    /\ loading' = {}
    /\ loaded' = {}
    /\ tomb' = {}
    /\ orphan' = {}
    /\ UNCHANGED <<schema, described, loads, updates, badUpdate, badLoad>>

Next ==
    \/ \E k \in Keys : Describe(k) \/ Load(k) \/ FinishLoad(k)
                       \/ CancelLoad(k) \/ Unload(k) \/ Update(k)
                       \/ UpdateEarly(k)
    \/ Close

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

(***************************************************************************)
(* Properties.                                                             *)
(***************************************************************************)

\* No plugin is left loaded here that the far end no longer wants: a load
\* cancelled in flight is disposed when it applies, and an unload that
\* overtook its load refuses it.
NoOrphan ==
    orphan = {}

\* An update reaches a row that is loaded, not one whose apply is still
\* out.
UpdatingLoadedOnly ==
    badUpdate = FALSE

\* A describe is exact: a plugin this host has is Found, one it does not
\* have is Missing.
DescribeExact ==
    \A k \in described : (schema[k] = "found") <=> (k \in Catalog)

\* A load follows a describe that found the plugin.
LoadOnlyDescribed ==
    badLoad = FALSE
===============================================================================
