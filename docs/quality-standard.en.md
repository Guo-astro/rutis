# rutis quality standard

[中文](quality-standard.md) · [Quality status](quality-status.en.md)

Status: draft. Scope: all code, packages, documentation and release artifacts in the rutis repository.
Companion: [Quality status](quality-status.en.md) (maps each clause of this standard to the current implementation, gaps and plans; updated with the code).

This standard defines rules and criteria only. It does not refer to specific files, tests, tool versions or issues. Implementations, components, languages and platforms change; the rules should not change with them. When a rule needs to change, follow §14.

Clause numbers (such as `Q4.2`) are for reference from the status document, design documents and reviews. Once published, a number is never reused.

---

## 1. Quality goal

**Q1.1** rutis is a framework: users attach plugins, runtimes, processes and machines to it and hand it their lifecycle, dependencies, cleanup and communication across boundaries. When one of its guarantees fails, users usually cannot detect it or work around it in their own code. rutis is therefore held to a higher quality bar than anything running on it.

**Q1.2** Concretely:

1. every guarantee rutis writes down can be verified, and has been;
2. guarantees hold in every environment declared as supported, not only on developers' machines;
3. under faults (process crashes, network interruptions, malformed input, missing resources), rutis's behavior is written down and predictable;
4. a broken guarantee is found before release;
5. the errors users see are enough for them to locate the problem themselves.

**Q1.3** Quality control must itself be sustainable: every verification must be able to name the kind of problem it prevents. A verification method that maps to no risk is not introduced; one that duplicates an existing method is merged into it.

---

## 2. Quality dimensions

The quality of rutis is defined along the following fourteen dimensions. For every change, each dimension is checked for impact (§9).

| No. | Dimension | Definition | Criterion |
| --- | --- | --- | --- |
| **Q2.1** | Functional correctness | Written behavior holds for all allowed inputs and execution orders, including concurrent interleavings | Every guarantee is verified; guarantees about concurrency are verified under different execution orders, not just by repetition |
| **Q2.2** | Fault behavior | When a part fails, rutis behaves by written rules: failures are explicit, bounded and recoverable | Every possible fault (§5.2) has written expected behavior, verified by injection |
| **Q2.3** | Fault isolation | A failure affects only its written scope (the same process, mount, link) and does not spread | After a failure, everything outside the scope keeps working, and this is demonstrated |
| **Q2.4** | Resource integrity | After ending, unloading, replacing or closing, nothing is left behind by rutis or anything it started: processes, connections, files, handles, threads, tasks, internal records | Residue is checked after every end; resource use does not grow under repetition |
| **Q2.5** | Data integrity | Data rutis persists or carries is not lost, corrupted or silently rewritten: configuration, persisted state, values crossing boundaries | Writes stay consistent on failure; concurrent writers do not overwrite each other; values are equivalent after a round trip across a boundary |
| **Q2.6** | Compatibility | Holds on every declared platform, language version and dependency version range; different implementations interoperate | Every cell of the support matrix is actually verified (§10) |
| **Q2.7** | Evolution stability | Changes to public APIs, wire protocols, plugin interfaces, configuration formats and persisted formats are intentional, visible and come with a migration path | Incompatible changes are detected and declared before release; behavior when versions meet is defined |
| **Q2.8** | Security | Only verified peers connect; credentials do not leak; untrusted input causes no crash, privilege escalation or resource exhaustion; memory-safety boundaries are clear | All requirements of §5.3 are met |
| **Q2.9** | Performance and scale | Within the declared scale, latency and resource use match what is declared and do not grow over time | Every performance claim is measured; long runs show no growth trend |
| **Q2.10** | Diagnosability | On error, users can tell from error messages, status output and diagnostic interfaces which part, what cause and what to do; diagnostics do not change system state | User-facing errors meet §5.6; diagnostic interfaces are verified |
| **Q2.11** | Developer experience | Plugin authors and embedders get through their first successful path starting from the documentation; local test tools agree with the real host | The first-use path is verified end to end as a black box; differences between test tools and real runtimes are bounded and written down |
| **Q2.12** | Installability and deployability | Artifacts of every distribution channel install, run and uninstall in a clean environment | Every channel on every supported platform is verified in a clean environment |
| **Q2.13** | Documentation consistency | Examples, parameters and described behavior in the documentation match reality | Every executable example has been executed; every guarantee maps to a verification |
| **Q2.14** | Release integrity | Released artifacts correspond to the source, have consistent versions and can be reproduced | Artifacts are built automatically from tagged source; packages in one release have consistent versions |

