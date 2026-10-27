# Design document conventions

[中文](design-docs.md)

How rutis design documents (`docs/design-<topic>-<date>.md`) are written, and when one is done. It applies to those written or revised from now on; earlier undated design documents (such as the [design principles](design-philosophy.en.md)) are not rewritten to it. They follow the design principles §7, for the reason [#690](https://github.com/arcships/rutis/issues/690) gives: while developer experience has no standard and does not stop a release, effort drifts toward internal correctness, so every design document first says what developers see.

## File and heading

- The file is `design-<topic>-<YYYY-MM-DD>.md`, one in each language (`.md` and `.en.md`), saying the same; a change to one is made to the other.
- It opens with: the title; a link to the other language; `Status:` (draft / final, not implemented / implemented) and the date (a revision the same day says what changed); `Issue:` the issue it answers, and the issues implementing it.

## Sections it must have

Their names and order may suit the topic, but all of these are there:

1. **Where things stand / what to solve**: what developers run into now, best quoted from the evaluation records ([#663](https://github.com/arcships/rutis/issues/663)), their words or their output.
2. **Decisions**: what was chosen and why; a sentence on why each alternative was dropped. Defaults are decided, not left open.
3. **Rules**: the behaviour, precise enough that whoever implements it does not guess. A rule across languages says it is the same in each, or where it differs and why.
4. **What developers see** (required, see below).
5. **Compatibility and migration**: which code, output and protocol change, and what happens when an old end meets a new one; incompatible changes go in that version's migration guide (both languages).
6. **Implementation**: the issues it is split into, and what done means for each.

## "What developers see"

A design document without this section, or one that covers only the case where everything works, is not done. Reviews read it first.

- A design that touches no developer (CI, a change inside only) has this section too, in one line: developers see no change.
- **By role**, for the roles the design touches: plugin authors, those assembling plugins with `rutis-host`, host owners (applications embedding rutis, or products using `rutis-host` as their runtime), those deploying.
- **When it works**: the code and configuration the role writes and the output it sees, in full in one language (TypeScript, say), the other languages only where they differ.
- **When it fails**: each error the role meets, one line each: how it is caused, the output **word for word** (the line rutis prints, and the error code a host gets), which row it is recorded on, how to fix it. The output names only what developers wrote (row names, service and method names, files and lines, what to change), no words internal to rutis (the [diagnostics design](design-diagnostics-2026-10-26.en.md) §4.5).
- **Concepts to learn**: the concepts the design adds for the role, against the concept budget per layer kept in #663; going over budget says why, or the design changes.
- The implementing PRs' acceptance tests compare the output this section writes, word for word; when the output changes, this section changes first.

For reference: the [diagnostics design](design-diagnostics-2026-10-26.en.md) §8, the [config schema](design-config-schema-2026-10-27.en.md) §9, [object references](design-object-refs-2026-10-27.en.md) §8.

## Discussion and finalizing

- Start from the developer: what they see first, then how it works inside.
- Finalize in one pass: review comments improve the same document; no "open questions" section is left.
- When implementing shows the design must change, the design document changes first (a same-day revision noted, or a new date), then the code.
