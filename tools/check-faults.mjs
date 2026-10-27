// The fault acceptance summary (#690): the 14 faults of #663 × each language,
// and the tests that cause each and compare what the developer sees.
//
//   node tools/check-faults.mjs          fail if a listed test is gone, or
//                                        docs/faults(.en).md is not what
//                                        tests/faults.json gives
//   node tools/check-faults.mjs --write  write the tables into docs/faults(.en).md
//
// With GITHUB_STEP_SUMMARY set (CI), the table goes into the job's summary.
// The tests themselves run in ci.yml's jobs (docs/faults.md says which).
import { appendFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs'

const root = new URL('../', import.meta.url)
const read = path => readFileSync(new URL(path, root), 'utf8')
const { languages, faults } = JSON.parse(read('tests/faults.json'))

const problems = []

// A test is there when its file defines it: `fn name` (Rust), `def name`
// (Python), `func name(` (Go), or `test('title'` / `it('title'` (Node, Bun;
// quotes in the title may be escaped). A name in a comment does not count.
const escape = text => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
const definition = (file, name) => {
  if (/\.(mjs|js|ts)$/.test(file)) {
    return new RegExp(`\\b(test|it)\\(\\s*(['"\`])${escape(name)}\\2`)
  }
  if (file.endsWith('.py')) return new RegExp(`\\bdef ${escape(name)}\\b`)
  if (file.endsWith('.go')) return new RegExp(`\\bfunc ${escape(name)}\\(`)
  return new RegExp(`\\bfn ${escape(name)}\\b`)
}
const keys = new Set(['tests', 'missing', 'na', 'pending'])
for (const [index, fault] of faults.entries()) {
  for (const language of languages) {
    const where = `fault ${index + 1} (${fault.en}), ${language.en}`
    const cell = fault[language.id]
    if (!cell) {
      problems.push(`${where}: no entry`)
      continue
    }
    for (const key of Object.keys(cell)) {
      if (!keys.has(key)) problems.push(`${where}: unknown key ${key} (use tests, missing, na or pending)`)
    }
    if (!cell.tests && !cell.missing && !cell.na) {
      problems.push(`${where}: give tests, missing or na`)
    }
    for (const [file, name] of cell.tests ?? []) {
      if (!existsSync(new URL(file, root))) {
        problems.push(`${where}: ${file} does not exist`)
        continue
      }
      const text = read(file).replace(/\\(['"`])/g, '$1')
      if (!definition(file, name).test(text)) problems.push(`${where}: ${file} defines no test ${name}`)
    }
  }
}

const words = {
  zh: {
    head: ['#', '故障', ...languages.map(l => l.zh), 'issue'],
    tests: n => `${n} 个测试`,
    pending: issue => `待接 ${issue}`,
    missing: '无测试',
    na: '不适用',
    join: '，',
    colon: '：',
  },
  en: {
    head: ['#', 'Fault', ...languages.map(l => l.en), 'Issues'],
    tests: n => `${n} test${n === 1 ? '' : 's'}`,
    pending: issue => `pending ${issue}`,
    missing: 'no test',
    na: 'n/a',
    join: '; ',
    colon: ': ',
  },
}

const link = issue => `[${issue}](https://github.com/arcships/rutis/issues/${issue.slice(1)})`
const cellText = (cell, w) => {
  const parts = []
  if (cell.tests) parts.push(w.tests(cell.tests.length))
  if (cell.missing) parts.push(w.missing)
  if (cell.na) parts.push(w.na)
  if (cell.pending) parts.push(w.pending(link(cell.pending)))
  return parts.join(w.join)
}

const row = cells => `| ${cells.join(' | ')} |`
const table = lang => {
  const w = words[lang]
  return [
    row(w.head),
    row(w.head.map(() => '---')),
    ...faults.map((fault, index) =>
      row([index + 1, fault[lang], ...languages.map(({ id }) => cellText(fault[id], w)), fault.issues.map(link).join(' ')])),
  ].join('\n')
}

const details = lang => faults.flatMap((fault, index) => [
  `### ${index + 1}. ${fault[lang]}`,
  '',
  ...languages.map(language => {
    const cell = fault[language.id]
    const tests = (cell.tests ?? []).map(([file, name]) => `\`${file}\` ${/^\w+$/.test(name) ? `\`${name}\`` : `“${name}”`}`)
    const rest = cellText({ ...cell, tests: undefined }, words[lang])
    return `- ${language[lang]}${words[lang].colon}${[...tests, rest].filter(Boolean).join(words[lang].join)}`
  }),
  '',
]).join('\n').trimEnd()

const begin = '<!-- faults:begin (node tools/check-faults.mjs --write) -->'
const end = '<!-- faults:end -->'
const generated = lang => `${begin}\n\n${table(lang)}\n\n${details(lang)}\n\n${end}`

for (const [lang, path] of [['zh', 'docs/faults.md'], ['en', 'docs/faults.en.md']]) {
  const text = read(path)
  const from = text.indexOf(begin)
  const to = text.indexOf(end)
  if (from < 0 || to < from) {
    problems.push(`${path} has no ${begin} … ${end} block`)
    continue
  }
  const next = text.slice(0, from) + generated(lang) + text.slice(to + end.length)
  if (process.argv.includes('--write')) writeFileSync(new URL(path, root), next)
  else if (next !== text) problems.push(`${path} is not what tests/faults.json gives: run node tools/check-faults.mjs --write`)
}

if (process.env.GITHUB_STEP_SUMMARY) {
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, `## Fault acceptance (#690, docs/faults.en.md)\n\n${table('en')}\n\n`)
}

if (problems.length) {
  for (const problem of problems) console.error(problem)
  process.exit(1)
}
console.log(`${faults.length} faults × ${languages.length} languages: every listed test is there`)