---

## 3. Levels of guarantees

rutis's behavior falls into four levels; the strength of verification depends on the level. Design documents must state the level of every behavior they define.

| No. | Level | Meaning | Verification required |
| --- | --- | --- | --- |
| **Q3.1** | Core guarantee | Users build their systems on it and cannot work around it when it fails. Includes: dependency gating (not started until ready, stopped when a dependency goes away, restarted when it returns); every cleanup runs exactly once; failures roll back and the old state keeps working; consumers depend on service names, not implementations; what cannot cross a boundary is reported explicitly, never silently degraded; on interruption calls fail explicitly, never report false success, never retry side effects automatically; nothing is left behind after an end | Risks always treated as the highest level (P0); verified on every supported platform and under fault injection; verified in at least two independent ways (§8.3) |
| **Q3.2** | Contract behavior | Specific behavior written in design or user documentation: ordering, parameters, defaults, error categories, timeouts | Automated verification; registered in the guarantee register (§9.4) |
| **Q3.3** | Implementation detail | Behavior written in no document | Not required to be verified; users must not rely on it; tests must not pin it (otherwise changing the implementation is misread as breakage) |
| **Q3.4** | Explicitly not guaranteed | Behavior the documentation says is not provided (for example some orderings across a boundary, some detections) | Reason written down; if someone could mistake it for a guarantee, a verification pins that it is indeed not provided, so it does not silently become an implicit guarantee |

**Q3.5** When a behavior is promoted from implementation detail to contract behavior or core guarantee, verification of the matching level is added at the same time. Demoting a behavior is an incompatible change (§9.3).

---

## 4. Risk assessment

**Q4.1** Every design, new component and significant change gets a risk assessment: in the usage scenarios it is part of, list what can go wrong along each quality dimension (§2).

**Q4.2** Impact:

| Impact | Criterion |
| --- | --- |
| High | Violates a core guarantee; causes data loss, service interruption or a security problem; users cannot work around it in their own code |
| Medium | Functionality is unavailable under some conditions, but users can notice and work around it; error messages are insufficient and make diagnosis hard |
| Low | Experience issue, correctness not affected |

**Q4.3** Likelihood:

| Likelihood | Criterion |
| --- | --- |
| High | Met in normal use; the code involved changes often; it has already happened |
| Medium | Met under specific conditions: a platform, a kind of fault, a concurrent timing, a version combination |
| Low | Needs an uncommon combination of conditions |

**Q4.4** Priority and verification strength:

| Priority | Rule | Required verification |
| --- | --- | --- |
| P0 | High impact with medium or high likelihood; or involves a core guarantee | Automated; runs before merging when the change affects it, and in every daily and pre-release full run (Q12.3); covers every declared platform |
| P1 | High impact with low likelihood; or medium impact with medium or high likelihood | Automated; runs at least daily and before release |
| P2 | Everything else | May rely on review, documentation or manual verification only |

**Q4.5** The outcome of risk assessment (risks, levels, chosen controls) is recorded in the status document and updated as designs change.

---

## 5. Cross-cutting requirements

These do not depend on the type of component; they apply everywhere.

### 5.1 Verifiability of guarantees

**Q5.1.1** A guarantee that cannot be written as a verification cannot be published as a guarantee: either rephrase it in a verifiable form or demote it to "explicitly not guaranteed".

**Q5.1.2** Verification must check results, not only execute code. "The code ran" is not "the behavior was verified".

### 5.2 Faults

**Q5.2.1** For every part of rutis the following faults are normal, not exceptional. Every part's design must state its behavior under them, and that behavior is verified by injection:

