# Fixtures

`patch-expected.json` is generated from `patch-cases.json` with the JavaScript
original, `applyEntryPatches` from `@deepseek-ai/cordis-plugin-include`
(installed by `npm --prefix crates/rutis-dsh/dsh ci`):

```js
import { applyEntryPatches } from '<rutis>/crates/rutis-dsh/dsh/node_modules/@deepseek-ai/cordis-plugin-include/lib/index.js'
// for each case:
const warnings = []
const rows = applyEntryPatches([], structuredClone(c.layers.flat()), (m) => warnings.push(m))
// { name: c.name, rows, warnings: warnings.length }
```

Regenerate it after adding a case; never edit it by hand.

`patch-random-expected.json` holds 120 seeded random layer stacks composed by
the same original — version 1.0.9, the one `crates/rutis-dsh/dsh/package-lock.json`
pins — for `tests/patch_random_parity.rs`: each case carries its layers, the rows
`applyEntryPatches` returned, and how many times it warned. `patch-random.mjs`
generates it, with its own deterministic seeds; its header has the command
(the package is not a dependency of this repository). Regenerate it with the
same package version; never edit it by hand.
