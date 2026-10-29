#!/usr/bin/env node
// Check the guarantee register (docs/guarantees.md, .en.md) against the
// tests, read only (§4, §5 of the register):
//
// - every test a row's "verification" column lists is there: its file, and
//   the function or test named (`path::fn`, `path::Class::test`, `::fn`
//   after a path, `path` (`test name`, …), Swift `Type.method`, Kotlin
//   `Type.method`);
// - every file a row lists carries a `guarantee:` marker naming the row;
// - every `guarantee:` marker in the repository names a row;
// - both languages list the same rows, with the same level and status.
//
//   node tools/check-guarantees.mjs            the problems, exit 1 if any
//   node tools/check-guarantees.mjs --counts   also the §16 counts
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'

const root = new URL('../', import.meta.url)
const read = path => readFileSync(new URL(path, root), 'utf8')
const exists = path => existsSync(new URL(path, root))

// The rows of one register: id → { level, status, verification, line }.
function rows(file, problems) {
  const found = new Map()
  let header = null
  read(file).split('\n').forEach((line, index) => {
    // A line inside a table that is not a row: a row broken in two.
    if (header && line.trim() && !line.startsWith('|')) { problems.push(`${file}:${index + 1}: a table row broken across lines`); return }
    if (!line.startsWith('|')) { header = null; return }
    const cells = line.slice(1, -1).split(/(?<!\\)\|/).map(cell => cell.trim())
    if (['编号', 'ID', 'Id', 'Number'].includes(cells[0])) { header = cells; return }
    if (!header || /^-+$/.test(cells[0])) return
    const id = /^`([^`]+)`$/.exec(cells[0])?.[1]
    if (!id) return
    const at = name => cells[header.findIndex(cell => name.includes(cell))]
    found.set(id, {
      level: at(['级别', 'Level']),
      status: at(['状态', 'Status']),
      verification: at(['验证', 'Verification', 'Verified by']),
      line: index + 1,
    })
  })
  return found
}

// Where a path of the register is: relative to crates/, or to the root.
// (`rutis-e2e/` is the e2e crate, at tests/e2e.)
const ROOTS = ['crates/', 'node/', 'bun/', 'python/', 'go/', 'swift/', 'kotlin/', 'tests/', 'tools/', 'specs/', 'docs/', 'examples/', 'fuzz/', '.github/']
function located(path) {
  if (path.startsWith('rutis-e2e/')) return `tests/e2e/${path.slice('rutis-e2e/'.length)}`
  if (ROOTS.some(prefix => path.startsWith(prefix))) return path
  return `crates/${path}`
}

// Whether `file` defines the test `name`.
function defines(file, name) {
  const text = read(file)
  // `Class::test` / `module::tests::test`: the last part is the test.
  const last = name.split('::').at(-1)
  const plain = last.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  return new RegExp(`\\b(fn|def|func|fun|async fn|async def|class|mod)\\s+${plain}\\b`).test(text)
    || text.includes(`'${last.replaceAll("'", "\\'")}'`) || text.includes(`"${last}"`) || text.includes(`\`${last}\``)
    || new RegExp(`\\b${plain}\\b`).test(text) && /\.(swift|kt)$/.test(file)
}