| Category | Faults |
| --- | --- |
| Processes | The far process exits at any moment (normally, by crashing, forcibly); a process stops responding; a process fails to start; a process exits before its handshake |
| Communication | The connection drops at any moment; half-open (one end thinks it is connected, the other is gone); delay; the far end sends malformed, oversized, incomplete or out-of-order messages |
| User code | A plugin errors, panics, blocks or never returns during start, run or cleanup |
| Configuration | Configuration is invalid, missing, refers to something that does not exist, or is broken while running |
| Environment | A language environment, package, file or port it depends on is missing or unusable; insufficient permissions |
| The host itself | The host receives a termination signal; the host is forcibly terminated |
| Concurrency | Operations on the same object interleave in any order; many concurrent starts, calls and closes at the same moment |
| Versions | The two ends run different versions; protocol or interface versions do not match |

**Q5.2.2** Criteria under faults:

1. failures are explicit: the caller gets a definite failure with a distinguishable error category within bounded time;
2. no false success is reported; when the outcome is unknown, it is not reported as "not executed";
3. operations with side effects are not retried automatically;
4. the scope of failure matches the written isolation boundary (Q2.3);
5. nothing is left behind after the failure (Q2.4);
6. where recovery is possible, the recovery path is verified.

**Q5.2.3** Where no timeout, retry or automatic restart is provided, this is written down together with what users should do instead.

### 5.3 Security

**Q5.3.1** Trust boundaries are written down: what is trusted (for example plugin code) and what is not fully trusted (the network, remote machines, external input).

**Q5.3.2** Connections crossing a trust boundary are authenticated; communication across machines is encrypted; unencrypted communication is allowed only on the local machine.

**Q5.3.3** Credentials must not appear in logs, error messages, diagnostic output, command-line arguments, URLs or test artifacts. Every feature that handles credentials verifies this.

**Q5.3.4** Input from outside a trust boundary (network messages, frames from a peer, configuration files, plugin manifests, protocol handshakes):

1. has size and depth limits, and exceeding them is an explicit rejection;
2. malformed input causes no crash, unbounded memory or unbounded time;
3. rejections carry distinguishable categories and leak no internal information.

**Q5.3.5** Authorization is based on verified identity, never on what a peer claims about itself. After an identity or authorization is revoked, established connections and connections being established both lose it, and late results cannot bypass the revocation.

**Q5.3.6** Use of unsafe code (memory-unsafe language features, foreign function calls, dynamically loaded native code) is concentrated and written down; each place states its safety preconditions. Before native code is loaded, its identity is checked; if the check fails, no part of the loaded code runs.

**Q5.3.7** Third-party dependencies are checked for known vulnerabilities and licenses; adding a dependency requires a stated reason.

**Q5.3.8** Running out of process is not isolation. A feature that needs to isolate untrusted code needs its own design; "it runs in another process" is not a substitute.

### 5.4 Resource integrity

**Q5.4.1** Every part that holds resources (processes, connections, files, listeners, threads, tasks, internal registrations) states when and by whom each resource is released.

**Q5.4.2** At the end of every verifiable scenario, residue is checked: every started process has exited; every listener is released; every temporary file is removed; the test process's own handles, threads and tasks are back at baseline; internal registries are back at baseline.

**Q5.4.3** Parts that support repeated creation and destruction are verified not to grow in resource use after many repetitions.

**Q5.4.4** When the host is forcibly terminated, the processes it started exit within bounded time and no orphan is left. This is verified on every supported platform, since the mechanisms differ.

### 5.5 Data integrity

**Q5.5.1** Persistent writes are atomic: on failure, storage holds either the old content or the new, never something in between.

**Q5.5.2** Where several writers can write the same storage, conflicts are detected; no one's change is silently overwritten.

**Q5.5.3** Every persisted format is versioned. A new version reads data written by every earlier version still supported, or refuses it explicitly and states how to migrate.

**Q5.5.4** For values crossing languages and processes, the conversion rules are written down and verified for every pair of implementations: values are equivalent after a round trip; values that cannot be represented are rejected explicitly, not silently changed.

### 5.6 Errors and diagnostics

**Q5.6.1** A user-facing error contains: where it happened (which plugin, which configuration row, which file, which peer); the cause; a feasible fix (such as the package to install or the field to change).

