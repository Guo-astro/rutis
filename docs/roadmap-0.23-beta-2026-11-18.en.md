# 0.23 beta

Status: beta definition, 2026-11-18. Tracking: [#694](https://github.com/arcships/rutis/issues/694) (release planning), [#183](https://github.com/arcships/rutis/issues/183) (quality control).

0.23 is the **beta** before 1.0: it closes the residual problems 0.22 exposed, makes one session-protocol version upgrade, and freezes the remaining public contracts into a shape that can stay compatible for a long time.

## What beta means

- **Frozen contracts**: plugin API 3 and `rutis.json` `format` 1; no Rust public API is removed or renamed. These are the parts plugin authors see directly.
- **Session protocol 4 to 5**: 0.23 allows one protocol version upgrade to fix replies after cancel and the async wait graph; old and new runtimes cannot mix, and one deployment upgrades together. It gets a bilingual migration guide.
- **Allowed**: defect, race and ordering fixes; platform fixes; new APIs, optional fields and optional capabilities; protocol-5 semantic changes; performance, tests, formal evidence, documentation and CI.
- **Not in 0.23**: removing or renaming a plugin API, changing `format` 1's default semantics, requiring plugin authors to rewrite code, or a second round of breaking changes beyond protocol 5.

## Scope

The `0.23 beta` milestone contains every open issue:

| Area | Issues | What to do |
| --- | --- | --- |
| Defects and ordering | #1088, #1133, #1295, #1103, #1080 | Swift/Kotlin cleanup emits, the `rows.ended` withdrawal window, Windows 4002, host single-worker policy B, nightly failures |
| Protocol decisions | #1099, #1098, #1040 | Replies after cancel, fire-and-forget vs await, the service-name boundary of a runtime row; protocol 5 and its migration guide |
| Evidence close-out | #1270, #1271, #1272, #1273, #1274 | Test-only rules, Kani prerequisites, the two Swift columns, Swift strict concurrency, Kotlin `RutisApp` Fray |
| Performance and loader | #1125, #1126, #1076, #1077 | Event hot path, `instance_rows` scan, spawned-instance row views |
| CI and engineering | #577, #183 | PR time target and platform cost, quality-control tracking |
| Types and roadmap | #1289, #694, #663 | Recursive data type aliases, version planning, developer experience |

## Release criteria

0.23.0 ships only when:

- the 0.23 milestone has no P0/P1 defect;
- the `release.yml` dry run is green (five platforms' package/install, Windows dylib, Swift cases);
- one full nightly run is green;
- the two-machine network smoke passes;
- the release notes and the bilingual migration guide are ready;
- every explicitly-not-guaranteed item that remains open is listed in the release notes.

## Explicitly outside 0.23

- 1.0's long-term compatibility promise and API-stabilization process;
- a new round of breaking changes beyond protocol 5;
- a redesign that requires plugin authors to rewrite code.
