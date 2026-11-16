// Generates `patch-random-expected.json`: random patch layers composed by the
// JavaScript original, `applyEntryPatches` from
// `@deepseek-ai/cordis-plugin-include` 1.0.9 (the version
// `crates/rutis-dsh/dsh/package-lock.json` pins), for
// `tests/patch_random_parity.rs`.
//
// Node only, and the package is not a dependency of this repository:
//
//   mkdir -p /tmp/cordis && cd /tmp/cordis
//   npm install --legacy-peer-deps \
//     @deepseek-ai/cordis-plugin-include@1.0.9 \
//     @deepseek-ai/cordis-plugin-loader@1.0.5 @deepseek-ai/cordis@4.0.4
//   node crates/rutis-loader/tests/fixtures/patch-random.mjs \
//     /tmp/cordis/node_modules/@deepseek-ai/cordis-plugin-include/lib/index.js \
//     > crates/rutis-loader/tests/fixtures/patch-random-expected.json
//
// The output is deterministic: every case has its own seed, and the package
// version is fixed, so regenerating it gives the same file. Never edit it by
// hand.

import { pathToFileURL } from 'node:url'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'

const modulePath = process.argv[2]
if (!modulePath) {
  console.error('usage: node patch-random.mjs <cordis-plugin-include/lib/index.js>')
  process.exit(2)
}
// 1.0.9 keeps the semantics the Rust port matches; later versions changed
// `applyPatches` (parents, positions).
const manifest = JSON.parse(
  readFileSync(join(dirname(modulePath), '..', 'package.json'), 'utf8'),
)
if (manifest.version !== '1.0.9') {
  console.error(`expected @deepseek-ai/cordis-plugin-include 1.0.9, got ${manifest.version}`)
  process.exit(1)
}
const { applyEntryPatches } = await import(pathToFileURL(modulePath).href)

/// One case's stream of numbers, independent of every other case.
function mulberry32(seed) {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const int = (rng, n) => Math.floor(rng() * n)
const between = (rng, low, high) => low + int(rng, high - low + 1)
const pick = (rng, values) => values[int(rng, values.length)]
const chance = (rng, p) => rng() < p

/// JSON a patch may write anywhere: no objects with prototype keys, nothing
/// `structuredClone` cannot take.
function value(rng, depth) {
  const kinds =
    depth > 0
      ? ['null', 'bool', 'int', 'float', 'string', 'array', 'object']
      : ['null', 'bool', 'int', 'float', 'string']
  switch (pick(rng, kinds)) {
    case 'null':
      return null
    case 'bool':
      return chance(rng, 0.5)
    case 'int':
      return between(rng, -3, 9)
    case 'float':
      return pick(rng, [0.5, -1.25, 1000, 0])
    case 'string':
      return pick(rng, ['', 'x', 'y y', 'ü', '"q"', 'a/b'])
    case 'array': {
      const items = []
      for (let i = between(rng, 0, 3); i > 0; i--) items.push(value(rng, depth - 1))
      return items
    }
    default: {
      const object = {}
      for (let i = between(rng, 0, 3); i > 0; i--) {
        object[pick(rng, ['x', 'y', 'note', 'value', 'config'])] = value(rng, depth - 1)
      }
      return object
    }
  }
}

const NAMES = ['alpha', 'beta', 'gamma', 'plugin-a', '']

/// A row the way a patch inserts one: always an object, as the JavaScript
/// original walks every entry with `entry.id` and `entry.group`.
function row(rng, ids, depth, tag, fresh) {
  const entry = {}
  // Existing ids too: an insert may repeat an id the tree already has.
  const which = int(rng, 10)
  if (which < 7) entry.id = pick(rng, ids)
  else if (which < 9) {
    entry.id = `n${tag}_${fresh.count++}`
    ids.push(entry.id)
  }
  if (chance(rng, 0.85)) entry.name = pick(rng, NAMES)
  if (chance(rng, 0.15)) entry.disabled = chance(rng, 0.5)
  if (chance(rng, 0.1)) entry.inject = [pick(rng, ['a', 'b'])]
  if (chance(rng, 0.1)) entry[`extra${int(rng, 3)}`] = value(rng, 1)
  if (chance(rng, 0.3)) {
    // A group: its `config` array's rows are walked with it.
    entry.group = pick(rng, [true, true, 'yes', 1])
    if (chance(rng, 0.8)) {
      entry.config = []
      for (let i = between(rng, 0, 2); i > 0; i--) {
        entry.config.push(row(rng, ids, depth - 1, tag, fresh))
      }
    } else {
      // Or not an array: an insert into it starts the rows over. Not an
      // array of other values either, which `buildMap` would walk into.
      entry.config = pick(rng, [null, 5, 'x', true, { x: 1 }])
    }
  } else if (depth > 0 && chance(rng, 0.35)) {
    // Rows of a non-group: neither indexed nor flattened.
    entry.config = []
    for (let i = between(rng, 0, 2); i > 0; i--) {
      entry.config.push(row(rng, ids, depth - 1, tag, fresh))
    }
  } else if (chance(rng, 0.5)) {
    entry.config = value(rng, 2)
  }
  return entry
}

const OVERRIDE_KEYS = ['disabled', 'group', 'isolate', 'inject', 'custom', 'x', 'config']

function patch(rng, ids, tag, fresh) {
  const one = {}
  if (chance(rng, 0.45)) {
    one.insert = []
    for (let i = between(rng, 0, 3); i > 0; i--) one.insert.push(row(rng, ids, 2, tag, fresh))
    if (chance(rng, 0.6)) {
      // A target that exists, or one no row has: the rows are skipped.
      one.id = chance(rng, 0.85) ? pick(rng, ids) : `missing${int(rng, 3)}`
    }
    return one
  }
  if (chance(rng, 0.92)) {
    one.id = chance(rng, 0.15)
      ? ''
      : chance(rng, 0.8)
        ? pick(rng, ids)
        : `missing${int(rng, 2)}`
  }
  if (chance(rng, 0.3)) one.name = pick(rng, [...NAMES, 'other'])
  for (let i = between(rng, 0, 3); i > 0; i--) {
    const key = pick(rng, OVERRIDE_KEYS)
    if (key in one) continue
    one[key] = value(rng, 2)
  }
  return one
}

function compose(caseIndex) {
  const rng = mulberry32(0x6310 + caseIndex * 7919)
  const ids = ['a', 'b', 'c', 'g', 'h']
  const fresh = { count: 0 }
  const layers = []
  for (let layer = between(rng, 0, 4); layer > 0; layer--) {
    const patches = []
    for (let i = between(rng, 0, 5); i > 0; i--) {
      patches.push(patch(rng, ids, `${caseIndex}_${layer}`, fresh))
    }
    layers.push(patches)
  }
  return { name: `random ${caseIndex}`, layers }
}

const cases = Array.from({ length: 120 }, (_, index) => compose(index))
const expected = cases.map((one) => {
  const warnings = []
  // The original mutates the rows it inserts, so it composes a clone, as
  // the README of these fixtures does.
  const rows = applyEntryPatches([], structuredClone(one.layers.flat()), (message) =>
    warnings.push(message),
  )
  return { name: one.name, layers: one.layers, rows, warnings: warnings.length }
})
process.stdout.write(JSON.stringify(expected) + '\n')