**Q5.6.2** Error categories are for programs; error text is for people. Programs (including rutis itself and its tests) never branch on error text.

**Q5.6.3** The output of common errors is pinned; changes to it are called out in review.

**Q5.6.4** Diagnostic interfaces (status queries, self-description, tracing) do not change system state and do not call user code; their consistency (whether they are an atomic snapshot) is written down.

**Q5.6.5** Traces and logs do not record the content of messages crossing boundaries, only direction, length and category. Recording content requires explicit opt-in and a note that it may include sensitive data.

### 5.7 Performance

**Q5.7.1** A performance figure in the documentation is marked as either a "guarantee" or a "reference value". Guarantees have automated verification in a fixed measurement environment; reference values state their measurement conditions.

**Q5.7.2** The designed scale (numbers of plugins, runtimes, nodes, instances, continuous running time) is declared. Within it, resource use shows no growth trend over long runs.

**Q5.7.3** Detection of performance changes is not made a mandatory gate in an environment whose noise exceeds the detection threshold.

---

## 6. Verification requirements by component type

Components of rutis fall into the types below. A component may belong to several types and then meets the requirements of each. When adding a component, first decide which types it belongs to.

### Q6.1 Lifecycle and concurrency core

E.g.: the plugin state machine, dependency gating, the service registry, event dispatch.

1. Every state transition rule is verified, including every kind of request arriving in every state;
2. guarantees involving concurrency hold under both single-threaded and multi-threaded execution;
3. parts that have had problems before, or that are sensitive to execution order, are verified with methods that control execution order (systematically trying different interleavings and reporting the exact order on failure), not only by repetition;
4. no exception from a user callback (error, panic, never returning) can corrupt the framework's own state;
5. after repeated creation and destruction, internal records return to baseline;
6. if there is a reference implementation (the origin of the behavior), its language-independent behavior is checked item by item, and the reason for each difference is recorded.

### Q6.2 Cross-process protocols

E.g.: the session protocol, runtime control operations, node operations.

1. The message set, the fields of each message, error categories and the version number are written as a specification;
2. every implementation (every language) passes the same conformance checks;
3. malformed, unknown, duplicate, out-of-order messages and messages referring to nonexistent things each have written handling, which is verified;
4. on a version or capability mismatch, the handshake refuses explicitly and says which end needs upgrading;
5. the communication faults listed in Q5.2 are verified by injection and meet Q5.2.2;
6. resources across the boundary (references, handles) follow counting or lease rules, verified not to leak or be released early under any sequence of acquisition and release;
7. cancellation reaches the far end, hop by hop when forwarded;
8. a new protocol design, or a change to the rules of multi-party interaction, is model-checked for deadlock and key invariants before implementation (§11.2);
9. changes to the wire format follow §9.3.

### Q6.3 Channels and transports

E.g.: local sockets, inherited descriptors, WebSocket, in-memory channels, and transports added later.

1. Every transport passes the channel contract: order, no loss or duplication, message boundaries, backpressure, idempotent close that wakes blocked parties, an observable close by the far end;
2. every transport has a message size limit and closes explicitly when it is exceeded; the boundary values (exactly at the limit, one byte over) are verified;
3. transports that must detect lost peers are verified to detect half-open connections within the declared time;
4. authentication and rejection categories are the same across implementations;
5. whether a transport is available on a platform is decided by the support matrix (§10).

### Q6.4 Parsing external input

E.g.: protocol frame decoding, framing, handshake requests, configuration files, plugin manifests, configuration merge rules.

1. Boundary values are verified: empty, minimum, maximum, over maximum, deepest nesting;
2. fuzzing: no crash, unbounded memory or unbounded time under random and mutated input; the corpus is kept in the repository and problems found become fixed tests;
3. where the result has checkable properties (decoding after encoding gives the original; agreement with a reference implementation), they are verified with random input;
4. error messages point at the location in the input.

### Q6.5 Resource holders

E.g.: parts that start processes, hold connections or listeners, create temporary files.

1. Meet all of Q5.4;
2. no race when many are created concurrently in one process (for example a descriptor wrongly inherited by another child process);
3. release does not depend on users calling something: if a user forgets or exits abnormally, there is a written path for reclaiming the resource.

