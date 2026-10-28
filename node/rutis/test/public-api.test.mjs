// The public API of @arcships/rutis is its hand-written `.d.ts` files (#692,
// SDK entry design §6). This test keeps the code in step with them: the
// package exposes only `.`, `./advanced` and `./testing`, and each entry's
// runtime exports are exactly the values its `.d.ts` declares. Adding a
// name means adding it to the `.d.ts`, where review sees it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

const root = new URL('../', import.meta.url)
const pkg = JSON.parse(readFileSync(new URL('package.json', root), 'utf8'))

// Values a `.d.ts` declares, read line by line: `export declare const|let|
// function|class NAME`, and `export { a, b as c }` without `type`.
function declared(file) {
  const names = new Set()
  for (const line of readFileSync(new URL(file, root), 'utf8').split('\n')) {
    const value = line.match(/^export declare (?:const|let|var|function|class|enum) (\w+)/)
    if (value) names.add(value[1])
    const list = line.match(/^export \{([^}]*)\}/)
    if (list) for (const item of list[1].split(',')) {
      const name = item.trim().split(/\s+as\s+/).pop()
      if (name && !item.trim().startsWith('type ')) names.add(name)
    }
  }
  return [...names].sort()
}

test('the package has three entries: the root, advanced and testing', () => {
  const entries = Object.keys(pkg.exports).filter(entry => entry !== './package.json')
  assert.deepEqual(entries, ['.', './advanced', './testing'])
})

for (const [entry, { types, default: code }] of Object.entries(pkg.exports)) {
  if (entry === './package.json') continue
  test(`${entry}: the runtime exports are what ${types.slice(2)} declares`, async () => {
    const exported = Object.keys(await import(new URL(code, root))).sort()
    assert.deepEqual(exported, declared(types),
      `the public API of @arcships/rutis changed: export a value and declare it in ${types.slice(2)} together`)
  })
}