// The tests a verification cell lists: [{ file, name? }].
function tests(cell) {
  const listed = []
  let file = null
  const text = cell ?? ''
  // `path::a::b`, `::b`, `path`, then (`name`, `name`) after a path.
  const token = /\b(Swift|Kotlin)\s+`([^`/]+)`|`([^`]+)`|（([^）]*)）|\(([^)]*)\)/g
  let match
  while ((match = token.exec(text))) {
    if (match[1]) {
      const [type, ...method] = match[2].split('.')
      listed.push({ language: match[1], type, name: method.join('.') })
      continue
    }
    const group = match[4] ?? match[5]
    if (group !== undefined) {
      if (!file || !/\.(mjs|ts|js)$/.test(file)) continue
      for (const name of group.matchAll(/`([^`]+)`/g)) listed.push({ file, name: name[1] })
      continue
    }
    const code = match[3]
    if (code.startsWith('::')) {
      if (file) listed.push({ file, name: code.slice(2) })
      continue
    }
    const [path, ...rest] = code.split('::')
    if (!/\.(rs|mjs|ts|js|py|go|swift|kt)$/.test(path) || path.includes(' ')) continue
    file = located(path)
    listed.push(rest.length ? { file, name: rest.join('::') } : { file })
  }
  return listed
}

// Every `guarantee:` marker outside docs/: { file, line, ids }.
function markers() {
  const out = execFileSync('git', ['grep', '-n', '-I', 'guarantee:', '--', '.', ':!docs', ':!tools/check-guarantees.mjs'], { cwd: root, encoding: 'utf8' })
  // Prose that mentions the word (`` `guarantee:` ``) is not a marker.
  return out.split('\n').filter(line => line && !line.includes('`guarantee:`')).map(line => {
    const [, file, number, text] = /^([^:]+):(\d+):(.*)$/.exec(line)
    const after = text.slice(text.indexOf('guarantee:') + 'guarantee:'.length)
    const ids = [...after.matchAll(/(?:^|[\s,(、])((?:core|boundary|events|instance|bridge|host|dsh|sdk|loader|swift|kotlin|go|node|python|bun)\/[A-Za-z0-9.*_-]+)/g)].map(m => m[1].replace(/[.,;:]+$/, ''))
    return { file, line: Number(number), ids }
  })
}

const problems = []
const zh = rows('docs/guarantees.md', problems)
const en = rows('docs/guarantees.en.md', problems)
for (const id of zh.keys()) if (!en.has(id)) problems.push(`docs/guarantees.en.md: no row ${id}`)
for (const id of en.keys()) if (!zh.has(id)) problems.push(`docs/guarantees.md: no row ${id}`)

const marked = markers()
const markedIds = new Map()
for (const { file, ids } of marked) for (const id of ids) {
  if (!markedIds.has(id)) markedIds.set(id, new Set())
  markedIds.get(id).add(file)
}

for (const [file, register] of [['docs/guarantees.md', zh], ['docs/guarantees.en.md', en]]) {
  for (const [id, row] of register) {
    for (const test of tests(row.verification)) {
      if (test.language) {
        const files = execFileSync('git', ['ls-files', test.language === 'Swift' ? 'swift' : 'kotlin'], { cwd: root, encoding: 'utf8' }).split('\n')
        const home = files.find(path => path.endsWith(`/${test.type}.${test.language === 'Swift' ? 'swift' : 'kt'}`))
        if (!home) problems.push(`${file}:${row.line}: ${id}: ${test.language} ${test.type} not found`)
        else if (test.name && !defines(home, test.name)) problems.push(`${file}:${row.line}: ${id}: ${home} has no ${test.name}`)
        continue
      }
      if (!exists(test.file)) { problems.push(`${file}:${row.line}: ${id}: ${test.file} does not exist`); continue }
      if (test.name && !defines(test.file, test.name)) problems.push(`${file}:${row.line}: ${id}: ${test.file} has no ${test.name}`)
      if (file === 'docs/guarantees.md' && !markedIds.get(id)?.has(test.file)) problems.push(`${file}:${row.line}: ${id}: ${test.file} has no guarantee: ${id} marker${test.name ? ` (${test.name})` : ''}`)
    }
  }
}
for (const { file, line, ids } of marked) {
  if (!ids.length) { problems.push(`${file}:${line}: a guarantee: marker that names no row`); continue }
  for (const id of ids) if (!zh.has(id) && !/\*$/.test(id)) problems.push(`${file}:${line}: guarantee: ${id} is no row`)
}

const counted = [...new Set(problems)]
for (const line of counted) console.log(line)
console.log(`${zh.size} rows, ${marked.length} markers: ${counted.length} problems`)

if (process.argv.includes('--counts')) {
  const tally = {}
  for (const [id, row] of zh) {
    const key = `${row.level} / ${row.status}`
    tally[key] = (tally[key] ?? 0) + 1
  }
  console.log(tally)
}
process.exitCode = counted.length ? 1 : 0