### Q6.6 User-facing entry points

E.g.: command-line tools, configuration file formats, project templates.

1. Verified as a black box: the released form of the program, invoked as users invoke it, without internal interfaces;
2. every command's success path, common error paths and exit codes are verified; exit code rules are in the user documentation;
3. handling of termination signals is verified on every platform and meets Q5.4.4;
4. every field of a configuration file is verified; how unknown fields are treated is written down;
5. a project generated from a template installs its dependencies, passes its own tests and runs in a clean environment;
6. output meets §5.6; output meant for scripts is an interface, and changes follow §9.3.

### Q6.7 Plugin SDKs and test tools

E.g.: the plugin SDK of each language; test tools that need no host.

1. The SDKs of different languages have the same plugin interface semantics: declarations, lifecycle, value passing, sync and async, cancellation, version marking; differences are written down;
2. test tools agree with real runtimes: the same cases run on the test tool and on the real runtime and results are compared; known differences are written down and bounded;
3. test tools surface early, on the developer's machine, code that only works in process and breaks across processes;
4. on a plugin interface version mismatch, the error says which side needs upgrading.

### Q6.8 Language runtimes

E.g.: the per-language runtime processes. Before support for a new language or runtime (including another execution engine for an existing language) is declared, all of the following hold:

1. it passes every item of the session and runtime conformance checks;
2. on every declared platform and every local handover method;
3. whether it is reentrant is written down, with its rules for synchronous calls to and from other runtimes; under the chosen cross-runtime synchronous call policy it does not deadlock (as established by model checking);
4. a crash affects only the written scope (the same runtime instance), verified;
5. it has a plugin SDK and test tool meeting Q6.7;
6. it has a declared minimum version and is in the support matrix (§10);
7. the runtime cannot download or execute undeclared code without the host knowing.

### Q6.9 Service contracts across implementations

E.g.: one service name provided by implementations in different languages or places.

1. A service's method shapes (sync or async) can be declared;
2. values passed across languages are converted by one written set of rules (plugin API, "Passing values"): every runtime and every SDK's test tool passes values by those rules, and this is verified; an implementation in any language can name the errors it throws;
3. when an implementation is replaced, consumers need no change and the host needs no restart; calls during the replacement either succeed or fail explicitly, never hang; the old implementation leaves nothing behind;
4. whether different implementations of one service return the same data (fields, missing versus `null`, error names, meaning) is the responsibility of the service's authors and outside rutis: rutis does not define a service's data shapes and provides no contract format or checking tool.

### Q6.10 Persistence

E.g.: the editable layer of layered configuration, configuration files.

1. Meet all of Q5.5;
2. every kind of edit gives the same state after save and reload;
3. failed saves, conflicts and partial writes are verified;
4. every release keeps a sample of its persisted data, and later versions must read it (Q5.5.3).

### Q6.11 Code generation

E.g.: Rust bindings generated from another language's type declarations.

1. Every type mapping rule is verified, including the difference between optional, nullable and both;
2. what cannot be generated is reported at build time with location and reason, not skipped silently;
3. generated code is compiled and called against real external packages;
4. when external packages or frameworks publish new versions, periodic verification finds incompatibilities (reported, not blocking merges).

### Q6.12 Dynamic loading of native code

1. Identity and compatibility are checked before loading; if anything does not match, loading is refused and no part of the refused code runs;
2. every kind of refusal has a counterexample verified;
3. when a replacement fails, the old version keeps working;
4. builds are reproducible: the same source built at different paths and on different machines gives the same artifact;
5. every supported platform is verified: before merging according to what the change affects, and in every daily and pre-release full run (Q12.3); not only when the relevant code changes.

### Q6.13 Distribution and network nodes

E.g.: connections between nodes, remote runtimes, leases and takeover, reconnection.

1. Meet Q6.2 and Q6.3;
2. verified between two independent host processes, not only simulated in one process;
3. timing rules for reconnection, takeover and revocation are model-checked at design time (§11.2); at implementation time, tests cover late messages and concurrent takeover;
4. periodically verified under real network conditions (delay, loss, disconnection);
5. retry and backoff parameters are contract behavior and are verified exactly, not just "it retries".

