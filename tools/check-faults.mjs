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

// A test is there when its file has its name: as a whole word for a function,
// as written for a Node or Bun test's title (quotes in it may be escaped).
const escape = text => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
for (const [index, fault] of faults.entries()) {
  for (const { id } of languages) {
    const cell = fault[id]
    if (!cell) {
      problems.push(`fault ${index + 1} (${fault.en}) has no entry for ${id}`)
      continue
    }
    if (!cell.tests && !cell.missing && !cell.na && !cell.pending) {
      problems.push(`fault ${index + 1} (${fault.en}), ${id}: give tests, missing, na or pending`)
    }
    for (const [file, name] of cell.tests ?? []) {
      if (!existsSync(new URL(file, root))) {
        problems.push(`fault ${index + 1}, ${id}: ${file} does not exist`)
        continue
      }
      const text = read(file)
      const found = /^\w+$/.test(name)
        ? new RegExp(`\\b${escape(name)}\\b`).test(text)
        : text.replace(/\\(['"`])/g, '$1').includes(name)
      if (!found) problems.push(`fault ${index + 1}, ${id}: ${file} has no test ${name}`)
    }
  }
}

const words = {
  zh: {
    head: ['#', '故障', ...languages.map(l => l.zh), 'issue'],
    tests: n => `${n} 项`,
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