### Q6.14 Compatibility layers for external frameworks

E.g.: mounting another framework's plugins; joining another framework's applications as nodes.

1. The external framework's native behavior is the reference, compared on the same scenarios;
2. behavior that cannot be kept across the boundary is written as boundary rules; detectable violations raise errors, verified;
3. real published plugins of that ecosystem are used as verification material;
4. new versions of the external framework are verified periodically (not blocking merges).

### Q6.15 Distribution artifacts

E.g.: packages on each channel, binaries, platform-specific packages.

1. For every channel and every supported platform, in a clean environment without the repository's source: install → run the minimal usage path → uninstall;
2. verify that what runs is the artifact of this build;
3. after release, verify once more with the artifacts from the registries;
4. packages of one release have consistent versions, checked automatically.

### Q6.16 Development and diagnostic channels

1. Present only in development builds or when explicitly enabled; absent from production builds, verified;
2. reachable only locally, with restricted access;
3. commands that change system state go through the same checks as the production path and leave state unchanged on failure.

---

## 7. Rules for tests themselves

**Q7.1 Determinism.** Test results do not depend on machine speed, load or test order:

1. no fixed-duration waits for synchronization; waiting for something uses an explicit synchronization point; asserting that something does not happen uses a controllable clock, or is rewritten as "another thing has happened while this one still has not";
2. timeouts only guard against hangs and are not performance assertions;
3. guarantees with time semantics (heartbeats, backoff, deadlines) are verified with a controllable clock.

**Q7.2 Reproducibility.** Tests that use randomness print their seed and can be replayed from it; on failure they keep what is needed to reproduce (seed, operation sequence, output of every process).

**Q7.3 No masking of failures.** Automation does not retry failed tests. Intermittent failures are defects: recorded at once and fixed, or shown to be a problem of the test, within a set time. A temporarily skipped test states its reason and the defect record.

**Q7.4 Failures must be visible.** Exceptions in background threads, child processes and asynchronous tasks fail the test; printing them is not enough.

**Q7.5 Isolation.** Tests do not share ports, files, environment variables or global state.

**Q7.6 Black box and white box are separate.** Tests of user entry points use no internal interfaces; tests of internal rules may. Neither replaces the other.

**Q7.7 References can be regenerated.** Outputs used as references (expected files, results of reference implementations) can be regenerated by a command in the repository, and the regenerated result is periodically checked against what is stored.

**Q7.8 Skips are explained.** When a combination (platform, implementation, version) does not run a verification, the reason is written down; silent skips are not allowed.

---

## 8. Independence and layers of verification

**Q8.1** Verification has three layers; none replaces another:

| Layer | Question it answers |
| --- | --- |
| Rules | Do the rules of a single component hold (unit, contract, conformance) |
| Composition | Do they hold when components are combined, in real processes (integration, black-box end to end, fault injection) |
| Environment | Do they hold in users' real environments (support matrix, clean installation, long runs, real networks) |

**Q8.2** Every core guarantee is verified at all three layers.

**Q8.3** Every core guarantee is verified in at least two independent ways, for example: Rust-driven conformance checks and each language SDK's own tests; library-level tests and black-box tests; tests and model checking. The purpose is to keep the verifying code and the implementation from making the same mistake.

---

## 9. Change process

### 9.1 Design

**Q9.1.1** Every design document contains:

1. the behavior it defines, with the level of each (§3);
2. a risk assessment (§4);
3. acceptance criteria, each writable as a verification;
4. the component types it belongs to (§6) and how their requirements are met.

**Q9.1.2** A protocol involving multi-party interaction is model-checked before its design is final (§11.2).

### 9.2 Implementation

**Q9.2.1** Behavior changes come with verification.

**Q9.2.2** A defect fix starts with a test that reproduces the defect; after the fix it passes; it is named after or refers to the defect record.

**Q9.2.3** When fixing a defect, also ask: why did existing verification not find it? If the answer is that a kind of verification is missing, add that kind, not only this one test.

**Q9.2.4** New code that parses external input also meets Q6.4.

**Q9.2.5** New unsafe code states its safety preconditions (Q5.3.6).

### 9.3 Incompatible changes

**Q9.3.1** All of the following are interfaces, and incompatible changes to them follow this section: public programming interfaces; wire protocols; plugin interfaces; configuration formats; persisted formats; command-line arguments and exit codes; output formats meant for scripts; demotion of guarantees.

**Q9.3.2** An incompatible change:

1. is detected automatically or explicitly flagged in review before merging;
2. matches the versioning rules;
3. comes with migration notes whose code is verified;
4. when different versions meet (for example a new host and an old runtime), they either work or refuse explicitly at connection time with a reason; the previous release meeting the current version is verified.

### 9.4 Guarantee register

**Q9.4.1** All core guarantees and contract behavior are recorded in a guarantee register: number, source, content, level, status (verified / partly verified / not verified / explicitly not guaranteed), verifications.

**Q9.4.2** Verification code is marked with the numbers of the guarantees it verifies.

**Q9.4.3** When a verification is removed or skipped, the status of its guarantee in the register is updated in the same change.

**Q9.4.4** At every release, unverified core guarantees number zero; unverified contract behaviors are no more than at the previous release.

---

## 10. Support matrix

**Q10.1** Dimensions of the support matrix: operating system and architecture; the version of each language environment; version ranges of optional dependencies; each runtime combined with each transport; protocol versions.

**Q10.2** Everything declared as supported is actually verified:

1. the **minimum** declared version and the **latest** version of each dimension;
2. the declared lower bound of a dependency is verified with that lower bound;
3. each combination of runtime × transport × platform is either verified or marked unsupported in the matrix with a reason.

**Q10.3** An environment no longer verified is removed from the support declaration. "Declared supported but never verified" is not allowed.

**Q10.4** A new platform, language version, runtime or transport enters the matrix and passes verification before support for it is declared.

---

## 11. Choosing verification methods

### 11.1 Methods by kind of problem

| Kind of problem | Method to use | Not a substitute for it |
| --- | --- | --- |
| Does a single rule hold | Unit or contract verification | — |
| Do several implementations agree | Conformance checks (one set of checks run by every implementation) | Separate tests written per implementation |
| Does it agree with reference behavior | Differential verification (same input, compare output) | Reading the reference's documentation only |
| Does it hold under concurrent interleavings | Model checking with controlled scheduling | Repetition |
| Is the protocol design itself correct | Design model checking | Tests of the implementation |
| Is it robust under malformed input | Fuzzing and boundary values | A few handwritten error cases |
| Logic with clear properties | Property checks on random input | A few fixed cases |
| Does it hold when combined | Black-box end to end | Library-level integration |
| Does it hold under faults | Fault injection | Happy-path tests |
| Is there a slow leak | Long runs and resource trends | A single residue check |
| Environment differences | Support matrix, clean installation | The developer's machine |
| Are error messages usable | Output snapshots plus human review | — |

### 11.2 When to use model checking

**Q11.2.1** Design model checking is used where all of the following hold:

1. several independent participants, with messages that may be delayed, reordered or lost;
2. the design is not final, or its correctness rests on written reasoning only;
3. errors appear only in particular interleavings and are hard to trigger reliably with tests.

**Q11.2.2** Single-process state machines and sequential logic do not use design model checking; their order-sensitive parts are verified on the implementation itself with controlled scheduling (Q6.1.3).

**Q11.2.3** A design model states which design it corresponds to, what it simplifies and which invariants it checks. When the design changes, the model changes with it.

### 11.3 When not to adopt a method

**Q11.3.1** A verification method is not introduced when:

1. it cannot name the kind of problem it prevents;
2. it checks the same kind of problem as an existing method without finding more;
3. it only produces reports that nobody acts on;
4. in the current environment, its false-alarm rate outweighs what it finds.

**Q11.3.2** Execution metrics such as code coverage are not allowed as mandatory gates: they show that code ran, not that behavior was checked (Q5.1.2). They may only be used to find areas with no verification at all.

---

## 12. Continuous integration

Continuous integration is where quality control is carried out: it runs most of the verification this standard requires. It is also a system to be designed and measured: too slow, and developers bypass it or wait for a long time; too thin, and problems slip into the main line.

**Q12.1** Verification runs at levels according to how soon a problem needs to be known:

| When | What runs |
| --- | --- |
| Before every merge | P0 verification affected by the change (Q12.3); static checks; the cheaper parts on each platform |
| Daily (when the main line has new merges), and before release | All verification: all platforms, every cell of the support matrix; clean installation; versions meeting across releases; reproducible builds |
| Daily | Longer P1 verification: long runs, fuzzing, large-scale model checking, real network conditions, new versions of external ecosystems |
| Before release | §13 |

**Q12.2** Verification before merging is kept within a reasonable time; when it grows beyond that, the expensive parts that find little are moved to a later level, not deleted.

**Q12.3 Selection by change.** Before merging, only the verification affected by the change runs; all verification runs **at least once a day** (when the main line has new merges) and **before every release**.

1. The mapping from what changed to what is verified lives in the CI configuration and is reviewed like code;
2. changes whose impact cannot be determined (build configuration, dependency lock files, the CI configuration itself, widely shared parts) run all verification;
3. when a verification skipped before merging fails in a daily or pre-release full run, after the fix the mapping is checked for this kind of change and corrected (Q9.2.3).

**Q12.4 Cancel superseded runs.** When a change gets a new commit, runs in progress for the older commit are cancelled at once. The daily full run checks every commit merged since the last successful run and is not cancelled; when it fails, an issue is opened naming the failed verification and the commit range (from the last successful run's commit to this run's).

**Q12.5 One merge condition.** The merge condition is a single aggregate check: it passes when every selected verification passes; verifications skipped because they were not selected do not count as failures; the aggregate check itself always runs and cannot be skipped.

**Q12.6 Time is measured as developers wait.** The time budget for verification before merging and before release counts from pushing a commit (or tag) to getting a result, including queueing, not only execution. The requirements below apply to verification before merging and before release only; the daily full run is not on a developer's waiting path.

1. Shorten the critical path (the longest item) first, then the rest; verification on the critical path may be split into parallel parts;
2. on scarce execution resources (platforms or machines of limited number), each run takes as few jobs as possible; checks that can be done one after another on the same machine are merged into one job;
3. checks that cannot use caches, such as build reproducibility, are kept off the critical path before merging unless the change directly affects them.

**Q12.7 No duplication.** A kind of check runs in only one place at a given level (Q11.3.1).

**Q12.8 Main line failures first.** A verification failure on the main line is a highest-priority defect: find the merge that introduced it, and do not merge changes it may affect until it is fixed.

**Q12.9 Measure continuous integration itself.** Regularly record: time from push to result (including queueing), what makes up the critical path, the time of each job, the intermittent failure rate, main line failures and how many of them were not selected before merging. When over budget, adjust per Q12.2 and Q12.6.

**Q12.10 CI configuration is code.** Changes to CI configuration are reviewed like code; a change to CI configuration runs all verification before merging.

---

## 13. Release gate

**Q13.1** A release requires all of the following:

1. all verification passes on the commit released (a daily or pre-release full run); the most recent daily runs all pass;
2. no new intermittent failures or skipped verifications;
3. all core guarantees verified; unverified contract behaviors no more than at the previous release (Q9.4.4);
4. incompatible changes detected, declared and accompanied by migration notes whose code is verified;
5. a sample of this release's persisted data is stored;
6. artifacts of every channel and platform verified in a clean environment;
7. verification across real machines done (may be manual; results recorded);
8. release notes list the changes to the guarantee register in this release.

**Q13.2** After release, verify once more with the artifacts from the registries; on failure, withdraw or mark that release and publish a fix.

---

## 14. Maintaining this standard

**Q14.1** Changes to this standard go through review; the change description states which kind of problem the new or changed clause prevents and how it affects existing verification.

**Q14.2** Clause numbers are never reused; a removed clause keeps its number, marked "withdrawn" with the reason.

**Q14.3** The status document describes, by clause number, how each clause is currently met; when a clause is added, a matching row is added there.

**Q14.4** Before every release, the clauses not yet met in the status document are reviewed: each has a plan or is explicitly accepted as a temporary exception, with a reason and a deadline.
