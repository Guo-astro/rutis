#!/usr/bin/env node
// Rule trace table: docs/formal-trace/ (§5 of
// docs/design-formal-verification-2026-11-08.md, issue #634).
//
//   node tools/formal-trace.mjs            counts, then checks; exit 1 on a problem
//   node tools/formal-trace.mjs --write    regenerate the guarantee rows first
//   node tools/formal-trace.mjs --check    checks only (no counts)
//
// The guarantee rows (rules/guarantees-*.md) are the register's rows
// (docs/guarantees.md, §6–§15), one row per register row, extracted from the
// Markdown tables and never written by hand: `--write` rewrites the automatic
// columns (id, source, plain-language promise, level, evidence, status), keeps
// the manual ones (scope, mutation or counterexample, gap) by id, and refreshes
// the index in README.md between <!-- index:begin --> and <!-- index:end -->.
// `--check` reports a row that drifted from the register, a row the register
// has and the table does not (and the reverse), a status or level outside the
// vocabulary, a duplicate id, and an evidence path, test name or line range
// that does not resolve. The trace rows (rules/trace-contract.md,
// rules/event-close-admission.md) are written by hand and checked the same way.
//
// The protocol rows (rules/protocol-*.md) are the exported tables of issue
// #634's section comments, one row per rule, kept verbatim in the issue's ten
// columns and never rewritten by this script. Their status is the issue's
// vocabulary (已覆盖, 部分, …); the script maps it to the table's status at
// read time (§3 of the README maps only downwards), checks the id form, the
// per-section totals of #634's summary table, and the evidence the 现有形式化
// column names. Replaced rows are kept and not counted.
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { posix } from 'node:path'
import { fileURLToPath } from 'node:url'
import { ANCHORS, NOTES, REGISTRY_FAMILIES, SOURCES, SUPERSEDED, UNMAPPED } from './formal-trace-recheck.mjs'
import { SEMANTIC_DOCS, SEMANTIC_PAGE, SEMANTIC_PARTITIONS } from './formal-trace-semantic.mjs'

const root = fileURLToPath(new URL('../', import.meta.url))
// The two documents of the events partitions, by the prefix their anchors use.
const EVENT_DOCS = { EVP: 'design-events-protocol-2026-10-18.md', EVD: 'design-events-2026-10-18.md' }
const read = file => readFileSync(`${root}${file}`, 'utf8')
const exists = file => existsSync(`${root}${file}`)
const lineCount = file => read(file).split('\n').length

const REGISTER = 'docs/guarantees.md'
const RULES = 'docs/formal-trace/rules'
const INDEX = 'docs/formal-trace/README.md'
const RECHECK = 'docs/formal-trace/recheck-0.22.md'
const RECHECK_BEGIN = '<!-- recheck-index:begin -->'
const RECHECK_END = '<!-- recheck-index:end -->'
const SESSION_SPEC = 'docs/protocol-session.md'
const INDEX_BEGIN = '<!-- index:begin -->'
const INDEX_END = '<!-- index:end -->'

// The columns of a row, in the order §5 lists the fields.
const COLUMNS = ['编号', '现行来源', '人话承诺', '级别', '适用范围', '证据', '变异或反例', '状态', '缺口']

// One file per guarantee family: [file, title, families, register sections].
const FILES = [
  ['guarantees-core.md', '`core/*`', ['core'], '§6、§7'],
  ['guarantees-events.md', '`events/*`', ['events'], '§7、§8a'],
  ['guarantees-boundary.md', '`boundary/*`', ['boundary'], '§8'],
  ['guarantees-instance.md', '`instance/*`', ['instance'], '§9'],
  ['guarantees-bridge.md', '`bridge/*`', ['bridge'], '§10'],
  ['guarantees-host.md', '`host/*`、`dsh/*`', ['host', 'dsh'], '§11、§13、§15'],
  ['guarantees-sdk.md', '`sdk/*`', ['sdk'], '§12'],
  ['guarantees-loader.md', '`loader/*`、`swift/*`', ['loader', 'swift'], '§14'],
]
const HANDWRITTEN = ['trace-contract.md', 'event-close-admission.md']

// The exported #634 section comments: one file per section (the session
// section is one file with two tables), with the current and replaced counts
// of #634's summary table. `id` says which numbering the section uses.
const PROTOCOL = [
  { file: 'protocol-registry-1.md', partition: 'registry-1', label: '#634 · registry-1', comment: 6384932214, current: 156, replaced: 9, id: 'registry' },
  { file: 'protocol-registry-2.md', partition: 'registry-2', label: '#634 · registry-2', comment: 6384932508, current: 218, replaced: 0, id: 'registry' },
  { file: 'protocol-events.md', partition: 'events', label: '#634 · events', comment: 6384932886, current: 253, replaced: 9, id: 'protocol' },
  { file: 'protocol-session.md', partition: 'session', label: '#634 · session 上/下', comment: 6384933233, current: 246, replaced: 104, id: 'protocol' },
  { file: 'protocol-runtime.md', partition: 'runtime', label: '#634 · runtime', comment: 6384933883, current: 264, replaced: 45, id: 'protocol' },
  { file: 'protocol-remote.md', partition: 'remote', label: '#634 · remote', comment: 6384934401, current: 231, replaced: 17, id: 'protocol' },
]
const PROTOCOL_COLUMNS = ['编号', '来源', '规则', '参与方', '手段', '对应', '现有形式化', '状态', '归属 issue', '备注']
const ISSUE_STATUSES = ['已覆盖', '部分', '未覆盖', '不适用', '明确不保证', '未登记', '已取代']
const PROTOCOL_PREFIXES = ['EVP', 'EVD', 'SES', 'VAL', 'TAG', 'M1', 'CHN', 'SYN', 'M2', 'MNT', 'REQ', 'RTP', 'INS', 'MSV', 'REM', 'DIA', 'APP', 'NOD']
const EVIDENCE_EXTENSIONS = /\.(?:rs|mjs|ts|js|py|go|swift|kt|yml|json|toml|md)$/

const LEVELS = ['核心承诺', '契约行为', '明确不保证', '—']
const STATUSES = ['已验证', '部分验证', '未验证', '明确不保证', '已撤回', '不单独计数']

// Cells of a Markdown table row; `\|` stays escaped, as the register writes it.
const cellsOf = line => line.slice(1, -1).split(/(?<!\\)\|/).map(cell => cell.trim())

// A cell as it is written: one line, `|` escaped.
const cell = text => String(text ?? '').replace(/\r?\n/g, ' ').replace(/(?<!\\)\|/g, '\\|').trim()

// The rows of a table whose first column is an id in code spans.
function table(text) {
  const rows = []
  let header = null
  for (const [index, line] of text.split('\n').entries()) {
    if (!line.startsWith('|')) {
      if (line.trim()) header = null
      continue
    }
    const found = cellsOf(line)
    if (['编号', 'ID', 'Id'].includes(found[0])) { header = found; continue }
    if (!header || /^-+$/.test(found[0])) continue
    const id = /^`([^`]+)`$/.exec(found[0])?.[1]
    if (!id) continue
    const row = { id, line: index + 1 }
    for (const [column, name] of header.entries()) row[name] = found[column] ?? ''
    rows.push(row)
  }
  return rows
}

// The register rows, in file order, with the section they sit under.
function register() {
  const rows = []
  let section = null
  let header = null
  for (const [index, line] of read(REGISTER).split('\n').entries()) {
    const heading = /^## (.+)$/.exec(line)
    if (heading) {
      section = heading[1]
      header = null
      continue
    }
    if (!line.startsWith('|')) {
      if (line.trim()) header = null
      continue
    }
    const found = cellsOf(line)
    if (['编号', 'ID', 'Id'].includes(found[0])) { header = found; continue }
    if (!header || /^-+$/.test(found[0])) continue
    const id = /^`([^`]+)`$/.exec(found[0])?.[1]
    if (!id) continue
    const at = name => {
      const column = header.indexOf(name)
      return column < 0 ? '' : found[column] ?? ''
    }
    const number = /^(\d+[a-z]?)\./.exec(section ?? '')?.[1] ?? ''
    rows.push({
      id,
      line: index + 1,
      section: number,
      origin: at('出处'),
      content: at('内容'),
      level: at('级别'),
      status: at('状态'),
      evidence: at('验证'),
      note: at('说明') || at('备注'),
    })
  }
  return rows
}

// A link of the register, made to work from rules/: the register's links are
// written in docs/, and rules/ is two levels down (`../specs/…` included).
const relink = text => text.replace(/\]\(([^)\s]+)\)/g, (whole, target) =>
  /^(?:[a-z][a-z0-9+.-]*:|\/|#|\.\.\/\.\.\/)/i.test(target) ? whole : `](../../${target})`)

// The register's status, in the table's vocabulary.
function status(row) {
  if (/^已撤回/.test(row.status)) return '已撤回'
  if (row.status === '—' || !row.status) return '不单独计数'
  return row.status
}

// The automatic columns of one register row.
function automatic(row) {
  return {
    编号: `\`${row.id}\``,
    现行来源: `[guarantees.md](../../guarantees.md) §${row.section}${row.origin ? ` · ${relink(row.origin)}` : ''}`,
    人话承诺: relink(row.content),
    级别: row.level,
    证据: row.evidence || '—',
    状态: status(row),
  }
}

// The manual columns a new row starts with: the register's own note is the gap
// it already names; a verified row without one has none.
function defaults(row) {
  const note = row.note?.trim()
  let gap = '未登记'
  if (/^已撤回/.test(row.status)) gap = note ? `${row.status}；${note}` : row.status
  else if (note) gap = note
  else if (status(row) === '已验证') gap = '—'
  return { 适用范围: '未登记', 变异或反例: '未登记', 缺口: gap }
}

// The manual columns already in a rules file, by id.
function manual(file) {
  const kept = new Map()
  if (!exists(file)) return kept
  for (const row of table(read(file))) {
    if (!exists(file)) continue
    kept.set(row.id, {
      适用范围: row.适用范围 ?? '未登记',
      变异或反例: row.变异或反例 ?? '未登记',
      缺口: row.缺口 ?? '未登记',
    })
  }
  return kept
}

const header = (title, sections) => `# 规则：${title}（保证登记表 ${sections}）

[中文索引](../README.md) · [English](../README.en.md) · 数据来源：[保证登记表](../../guarantees.md)

由 \`node tools/formal-trace.mjs --write\` 从登记表生成：自动列（编号、现行来源、人话承诺、级别、证据、状态）不要手改，手改会被 \`--check\` 报出；人工列（适用范围、变异或反例、缺口）按编号保留。缺口的初始值取登记表的「说明」列。

编号沿用登记表，一经登记不复用；撤销的行保留。

| ${COLUMNS.join(' | ')} |
| ${COLUMNS.map(() => '---').join(' | ')} |`

// The register's links are written in docs/; a link that does not resolve
// from rules/ but does from docs/ gets the `../../` it needs. Idempotent, so
// it also repairs the links copied from the register's notes column.
function repair(body) {
  return body.replace(/\]\(([^)\s]+)\)/g, (whole, target) => {
    if (/^(?:[a-z][a-z0-9+.-]*:|\/|#)/i.test(target)) return whole
    const bare = target.replace(/[#?].*$/, '')
    if (!bare || exists(posix.normalize(posix.join(RULES, bare)))) return whole
    return exists(posix.normalize(posix.join('docs', bare))) ? `](../../${target})` : whole
  })
}

function write() {
  const registerRows = register()
  const changed = []
  mkdirSync(`${root}${RULES}`, { recursive: true })
  for (const [file, title, families, sections] of FILES) {
    const kept = manual(`${RULES}/${file}`)
    const body = registerRows
      .filter(row => families.includes(row.id.split('/')[0]))
      .map(row => {
        const fields = { ...defaults(row), ...automatic(row), ...(kept.get(row.id) ?? {}) }
        return `| ${COLUMNS.map(column => cell(fields[column] ?? '')).join(' | ')} |`
      })
    const content = repair(`${header(title, sections)}\n${body.join('\n')}\n`)
    if (!exists(`${RULES}/${file}`) || read(`${RULES}/${file}`) !== content) {
      writeFileSync(`${root}${RULES}/${file}`, content)
      changed.push(file)
    }
  }
  console.log(changed.length ? `wrote ${changed.join(', ')}` : 'rules files are current')
}

const rowsOf = file => table(read(file)).map(row => ({ ...row, file }))

// Every row of the table: the generated files and the hand-written ones.
function rules() {
  const found = FILES.flatMap(([file]) => (exists(`${RULES}/${file}`) ? rowsOf(`${RULES}/${file}`) : []))
  for (const file of HANDWRITTEN) if (exists(`${RULES}/${file}`)) found.push(...rowsOf(`${RULES}/${file}`))
  return found
}

// The protocol tables keep the issue's columns and its plain ids; `table()`
// above only accepts ids in code spans, so they get their own reader.
function protocolTable(text) {
  const rows = []
  let header = null
  for (const [index, line] of text.split('\n').entries()) {
    if (!line.startsWith('|')) {
      if (line.trim()) header = null
      continue
    }
    const found = cellsOf(line)
    if (found[0] === '编号') { header = found; continue }
    if (!header || /^-+$/.test(found[0])) continue
    const id = found[0].replace(/^`|`$/g, '').trim()
    if (!id) continue
    const row = { id, line: index + 1, cells: found.length, width: header.length }
    for (const [column, name] of header.entries()) row[name] = found[column] ?? ''
    rows.push(row)
  }
  return rows
}

// Files named by an evidence cell, resolved against the tree: an exact path,
// a unique path suffix (`tests/foo.rs` for `crates/x/tests/foo.rs`), or a
// unique basename for the issue's shorthand (`rpc.rs`). A qualified path with
// no match is a missing path; a bare name with no match is unresolved
// shorthand, reported but not failed (the issue uses both).
const SKIP_DIRECTORIES = new Set(['node_modules', 'target', '.git', 'dist', 'build', '__pycache__', '.venv', 'venv', 'vendor'])
let protocolIndex = null
function protocolPaths() {
  if (protocolIndex) return protocolIndex
  const all = []
  const walk = relative => {
    for (const entry of readdirSync(`${root}${relative}`, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        if (!SKIP_DIRECTORIES.has(entry.name)) walk(`${relative}${entry.name}/`)
      } else all.push(`${relative}${entry.name}`)
    }
  }
  for (const prefix of ROOTS) if (exists(prefix)) walk(prefix)
  const byBase = new Map()
  for (const path of all) {
    const base = path.split('/').at(-1)
    if (!byBase.has(base)) byBase.set(base, [])
    byBase.get(base).push(path)
  }
  protocolIndex = { all, byBase }
  return protocolIndex
}

function resolveProtocolPath(path) {
  if (exists(path)) return { kind: 'exact', file: path }
  const { all, byBase } = protocolPaths()
  if (path.includes('/')) {
    const matches = all.filter(candidate => candidate.endsWith(`/${path}`))
    if (matches.length === 1) return { kind: 'suffix', file: matches[0] }
    if (matches.length > 1) return { kind: 'ambiguous', files: matches }
    return { kind: 'missing', qualified: true }
  }
  const matches = byBase.get(path) ?? []
  if (matches.length === 1) return { kind: 'base', file: matches[0] }
  if (matches.length > 1) return { kind: 'ambiguous', files: matches }
  return { kind: 'missing', qualified: false }
}

// The backticked file references of a protocol evidence cell, with the
// `::name` test and the `:from-to` lines when the issue writes them.
function protocolEvidenceOf(text) {
  const found = []
  for (const match of String(text ?? '').matchAll(/`([^`]+)`/g)) {
    const token = match[1].trim()
    const [pathPart, ...rest] = token.split('::')
    const name = rest.length ? rest.join('::') : null
    const range = /^(.+?):(\d+)(?:-(\d+))?$/.exec(pathPart)
    const path = range ? range[1] : pathPart
    if (!EVIDENCE_EXTENSIONS.test(path) || !/^[\w./\-]+$/.test(path)) continue
    found.push({
      token,
      path,
      name,
      from: range ? Number(range[2]) : null,
      to: range ? Number(range[3] ?? range[2]) : null,
      ...resolveProtocolPath(path),
    })
  }
  return found
}

// The issue's status, in the table's vocabulary. It only moves down: `已覆盖`
// is `已验证` only when the row names an implementation check and every file
// it names still resolves; a TLA-only row, a row without a name, or a row
// whose evidence is gone is `部分验证`.
const implementationMeans = means => String(means ?? '').split('+').map(part => part.trim())
  .some(part => /^(?:K(?:-|$)|PT$|FZ$|TV$|SH$|KANI$|测试$)/.test(part))
const namedEvidence = text => /`[^`]+`/.test(String(text ?? ''))
// An evidence cell that starts with a model and names no implementation check
// is model coverage only, even when the issue's `手段` column also lists K/PT
// (the means is the plan, the evidence is what exists).
const implementationEvidence = text => {
  const source = String(text ?? '')
  return /(?:^|[；;、,，\s])(?:K(?=[\s`：\-+（(]|$)|PT(?=[\s`：+（(]|$)|FZ(?=[\s`：+（(]|$)|TV(?=[\s`：+（(]|$)|SH(?=[\s`：+（(]|$)|KANI(?=[\s`：+（(]|$)|测试(?=[\s`：+（(]|$))/.test(source)
    || /\b(?:session|channel)::testing\b/.test(source)
    || /\b[\w.-]+\.(?:rs|mjs|ts|js|py|go|swift|kt)::/.test(source)
    || /\b[\w.-]+\.(?:expected\.json|test\.mjs|test\.ts|test\.js)\b/.test(source)
    || /\b[\w.-]+_test\.go\b/.test(source)
    || /\btests?\//.test(source)
}
const modelEvidence = text => {
  const source = String(text ?? '').trim()
  if (!/^(?:TLA\b|`(?:CrossRuntimeSync|RemoteLease|AccessPoint|remote-lease|cross-runtime-sync)`|(?:CrossRuntimeSync|RemoteLease|AccessPoint|RL|AP)\b)/.test(source)) return false
  return !implementationEvidence(source)
}
// The rule itself says it is a non-guarantee; for the issue's `不适用` rows
// the note is where that is written. A note that says one *should* be
// registered is not a non-guarantee yet.
const explicitNonGuarantee = row => /明确不保证/.test(row.规则 ?? '')
  || (row.状态 === '不适用' && /明确不保证/.test(row.备注 ?? ''))

function protocolStatus(row) {
  const issue = row.状态
  if (issue === '已取代') return '已撤回'
  if (issue === '明确不保证' || explicitNonGuarantee(row)) return '明确不保证'
  if (issue === '不适用') return '不单独计数'
  if (issue === '未登记' || issue === '未覆盖') return '未验证'
  if (issue === '部分') return '部分验证'
  if (issue !== '已覆盖') return '未验证'
  if (!implementationMeans(row.手段)) return '部分验证'
  if (modelEvidence(row.现有形式化)) return '部分验证'
  if (!namedEvidence(row.现有形式化)) return '部分验证'
  if (protocolEvidenceOf(row.现有形式化).some(item => item.kind === 'missing')) return '部分验证'
  return '已验证'
}

// Why a `已覆盖` row did not become `已验证`; the gap column carries it.
function protocolDowngrade(row) {
  if (row.状态 !== '已覆盖') return null
  if (!implementationMeans(row.手段)) return '现有形式化只有 TLA，不是实现证据'
  if (modelEvidence(row.现有形式化)) return '现有形式化只有 TLA/模型，没有实现检查的证据'
  if (!namedEvidence(row.现有形式化)) return '没有具名证据'
  if (protocolEvidenceOf(row.现有形式化).some(item => item.kind === 'missing')) return '具名证据在当前树找不到'
  return null
}

function protocolLevel(row) {
  return row.状态 === '明确不保证' || explicitNonGuarantee(row) ? '明确不保证' : '契约行为'
}

function protocolGap(row, status) {
  const parts = []
  if (row.状态 === '已取代') parts.push('已取代')
  else if (row.状态 === '未登记') parts.push('未登记（测试里用了、登记表没有）')
  else if (row.状态 === '不适用') parts.push(status === '明确不保证' ? '明确不保证（issue 状态：不适用）' : '不适用（只靠测试，不单独计数）')
  else if (row.状态 === '已覆盖' && status === '部分验证') {
    const reason = protocolDowngrade(row)
    if (reason) parts.push(`已覆盖降为部分验证：${reason}`)
  }
  const owner = row['归属 issue'] ?? ''
  if (owner && owner !== '—') parts.push(owner)
  if (row.备注) parts.push(row.备注)
  return parts.length ? parts.join('；') : '—'
}

// ── Re-check against the current 0.22 sources ───────────────────────────
//
// The issue snapshot is not the current rule: 0.19 changed the remote layer,
// protocol 4 dropped compat and the old capabilities, and the session layer
// got its own specification. `tools/formal-trace-recheck.mjs` names the
// current source of each family and the rules a later design removed. This
// pass only moves a row down: `已取代` and the rules listed as removed become
// `已撤回`, a register row the register dropped becomes `已撤回`, and a row
// whose current source is a design it replaced but has no current anchor is
// not `已验证`.

const superseded = new Map()
for (const entry of SUPERSEDED) for (const id of entry.ids) superseded.set(id, entry)

let registerStatuses = null
function registerStatusOf(id) {
  if (!registerStatuses) {
    registerStatuses = new Map()
    for (const row of register()) registerStatuses.set(row.id, status(row))
  }
  if (registerStatuses.has(id)) return { id, status: registerStatuses.get(id) }
  for (const match of [`${id} a`, `${id} b`, `${id} c`]) {
    if (registerStatuses.has(match)) return { id: match, status: registerStatuses.get(match) }
  }
  return null
}

// The register row a protocol-registry id belongs to: the id itself, the id
// without a space and a letter (`boundary/5.1a`), or without a trailing letter
// (`core/D2a` → `core/D2`).
function registerBase(id) {
  if (registerStatusOf(id)) return registerStatusOf(id)
  for (const candidate of [id.replace(/ [a-g]$/, ''), id.replace(/[a-g]$/, '')]) {
    if (candidate !== id && registerStatusOf(candidate)) return registerStatusOf(candidate)
  }
  return null
}

const familyOf = id => id.split('-')[0]

// The current source of a row: the register row for a registry id, the
// family's document otherwise.
function currentSource(row) {
  if (REGISTRY_FAMILIES.includes(row.id.split('/')[0])) {
    const base = registerBase(row.id)
    return {
      kind: base ? 'register' : 'historical',
      label: base ? `保证登记表 §${row.来源.replace(/^§/, '')}` : '保证登记表（0.22 已无此行）',
      doc: REGISTER,
      base: base?.id ?? null,
      status: base?.status ?? null,
      note: base ? '' : '登记表 0.22 不再登记这条规则',
    }
  }
  const source = SOURCES[familyOf(row.id)]
  if (!source) return { kind: 'historical', label: '未登记来源', doc: '', status: null, note: '来源文档未在本表登记' }
  return { ...source, label: `${source.label}（${row.来源}）`, base: null, status: null }
}

// The current anchors of a row and the note that says what covers it.
function anchorsOf(row) {
  if (REGISTRY_FAMILIES.includes(row.id.split('/')[0])) return { ids: [], note: '' }
  for (const [pattern, ids, note] of ANCHORS) {
    if (pattern.test(row.来源)) return { ids, note }
  }
  return { ids: [], note: '' }
}

function unmappedReason(row) {
  if (REGISTRY_FAMILIES.includes(row.id.split('/')[0])) {
    return '保证登记表 0.22 没有这一行：旧编号或已并入别的行，待重抽或删除'
  }
  for (const [pattern, why] of UNMAPPED) if (pattern.test(row.来源)) return why
  return '现行来源没有覆盖这条规则的规格或设计'
}

// Whether the current sources cover a rule at all: a current specification
// section (SESS id), a current design or guide that still contains the rule,
// or a live register row. A historical design, or a session-design section
// the current spec puts out of scope, is not covered.
function coveredOf(row, source, anchors) {
  if (source.kind === 'register') return true
  if (source.kind === 'historical') return anchors.ids.length > 0
  if (anchors.ids.length) return true
  if (familyOf(row.id) === 'SES' && /^SES §(?:四|五|九|十三)/.test(row.来源)) return false
  return true
}

// The re-checked status. Only downwards from the issue mapping, except that
// a register row whose register status says withdrawn or explicit
// non-guarantee follows the register.
function recheckStatus(row) {
  if (row.状态 === '已取代') return '已撤回'
  if (superseded.has(row.id)) return '已撤回'
  if (explicitNonGuarantee(row)) return '明确不保证'
  const mapped = protocolStatus(row)
  const source = currentSource(row)
  if (source.kind === 'register') {
    if (/^已撤回/.test(source.status ?? '')) return '已撤回'
    if (source.status === '明确不保证') return '明确不保证'
    if (source.status === '—' || !source.status) return '不单独计数'
  }
  if (row.状态 === '不适用') return '不单独计数'
  if (mapped === '已验证' && !coveredOf(row, source, anchorsOf(row))) return '部分验证'
  if (mapped === '已验证' && source.kind === 'historical' && anchorsOf(row).ids.length === 0) return '部分验证'
  return mapped
}

// The evidence cell, as the re-check scores it: implementation evidence that
// resolves, model-only evidence, or nothing that resolves.
function recheckEvidence(row) {
  const items = protocolEvidenceOf(row.现有形式化)
  const named = namedEvidence(row.现有形式化)
  const implementation = implementationEvidence(row.现有形式化)
  if (!items.length) {
    if (!named) return { kind: 'none', text: '无具名证据' }
    return {
      kind: implementation ? 'implementation' : 'model',
      text: implementation ? '具名证据是模块/用例名，不是文件路径；按原文保留' : '只有模型或手段不含实现检查',
    }
  }
  const resolved = items.filter(item => item.kind !== 'missing' && item.kind !== 'ambiguous').length
  const missing = items.length - resolved
  return {
    kind: missing ? 'partial' : implementation ? 'implementation' : 'model',
    text: `${resolved}/${items.length} 个具名引用解析${missing ? `，${missing} 个找不到` : ''}${implementation ? '' : '；只有模型或手段不含实现检查'}`,
  }
}

// The gap a re-checked row carries: why it is not verified against a current
// source, or the rule that replaced it.
function recheckGap(row, result) {
  const parts = []
  if (result.status === '已撤回') {
    if (result.issueStatus !== '已取代') {
      if (result.source.kind === 'register' && /^已撤回/.test(result.source.status ?? '')) {
        parts.push(`登记表已撤回：${result.source.status}（${result.source.base}）`)
      } else {
        const entry = superseded.get(row.id)
        parts.push(`已取代：${entry?.why ?? '当前规格已无此规则'}`)
      }
    }
  } else if (result.status !== '不单独计数' && result.status !== '明确不保证') {
    if (result.anchors.length) parts.push(`现行对应：${result.anchors.join('、')}${result.anchorNote ? `（${result.anchorNote}）` : ''}`)
    else if (result.covered) parts.push(`现行来源：${result.source.label}；没有会话规格锚点`)
    else parts.push(`无现行锚点：${result.unmapped}`)
    if (result.evidence.kind === 'none' || result.evidence.kind === 'partial') parts.push(result.evidence.text)
    if (result.status === '部分验证' && result.original === '已验证') parts.push('原状态降为部分验证：来源已历史化或证据不完整')
  } else if (result.status === '明确不保证' && result.issueStatus !== '明确不保证' && !explicitNonGuarantee(row)) {
    parts.push('登记表把这条登记为明确不保证')
  }
  if (NOTES.has(row.id) && result.status !== '已撤回') parts.push(NOTES.get(row.id))
  return parts
}

// One protocol row with everything the re-check keeps.
function rechecked(row) {
  const source = currentSource(row)
  const anchors = anchorsOf(row)
  const evidence = recheckEvidence(row)
  const status = recheckStatus(row)
  return {
    ...row,
    issueStatus: row.状态,
    source,
    anchors: anchors.ids,
    anchorNote: anchors.note,
    covered: coveredOf(row, source, anchors),
    unmapped: anchors.ids.length ? '' : unmappedReason(row),
    evidence,
    original: protocolStatus(row),
    status,
  }
}

const recheckOf = row => rechecked(row)

// The protocol rows, with the mapped status and the nine-column fields the
// index and the counts use.
function protocolRows() {
  const found = []
  for (const meta of PROTOCOL) {
    const file = `${RULES}/${meta.file}`
    if (!exists(file)) continue
    for (const row of protocolTable(read(file))) {
      const result = recheckOf(row)
      const status = result.status
      const gap = [protocolGap(row, status), ...recheckGap(row, result)].filter(part => part && part !== '—')
      found.push({
        ...row,
        file,
        meta,
        issueStatus: row.状态,
        适用范围: row.参与方 || '未登记',
        证据: row.现有形式化 || '—',
        级别: protocolLevel(row),
        状态: status,
        缺口: gap.length ? gap.join('；') : '—',
        recheck: result,
      })
    }
  }
  return found
}

// Where a path in an evidence cell is: the register writes paths relative to
// crates/, the trace rows from the root. (`rutis-e2e/` is tests/e2e.)
const ROOTS = ['crates/', 'node/', 'bun/', 'python/', 'go/', 'swift/', 'kotlin/', 'tests/', 'tools/', 'specs/', 'docs/', 'examples/', 'fuzz/', '.github/']
function locate(path) {
  if (path.startsWith('rutis-e2e/')) return `tests/e2e/${path.slice('rutis-e2e/'.length)}`
  return ROOTS.some(prefix => path.startsWith(prefix)) ? path : `crates/${path}`
}

// Whether `file` defines `name` (a test, a function, a class).
function defines(file, name) {
  const text = read(file)
  const last = name.split('::').at(-1)
  const plain = last.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const escaped = last.replaceAll("'", "\\'")
  if (file.endsWith('.tla')) return new RegExp(`^${plain}\\s*(?:\\([^)]*\\))?\\s*==`, 'm').test(text)
  if (file.endsWith('.json')) return text.includes(`"${last}"`)
  return new RegExp(`\\b(?:fn|def|func|fun|class|mod|struct|test|it|describe|async fn|async def)\\s+${plain}\\b`).test(text)
    || text.includes(`'${escaped}'`) || text.includes(`"${escaped}"`) || text.includes(`\`${escaped}\``)
}

// The `path::test` and `path:line-line` references of one evidence cell; a bare
// `::name` is the test last file named in the same cell. A span inside
// parentheses is a JavaScript test title only when the last file was one, and
// never a path of its own — the register writes fixtures that way.
function evidenceOf(text) {
  const found = []
  let last = null
  let depth = 0
  const source = String(text ?? '')
  for (let at = 0; at < source.length; at++) {
    const char = source[at]
    if (char === '（' || char === '(') { depth++; continue }
    if (char === '）' || char === ')') { depth = Math.max(0, depth - 1); continue }
    if (char !== '`') continue
    const end = source.indexOf('`', at + 1)
    if (end < 0) break
    const token = source.slice(at + 1, end).trim()
    at = end
    if (depth > 0) {
      if (last && /\.(mjs|ts|js)$/.test(last) && token && !token.includes('`')) found.push({ token, file: last, name: token, from: null, to: null })
      continue
    }
    const bare = /^::((?:[A-Za-z0-9_]+::)*[A-Za-z0-9_]+)$/.exec(token)
    if (bare && last) { found.push({ token, file: last, name: bare[1], from: null, to: null }); continue }
    const parsed = /^([^:]+?\.(?:rs|mjs|ts|js|py|go|swift|kt|yml))(?:(?:::)((?:[A-Za-z0-9_]+::)*[A-Za-z0-9_]+)|:(\d+)(?:-(\d+))?)?$/.exec(token)
    if (!parsed) continue
    last = locate(parsed[1])
    found.push({
      token,
      file: last,
      name: parsed[2],
      from: parsed[3] ? Number(parsed[3]) : null,
      to: parsed[4] ? Number(parsed[4]) : parsed[3] ? Number(parsed[3]) : null,
    })
  }
  return found
}

const problems = []
const say = line => problems.push(line)

// The evidence of one row: the file, the test name and the lines.
function evidence(row, counts) {
  for (const item of evidenceOf(row.证据)) {
    counts.references++
    if (!exists(item.file)) {
      say(`${row.file}:${row.line}: ${row.id}: ${item.token}: ${item.file} does not exist`)
      counts.missing++
      continue
    }
    if (item.name) {
      counts.names++
      if (!defines(item.file, item.name)) {
        say(`${file}:${row.line}: ${row.id}: ${item.token}: ${item.file} has no ${item.name}`)
        counts.namesMissing++
      }
    }
    if (item.to !== null) {
      counts.ranges++
      const total = lineCount(item.file)
      if (item.to > total) {
        say(`${file}:${row.line}: ${row.id}: ${item.token}: ${item.file} has ${total} lines, not ${item.to}`)
        counts.rangesMissing++
      }
    }
  }
}

// The evidence a protocol row names. A qualified path that does not resolve
// is a problem; a bare name the issue uses as shorthand is counted, not
// failed. A `::name` is checked when it is a plain test name (the issue also
// writes brace and wildcard sets).
function protocolEvidence(row, counts, file = row.file) {
  for (const item of protocolEvidenceOf(row.现有形式化)) {
    counts.references++
    if (item.kind === 'missing') {
      if (item.qualified) {
        say(`${file}:${row.line}: ${row.id}: ${item.token}: ${item.path} does not exist`)
        counts.missing++
      } else counts.unresolved++
      continue
    }
    if (item.kind === 'ambiguous') { counts.ambiguous++; continue }
    counts.found++
    if (item.name && !/[*{},]/.test(item.name)) {
      counts.names++
      if (!defines(item.file, item.name)) {
        say(`${file}:${row.line}: ${row.id}: ${item.token}: ${item.file} has no ${item.name}`)
        counts.namesMissing++
      }
    }
    if (item.to !== null) {
      counts.ranges++
      const total = lineCount(item.file)
      if (item.to > total) {
        say(`${file}:${row.line}: ${row.id}: ${item.token}: ${item.file} has ${total} lines, not ${item.to}`)
        counts.rangesMissing++
      }
    }
  }
}

// Every relative link of a Markdown file of this directory that does not
// resolve, as "file:line: target".
function links(file) {
  const broken = []
  let fence = null
  for (const [index, line] of read(file).split('\n').entries()) {
    const marker = /^\s*(`{3,}|~{3,})/.exec(line)
    if (marker) {
      if (!fence) fence = marker[1]
      else if (marker[1][0] === fence[0] && marker[1].length >= fence.length) fence = null
      continue
    }
    if (fence) continue
    for (const match of line.replace(/`[^`]*`/g, '').matchAll(/!?\[(?:[^\]\\]|\\.)*\]\(\s*(<[^>]*>|[^\s)]+)/g)) {
      const target = match[1].replace(/^<|>$/g, '')
      if (/^(?:[a-z][a-z0-9+.-]*:|#|\/)/i.test(target)) continue
      const path = target.replace(/[#?].*$/, '')
      if (!path) continue
      const resolved = posix.normalize(posix.join(posix.dirname(file), path)).replace(/\/$/, '')
      if (!exists(resolved)) broken.push(`${file}:${index + 1}: ${target}`)
    }
  }
  return broken
}

// The index table: rows per file, by status.
function index(rows) {
  const lines = [
    '| 文件 | 覆盖 | 条数 | 已验证 | 部分验证 | 未验证 | 明确不保证 | 已撤回 | 不单独计数 |',
    '| --- | --- | --- | --- | --- | --- | --- | --- | --- |',
  ]
  const counted = (file, title) => {
    const list = rows.filter(row => row.file === `${RULES}/${file}`)
    const of = status => list.filter(row => row.状态 === status).length
    lines.push(`| [${file}](rules/${file}) | ${title} | ${list.length} | ${of('已验证')} | ${of('部分验证')} | ${of('未验证')} | ${of('明确不保证')} | ${of('已撤回')} | ${of('不单独计数')} |`)
  }
  for (const [file, title] of FILES) counted(file, title)
  for (const file of HANDWRITTEN) counted(file, '手写')
  for (const meta of PROTOCOL) counted(meta.file, meta.label)
  const whole = status => rows.filter(row => row.状态 === status).length
  lines.push(`| 合计 | — | ${rows.length} | ${whole('已验证')} | ${whole('部分验证')} | ${whole('未验证')} | ${whole('明确不保证')} | ${whole('已撤回')} | ${whole('不单独计数')} |`)
  return lines.join('\n')
}

// ── The re-check page ───────────────────────────────────────────────────

const countBy = (list, key) => {
  const counted = new Map()
  for (const row of list) counted.set(key(row), (counted.get(key(row)) ?? 0) + 1)
  return counted
}

function recheckSummary(rows) {
  const lines = [
    '| 文件 | 分区 | 现行 | 已验证 | 部分验证 | 未验证 | 明确不保证 | 不单独计数 | 已撤回（快照） | 已撤回（本轮） | 有规格锚点 | 无现行来源 |',
    '| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |',
  ]
  const stat = list => {
    const current = list.filter(row => row.recheck.issueStatus !== '已取代')
    const of = status => current.filter(row => row.recheck.status === status).length
    return {
      current: current.length,
      verified: of('已验证'),
      partial: of('部分验证'),
      unverified: of('未验证'),
      nonGuarantee: of('明确不保证'),
      notCounted: of('不单独计数'),
      snapshot: list.filter(row => row.recheck.issueStatus === '已取代').length,
      withdrawn: current.filter(row => row.recheck.status === '已撤回').length,
      anchored: current.filter(row => row.recheck.anchors.length).length,
      uncovered: current.filter(row => !row.recheck.covered).length,
    }
  }
  for (const meta of PROTOCOL) {
    const s = stat(rows.filter(row => row.file === `${RULES}/${meta.file}`))
    lines.push(`| [${meta.file}](rules/${meta.file}) | ${meta.label} | ${s.current} | ${s.verified} | ${s.partial} | ${s.unverified} | ${s.nonGuarantee} | ${s.notCounted} | ${s.snapshot} | ${s.withdrawn} | ${s.anchored} | ${s.uncovered} |`)
  }
  const s = stat(rows)
  lines.push(`| 合计 | — | ${s.current} | ${s.verified} | ${s.partial} | ${s.unverified} | ${s.nonGuarantee} | ${s.notCounted} | ${s.snapshot} | ${s.withdrawn} | ${s.anchored} | ${s.uncovered} |`)
  return lines.join('\n')
}

function recheckChanges(rows) {
  const lines = [
    '| 快照映射状态 | 复核后状态 | 条数 |',
    '| --- | --- | --- |',
  ]
  const counted = new Map()
  for (const row of rows) {
    if (row.recheck.issueStatus === '已取代') continue
    const key = `${row.recheck.original} → ${row.recheck.status}`
    counted.set(key, (counted.get(key) ?? 0) + 1)
  }
  for (const [key, count] of [...counted].sort((a, b) => b[1] - a[1])) {
    const [from, to] = key.split(' → ')
    lines.push(`| ${from} | ${to} | ${count} |`)
  }
  return lines.join('\n')
}

function recheckTable(rows, meta) {
  const list = rows.filter(row => row.file === `${RULES}/${meta.file}`)
  const lines = [
    '| 编号 | 复核状态 | 快照映射状态 | 现行来源 | 现行锚点 | 证据 | 缺口 |',
    '| --- | --- | --- | --- | --- | --- | --- |',
  ]
  for (const row of list) {
    const source = row.recheck.source
    const withdrawn = row.recheck.status === '已撤回'
    const gap = withdrawn
      ? row.recheck.issueStatus === '已取代' ? 'issue 快照：已取代，不重抽' : recheckGap(row, row.recheck).join('；') || '—'
      : recheckGap(row, row.recheck).join('；') || '—'
    const fields = [
      row.id,
      row.recheck.status,
      row.recheck.original,
      row.recheck.issueStatus === '已取代' ? `${source.label}（issue 快照：已取代）` : source.label,
      withdrawn ? '—' : row.recheck.anchors.length ? row.recheck.anchors.join('、') : '—',
      withdrawn ? '—' : row.recheck.evidence.text,
      gap,
    ]
    lines.push(`| ${fields.map(cell).join(' | ')} |`)
  }
  return lines.join('\n')
}

function recheckUnmapped(rows) {
  const groups = new Map()
  for (const row of rows) {
    if (row.recheck.issueStatus === '已取代') continue
    if (row.recheck.covered) continue
    const key = row.recheck.unmapped || '未给理由'
    if (!groups.has(key)) groups.set(key, [])
    groups.get(key).push(row)
  }
  const lines = []
  for (const [why, list] of [...groups].sort((a, b) => b[1].length - a[1].length)) {
    const ids = list.map(row => row.id)
    lines.push(`- **${list.length} 条**：${why}`)
    lines.push('')
    lines.push(`  ${ids.map(id => `\`${id}\``).join('、')}`)
    lines.push('')
  }
  return lines.join('\n')
}

// ── The semantically re-extracted partitions ───────────────────────────
//
// Each partition is a file of rows plus its entries in
// `tools/formal-trace-semantic.mjs`. The Chinese re-check page gets a section
// per partition, the English page is generated whole; both come from this one
// join.

const STATUS_EN = new Map([
  ['已验证', 'verified'],
  ['部分验证', 'partial'],
  ['未验证', 'unverified'],
  ['明确不保证', 'not guaranteed'],
  ['已撤回', 'withdrawn'],
  ['不单独计数', 'not counted'],
])

const backticked = text => [...String(text ?? '').matchAll(/`([^`]+)`/g)].map(match => match[1])
// The evidence of a trace row is the paths and test names; the same cell also
// names code identifiers, which are not evidence.
const evidencePaths = text => backticked(text).filter(token => token.includes('/') || token.startsWith('::') || /\.(?:rs|mjs|ts|js|py|go|swift|kt)(?::|:|$)/.test(token))
const semanticSource = entry => [entry.source, entry.extra].filter(Boolean).join('；')
const semanticSourceEn = entry => [entry.sourceEn ?? entry.source, entry.extra].filter(Boolean).join('；')
// A rule no current document states at all: the row says so instead of naming a
// section, and it cannot be anchored (`--check` fails an anchor on it).
const NO_SOURCE = '0.22 无此条'

function semanticTableZh(list) {
  const lines = [
    '| 编号 | 状态 | 现行来源 | 现行锚点 | 证据 | 缺口 |',
    '| --- | --- | --- | --- | --- | --- |',
  ]
  for (const row of list) {
    const entry = row.semantic
    if (!entry) continue
    const anchors = entry.anchors.length ? entry.anchors.join('、') : `—（${entry.whyNoAnchor}）`
    lines.push(`| ${row.id} | ${row.状态} | ${cell(semanticSource(entry))} | ${cell(anchors)} | ${cell(evidencePaths(row.证据).join('、') || '—')} | ${cell(row.缺口)} |`)
  }
  return lines.join('\n')
}

// A registry row keeps the issue's rule wording; the table carries the four
// re-extracted things: source, register anchor, evidence and status with its
// reason.
function registryTableZh(list) {
  const lines = [
    '| 编号 | 状态 | 现行来源 | 现行锚点 | 证据 | 来源与状态说明 |',
    '| --- | --- | --- | --- | --- | --- |',
  ]
  for (const row of list) {
    const entry = row.semantic
    if (!entry) continue
    const anchors = entry.anchors.length ? entry.anchors.join('、') : `—（${entry.whyNoAnchor}）`
    lines.push(`| ${row.id} | ${row.状态} | ${cell(semanticSource(entry))} | ${cell(anchors)} | ${cell(entry.evidence)} | ${cell(entry.why)} |`)
  }
  return lines.join('\n')
}

function englishRecheckPage(lists) {
  const status = code => STATUS_EN.get(code) ?? code
  const all = lists.flatMap(list => list.rows).filter(row => row.semantic)
  const trace = lists.find(list => list.partition.translate)
  const registries = lists.filter(list => !list.partition.translate)
  const protocolCurrent = PROTOCOL.reduce((total, meta) => total + meta.current, 0)
  const protocolReplaced = PROTOCOL.reduce((total, meta) => total + meta.replaced, 0)
  const reExtractedCurrent = registries.reduce((total, list) => total + list.rows.filter(row => row.issueStatus !== '已取代').length, 0)
  const reExtractedReplaced = registries.reduce((total, list) => total + list.rows.filter(row => row.issueStatus === '已取代').length, 0)
  const sources = Object.entries(SEMANTIC_DOCS).map(([name, document]) => {
    let sections
    if (name === 'protocol-session.md') {
      const ids = [...new Set(all.flatMap(row => row.semantic.anchors))].filter(id => id.startsWith('SESS-'))
      ids.sort((a, b) => Number(a.slice('SESS-'.length)) - Number(b.slice('SESS-'.length)))
      sections = [`§11: ${ids.join(', ')}`]
    } else {
      sections = [...new Set(all.flatMap(row => [row.semantic.source, row.semantic.extra])
        .filter(source => source?.startsWith(`${name} `))
        .map(source => source.slice(name.length + 1)))]
      const order = section => Number(/^§(\d+(?:\.\d+)?)/.exec(section)?.[1] ?? Number.MAX_SAFE_INTEGER)
      sections.sort((a, b) => order(a) - order(b))
    }
    return `| ${document.labelEn} | [${document.doc.replace(/^docs\//, '')}](../${document.doc.replace(/^docs\//, '')}) | ${document.kind} | ${sections.join(', ') || '—'} |`
  })
  const traceRows = trace ? trace.rows.filter(row => row.semantic).map(row => {
    const entry = row.semantic
    const anchors = entry.anchors.length ? entry.anchors.join(', ') : `— (${entry.whyNoAnchorEn})`
    const evidence = evidencePaths(row.证据).join(', ') || '—'
    return `| ${row.id} | ${status(row.状态)} | ${cell(semanticSourceEn(entry))} | ${cell(anchors)} | ${cell(entry.en)} | ${cell(evidence)} | ${cell(entry.gapEn)} |`
  }) : []
  const joinList = items => (items.length <= 1 ? items.join('') : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`)
  const registrySections = registries.map((list, at) => {
    const rows = list.rows.filter(row => row.semantic).map(row => {
      const entry = row.semantic
      const anchors = entry.anchors.length ? entry.anchors.join(', ') : `— (${entry.whyNoAnchorEn})`
      return `| ${row.id} | ${status(row.状态)} | ${cell(semanticSourceEn(entry))} | ${cell(anchors)} | ${cell(entry.evidence)} | ${cell(entry.whyEn)} |`
    })
    const file = list.partition.file.replace(/^docs\/formal-trace\/rules\//, '')
    const intro = list.partition.introEn ?? `The rule wording of these ${list.rows.length} rows is the issue snapshot's (Chinese, \`rules/${file}\`); it was not rewritten. Each row was re-extracted on four points: the guarantee-register row that is its current source, that row as the anchor, the current evidence for the split claim, and the status with its reason.`
    return `## ${5 + at}. Per-row re-extraction: protocol list \`${list.partition.id}\`

${intro}

| Id | Status | Current source | Anchor | Evidence | Status and source |
| --- | --- | --- | --- | --- | --- |
${rows.join('\n')}`
  })
  const stillSection = 5 + registries.length
  const remaining = PROTOCOL.filter(meta => !lists.some(list => list.partition.file === `${RULES}/${meta.file}`)).map(meta => meta.partition)
  const partitionCounts = lists.map(list => {
    const count = code => list.rows.filter(row => row.状态 === code).length
    return `| ${list.partition.labelEn} | ${list.rows.length} | ${count('已验证')} | ${count('部分验证')} | ${count('未验证')} | ${count('明确不保证')} | ${count('已撤回')} | ${count('不单独计数')} |`
  })
  return `# Rule re-check against the 0.22 sources (#634)

[中文](recheck-0.22.md) · [Guarantee register](../guarantees.en.md) · [Session protocol specification](../protocol-session.md) · [Session trace contract](../formal-trace.en.md) · Issue: [#634](https://github.com/arcships/rutis/issues/634)

Status: first version. Date: ${SEMANTIC_PAGE.date}. Baseline: \`origin/main\` \`${SEMANTIC_PAGE.baseline}\`.

Generated by \`node tools/formal-trace.mjs --write\`; \`--check\` compares this page with ${lists.map(list => `\`${list.partition.file.replace(/^docs\/formal-trace\//, '')}\``).join(', ')} and the data in \`tools/formal-trace-semantic.mjs\`. ${1 + registries.length} partitions have been re-extracted semantically: the ${trace ? trace.rows.length : 0} rows of the session trace contract (\`TRC-*\`), rewritten whole, and the ${joinList(registries.map(list => String(list.rows.length)))} rows of the protocol list${registries.length === 1 ? '' : 's'} ${registries.map(list => `\`${list.partition.id}\``).join(', ')}, where the rule wording is still the issue snapshot's and the source, anchor, evidence and status are re-extracted. The other protocol partitions (${remaining.join(', ')}) were re-checked for source, anchor, evidence and status; their rule text has not been re-extracted, and their per-rule tables are in the Chinese page.

## 1. Method

The protocol rules pass four checks, and the trace rows pass a fifth. Every check that can be automated is in the generated pages; the judgement is in \`tools/formal-trace-recheck.mjs\` and \`tools/formal-trace-semantic.mjs\`.

1. **The source still exists**: the source document is in the current tree and 0.19 / 0.22 did not remove what it describes. Removed mechanisms (compat format, \`events-v2\`, the \`sync-wait\` capability, node \`export\` / \`import\`, \`Registration\`, the access point, \`Revocations\`, \`PeerRows\`, \`peer:\` rows, \`runtimes.remote\`, \`Transport\`, \`ChannelInfo\`) are \`withdrawn\`, and the gap names the rule that replaced them.
2. **A current anchor**: session rules map onto the \`SESS-*\` ids of \`docs/protocol-session.md\` §11; a registry row maps onto the guarantee-register row the issue split; an event rule maps onto the event protocol specification's own section or §15 decision, a \`SESS-*\` rule where the session specification carries it, or a register row; a runtime rule maps onto the guarantee-register row its 对应 column names, a \`SESS-*\` rule where the session specification states the synchronous wait, or a design section that is still the source; a remote rule maps onto the guarantee-register row its 对应 column names, a \`SESS-*\` rule where the session specification carries it, or a section of the remote-plugin, dial-out and app-integration designs or the connecting-nodes guide; a row with no anchor says why.
3. **The evidence resolves**: the paths, test names and line ranges of the evidence column are checked against the current tree; a rule with only a model, or without an implementation check, is \`partial\`.
4. **The status only moves down**: the issue snapshot is the ceiling. A rule is never raised to \`verified\` because a specification calls it current; current describes the rule, not its implementation evidence.
5. **The rule text is re-extracted** (trace rows only): the wording is the current source's, not the old design's, and the row names the \`SESS-*\` ids that carry it — or says why the session specification does not cover it.

## 2. Current sources of the re-extracted rows

| Source | File | Kind | Sections / ids |
| --- | --- | --- | --- |
${sources.join('\n')}

## 3. Result

| Partition | Rows | Verified | Partial | Unverified | Not guaranteed | Withdrawn | Not counted |
| --- | --- | --- | --- | --- | --- | --- | --- |
${partitionCounts.join('\n')}

No status moved because of the re-extraction itself in any partition: no test was added, the two trace rows that gained evidence citations (\`TRC-LIMIT-2\`, \`TRC-MALFORMED-2\`) already described that evidence in their wording, and each row of the protocol lists keeps the status the re-check gave it (the issue snapshot is the ceiling). \`TRC-GREET-3\` moved from \`unverified\` to \`not guaranteed\` after #1111 removed an unreachable check: no implementation offers a required-capabilities entry point. \`TRC-NG-4\` is withdrawn: protocol 4 no longer has the \`objects\` capability it described (#1123).

## 4. Per-row re-extraction: session trace contract

| Id | Status | Current source | Anchors | Rule (re-extracted) | Evidence | Gap |
| --- | --- | --- | --- | --- | --- | --- |
${traceRows.join('\n')}

${registrySections.join('\n\n')}

## ${stillSection}. What is still not re-extracted

${reExtractedCurrent === protocolCurrent ? `- No current protocol rule is left out: all ${protocolCurrent} current rows and all ${protocolReplaced} replaced rows of \`rules/protocol-*.md\` have an entry here. What the entries do not do is rewrite the wording — it stays the issue snapshot's, and the tables above do not translate it.` : `- The ${protocolCurrent - reExtractedCurrent} current protocol rules and the ${protocolReplaced - reExtractedReplaced} replaced rows of \`rules/protocol-*.md\`, except ${joinList(registries.map(list => `\`${list.partition.id}\``))}: the re-check maps them onto sources, anchors, evidence and status; it does not rewrite them. Their per-rule table is Chinese only.
- The rule wording of the ${joinList(registries.map(list => `${list.rows.length} \`${list.partition.id}\``))} rows: source, anchor, evidence and status are re-extracted, the wording is the issue snapshot's, and the tables above do not translate it.`}
- The ${registerRows.length} guarantee-register rows: generated from \`docs/guarantees.md\`, so the register's wording is the source, not a re-extraction.
- The 10 \`ECA-*\` rows: hand-written proposals of the event-close admission design, with no current specification behind them yet.
${remaining.length ? '- The English table of the remaining protocol partitions: a gap (#634).\n' : ''}`
}

function refreshSemantic(lists) {
  const content = englishRecheckPage(lists)
  if (!exists(SEMANTIC_PAGE.pageEn) || read(SEMANTIC_PAGE.pageEn) !== content) writeFileSync(`${root}${SEMANTIC_PAGE.pageEn}`, content)
}

function recheckPage(rows, lists) {
  const total = rows.filter(row => row.recheck.issueStatus !== '已取代').length
  const withdrawnNow = rows.filter(row => row.recheck.issueStatus !== '已取代' && row.recheck.status === '已撤回').length
  const sources = []
  for (const [prefix, source] of Object.entries(SOURCES)) {
    sources.push(`| \`${prefix}\` | ${source.label} | [${source.doc.replace(/^docs\//, '')}](../${source.doc.replace(/^docs\//, '')}) | ${source.kind === 'spec' ? '规格' : source.kind === 'design' ? '设计' : source.kind === 'guide' ? '指南' : '历史'} | ${source.note} |`)
  }
  const semantic = lists.flatMap(list => list.rows)
  const traceList = lists.find(list => list.partition.translate)
  const registryLists = lists.filter(list => !list.partition.translate)
  const semanticSections = lists.map(list => {
    const table = list.partition.translate ? semanticTableZh(list.rows) : registryTableZh(list.rows)
    const intro = list.partition.translate
      ? `\`TRC-*\` 的 ${list.rows.length} 行已按 0.22 现行来源重写：每行给出会话规格 §11 的 \`SESS-*\` 锚点（没有锚点的行写明会话规格不覆盖的部分），承诺文字改用现行规格的措辞与数字，来源章节与英文重抽文字在[英文复核页](recheck-0.22.en.md)。锚点、来源章节与英文页由 \`--check\` 核对：每个 \`SESS-*\` 都在会话规格里，并出现在该行「现行来源」。\`TRC-GREET-3\` 因 #1111 改记「明确不保证」，\`TRC-NG-4\` 已撤回：协议 4 没有它所描述的 \`objects\` 能力（#1123）。`
      : list.partition.introZh ?? `这些行是保证登记表的行被 #634 拆出的要求，规则文字仍是 issue 快照原文（\`rules/${list.partition.file.replace(/^docs\/formal-trace\/rules\//, '')}\`，中文），没有重写。每行重抽四件事：现行来源（登记表的哪一节）、现行锚点（登记表的哪一行）、承载这条拆分要求的现行证据、以及状态与理由。\`--check\` 核对：登记表行在它所在的节里、证据路径与用例名在当前树里、状态与复核结论一致。`
    return `### ${list.partition.label}（${list.rows.length} 行）\n\n${intro}\n\n${table}`
  })
  const semanticCurrent = registryLists.reduce((total, list) => total + list.rows.filter(row => row.issueStatus !== '已取代').length, 0)
  const semanticReplaced = registryLists.reduce((total, list) => total + list.rows.filter(row => row.issueStatus === '已取代').length, 0)
  return `# 规则复核：按 0.22 现行规格（#634）

[中文索引](README.md) · [英文复核页](recheck-0.22.en.md) · [保证登记表](../guarantees.md) · [会话协议规格](../protocol-session.md) · 议题：[#634](https://github.com/arcships/rutis/issues/634)

状态：第一版。日期：${SEMANTIC_PAGE.date}。基准：\`origin/main\` \`${SEMANTIC_PAGE.baseline}\`。

本文件由 \`node tools/formal-trace.mjs --write\` 生成，\`--check\` 检查它是否与 \`rules/protocol-*.md\` 及现行来源一致。它复核的是 \`rules/protocol-*.md\` 里的 ${total} 条现行规则（另 ${rows.filter(row => row.issueStatus === '已取代').length} 条 issue 快照里已取代的规则不重抽）。**状态是这一轮复核的结论；\`rules/protocol-*.md\` 保持 issue 的十列原文不改。**

## 1. 复核口径

每条规则只看四件事，能自动核对的都写进本文件，判断留在数据里（\`tools/formal-trace-recheck.mjs\`）：

1. **来源是否还在**：来源文档在当前树存在，且 0.19 / 0.22 没有删掉它描述的东西。删掉的（compat 格式、\`events-v2\`、\`sync-wait\` 能力、节点 \`export\` / \`import\`、\`Registration\`、接入点、\`Revocations\`、\`PeerRows\`、\`peer:\` 行、\`runtimes.remote\`、\`Transport\`、\`ChannelInfo\`）记「已撤回」，并在缺口写取代它的规则。
2. **有没有现行锚点**：会话层规则对到 \`docs/protocol-session.md\` 的 \`SESS-*\` 编号（§11 规则状态表）；其余分区没有新的现行规格，只有设计或指南，锚点写 \`—\`，并在缺口写明它落在哪份文档或为什么没有。
3. **证据是否解析**：用 \`现有形式化\` 列里的具名路径、用例名与行号核对当前树；解析不了的降级，只有模型或没有实现检查的记「部分验证」。
4. **状态只降不升**：以 issue 的十列映射为上限；本轮只把来源被删、登记表删行、证据不解析或来源已历史化的行降级。**没有一条因为“规格说是当前”就升为「已验证」**：规格的「当前」说的是规则现行，不是实现验证过。

复核不判断规则文字的语义是否正确：它核对来源、锚点、证据路径和状态。**TRC 分区的 ${traceList ? traceList.rows.length : 0} 行与协议清单 ${registryLists.map(list => `\`${list.partition.id}\` 的 ${list.rows.length} 行`).join('、')}已在 §5 语义重抽**${semanticCurrent === total ? '：协议清单全部现行规则都有条目，规则文字仍是 issue 快照原文' : `；协议清单其余 ${total - semanticCurrent} 条现行规则仍是 issue 快照原文，逐条语义重抽尚未完成`}。

## 2. 现行来源（0.22）

| 前缀 | 现行来源 | 文件 | 类别 | 说明 |
| --- | --- | --- | --- | --- |
${sources.join('\n')}

登记表分区（\`registry-1\`、\`registry-2\`）的规则是保证登记表的行：现行来源就是登记表当前那一行；登记表 0.22 已删的行记「已撤回」。\`session\` 分区的行按四种现行来源重抽：会话规格 §11 的 \`SESS-*\` 规则、保证登记表的行、仍现行的设计（多语言 M1、网络栈、值类型、会话标签、事件协议、远程会话、挂载），以及写明「\`0.22 无此条\`」的行（机制已删、没有替代规则）。\`runtime\` 分区同样按这四种：\`对应\` 列指到的登记表行、会话规格里写同步等待的 \`SESS-38\`/\`SESS-39\`/\`SESS-40\`、仍现行的设计（挂载、需求、Cordis 运行时插件、实例服务、Rust 服务对外、多语言 M1/M2），以及「\`0.22 无此条\`」。

## 3. 结果

<!-- recheck-index:begin -->
${recheckSummary(rows)}
<!-- recheck-index:end -->

状态怎么变：

${recheckChanges(rows)}

## 4. 逐条复核

${PROTOCOL.map(meta => `### ${meta.label}\n\n${recheckTable(rows, meta)}`).join('\n\n')}

## 5. 语义重抽（${semantic.length} 行，${SEMANTIC_PAGE.date}，基准 \`${SEMANTIC_PAGE.baseline}\`）

${1 + registryLists.length} 个分区已按 0.22 现行来源重抽（TRC、${registryLists.map(list => `\`${list.partition.id}\``).join('、')}）：协议清单 ${total} 条现行规则与 ${semanticReplaced} 条 issue 快照已取代行都有条目（状态按本页复核，规则文字仍是 issue 快照原文）。保证登记表 ${registerRows.length} 行由登记表生成，\`ECA\` 10 行仍是手写的设计提案。

${semanticSections.join('\n\n')}

## 6. 没有现行锚点的规则

这些规则在 0.22 没有可引用的现行规格：来源是旧设计、设计说未实现、或会话规格写明不覆盖。它们保持原状态或降级，不当作已完成复核；下一轮要么写出对应的现行规格，要么把它们登记为明确不保证。

${recheckUnmapped(rows)}

## 7. 这一轮改了什么、还剩什么

- **已撤回**：issue 快照的 184 条之外，本轮按 0.19 / 0.22 删除的机制追加了 ${withdrawnNow} 条（见 §3 的「已撤回（本轮）」列与 §4 的缺口）。它们仍在 \`rules/protocol-*.md\` 里，统计不计入。
- **未完成**：没有一行重写规则文字——协议清单的 ${total} 条现行规则仍是 issue 快照原文；远程没有对应的新规格，只有设计稿与指南（0.19 之后的连接行为在 \`design-remote-session-2026-10-28.md\`）。§6 按原因列出没有锚点的规则；写规格是 #635 的后续，不是这一轮能补的。
- **证据**：只核对 \`现有形式化\` 列的具名引用；备注列里的旧路径（如 \`crates/rutis-bridge/src/cordis/server.rs\`）没有逐条清，重抽时一并处理。
- **语义**：TRC 分区（规则文字整体重写）与协议清单 ${registryLists.map(list => `\`${list.partition.id}\``).join('、')}（来源、锚点、证据、状态）已在 §5 全部重抽，协议清单没有剩下的分区；规则文字仍是 issue 快照原文。规则的「已撤回 / 明确不保证 / 未验证 / 部分验证 / 已验证」按上面的四件事定，不按记忆。
`
}

function refreshRecheck(rows, lists) {
  const content = recheckPage(rows, lists)
  if (!exists(RECHECK) || read(RECHECK) !== content) writeFileSync(`${root}${RECHECK}`, content)
}

function indexBlock(text) {
  const begin = text.indexOf(INDEX_BEGIN)
  const end = text.indexOf(INDEX_END)
  if (begin < 0 || end < begin) return null
  return text.slice(begin + INDEX_BEGIN.length, end).replace(/^\n+|\n+$/g, '')
}

function refreshIndex(rows) {
  if (!exists(INDEX)) return
  const text = read(INDEX)
  const block = indexBlock(text)
  if (block === null) return
  const begin = text.indexOf(INDEX_BEGIN) + INDEX_BEGIN.length
  const end = text.indexOf(INDEX_END)
  const wanted = `${text.slice(0, begin)}\n${index(rows)}\n${text.slice(end)}`
  if (wanted !== text) writeFileSync(`${root}${INDEX}`, wanted)
}

const counts = { references: 0, missing: 0, names: 0, namesMissing: 0, ranges: 0, rangesMissing: 0 }
const protocolCounts = { references: 0, found: 0, unresolved: 0, ambiguous: 0, missing: 0, names: 0, namesMissing: 0, ranges: 0, rangesMissing: 0 }

if (process.argv.includes('--write')) write()

const rows = rules()
const registerRows = register()
const protocol = protocolRows()
const allRows = [...rows, ...protocol]
const byId = new Map(rows.map(row => [row.id, row]))
const anchorIds = new Set([...read(SESSION_SPEC).matchAll(/`(SESS-\d+)`/g)].map(match => match[1]))

// 0. The literals the data modules and this file share must agree. Nothing is
//    derived in a second copy: the family list comes from the re-check module,
//    the source keys from the same module, and the semantic table from its own
//    module. These checks catch a family or prefix that one side gained and the
//    other did not, and a source or section that no longer exists.
const registerFamilies = new Set(registerRows.map(row => row.id.split('/')[0]))
for (const family of registerFamilies) if (!REGISTRY_FAMILIES.includes(family)) say(`${REGISTER}: family ${family} is not in REGISTRY_FAMILIES (tools/formal-trace-recheck.mjs)`)
for (const family of REGISTRY_FAMILIES) if (!registerFamilies.has(family)) say(`${REGISTER}: REGISTRY_FAMILIES names ${family}, the register has no such family`)
const fileFamilies = new Set(FILES.flatMap(([, , families]) => families))
for (const family of REGISTRY_FAMILIES) if (!fileFamilies.has(family)) say(`${RULES}: REGISTRY_FAMILIES names ${family}, no generated file covers it`)
for (const family of fileFamilies) if (!REGISTRY_FAMILIES.includes(family)) say(`${RULES}: a generated file covers ${family}, REGISTRY_FAMILIES does not name it`)
const sourcePrefixes = new Set(Object.keys(SOURCES))
for (const prefix of PROTOCOL_PREFIXES) if (!sourcePrefixes.has(prefix)) say(`${RECHECK}: prefix ${prefix} has no SOURCES entry`)
for (const prefix of sourcePrefixes) if (!PROTOCOL_PREFIXES.includes(prefix)) say(`${RECHECK}: SOURCES names ${prefix}, PROTOCOL_PREFIXES does not`)
for (const [prefix, source] of Object.entries(SOURCES)) {
  if (!exists(source.doc)) say(`${RECHECK}: ${prefix}: ${source.doc} does not exist`)
  if (!['spec', 'design', 'guide', 'historical'].includes(source.kind)) say(`${RECHECK}: ${prefix}: kind ${source.kind} is not spec, design, guide or historical`)
}

// 0b. The semantically re-extracted partitions: one entry per row, the anchors
//     exist and the row names them, the source sections exist, a row without an
//     anchor says why, a registry entry's status agrees with the re-check, and
//     its evidence resolves against the current tree.
// A section of a source document, as the semantic entries cite it: a heading
// (`§4.2`, `§三`), a heading below a chapter (`§三.1` -> `## 三、…` then
// `### 1. …`), or a numbered decision in a chapter (`§15.28` -> `## 十五、…`
// then `28. …`), which is how the event protocol numbers its decisions.
const NUMERALS = ['', '一', '二', '三', '四', '五', '六', '七', '八', '九', '十', '十一', '十二', '十三', '十四', '十五', '十六', '十七', '十八', '十九', '二十']
const chapterBody = (doc, chapter) => {
  const head = new RegExp(`^## ${chapter}[ 、]`, 'm').exec(read(doc))
  if (!head) return null
  const rest = read(doc).slice(head.index + head[0].length)
  const next = rest.search(/^## /m)
  return next < 0 ? rest : rest.slice(0, next)
}
const sectionExists = (doc, section) => {
  const plain = String(section ?? '').replace(/^§/, '')
  const escaped = plain.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  if (new RegExp(`^#{2,4} ${escaped}(?:[ 、]|\\.\\s|$)`, 'm').test(read(doc))) return true
  const nested = /^([一二三四五六七八九十]+|[1-9]\d?)\.(\d+)$/.exec(plain)
  if (!nested) return false
  const chapter = /^\d+$/.test(nested[1]) ? NUMERALS[Number(nested[1])] ?? nested[1] : nested[1]
  const body = chapterBody(doc, chapter)
  if (!body) return false
  return new RegExp(`^#{3,4} ${nested[2]}\\. `, 'm').test(body) || new RegExp(`^${nested[2]}\\. `, 'm').test(body)
}
const sourceProblem = (id, source) => {
  const [name, ...rest] = String(source ?? '').split(' ')
  const section = rest.join(' ')
  const document = SEMANTIC_DOCS[name]
  if (!document) return `${id}: source ${source} names no file of SEMANTIC_DOCS`
  if (!exists(document.doc)) return `${id}: ${document.doc} does not exist`
  if (!section) return `${id}: source ${source} names no section`
  return sectionExists(document.doc, section) ? null : `${id}: ${document.doc} has no section ${section}`
}
const sectionOf = source => /§(\S+)$/.exec(String(source ?? ''))?.[1] ?? ''
const registerSections = new Map(registerRows.map(row => [row.id, row.section]))
for (const document of Object.values(SEMANTIC_DOCS)) if (!exists(document.doc)) say(`${SEMANTIC_PAGE.pageEn}: ${document.doc} does not exist`)
const semanticLists = SEMANTIC_PARTITIONS.map(partition => ({
  partition,
  rows: allRows.filter(row => row.file === partition.file).map(row => ({ ...row, semantic: partition.rows[row.id] })),
}))
const semanticsOf = partition => semanticLists.find(list => list.partition === partition)?.rows ?? []
for (const { partition, rows: list } of semanticLists) {
  for (const row of list) {
    const entry = row.semantic
    const at = `${row.file}:${row.line}: ${row.id}`
    if (!entry) { say(`${at}: no semantic entry in tools/formal-trace-semantic.mjs`); continue }
    if (partition.translate) {
      if (!entry.en) say(`${at}: the semantic entry has no English rule`)
      if (!entry.gapEn) say(`${at}: the semantic entry has no English gap`)
    } else {
      if (!entry.evidence) say(`${at}: the semantic entry has no evidence`)
      if (!entry.why) say(`${at}: the semantic entry has no why`)
      if (!entry.whyEn) say(`${at}: the semantic entry has no whyEn`)
      if (!STATUSES.includes(entry.status)) say(`${at}: semantic status ${entry.status} is not one of ${STATUSES.join(', ')}`)
      else if (entry.status !== row.状态) say(`${at}: semantic status ${entry.status} differs from the re-checked status ${row.状态}`)
      for (const token of backticked(entry.evidence)) {
        const [file, name] = token.split('::')
        if (!/\.(?:rs|mjs|ts|js|py|go|swift|kt|json|tla)$/.test(file)) continue
        const path = locate(file)
        if (!exists(path)) { say(`${at}: evidence ${token}: ${path} does not exist`); continue }
        if (name && !defines(path, name)) say(`${at}: evidence ${token}: ${path} does not define ${name}`)
      }
    }
    if (!entry.anchors.length && !entry.whyNoAnchor) say(`${at}: no anchors and no whyNoAnchor`)
    if (!entry.anchors.length && !entry.whyNoAnchorEn) say(`${at}: no anchors and no whyNoAnchorEn`)
    if (!['session', 'register', 'mixed', 'events'].includes(partition.anchorKind)) say(`${at}: anchorKind ${partition.anchorKind} is not session, register, mixed or events`)
    // A session partition mixes both: a `SESS-*` id anchors on the session
    // specification, any other id on a guarantee-register row. The events
    // partition adds the event protocol's own sections and decisions.
    const sessionAnchor = anchor => partition.anchorKind === 'session'
      || (['mixed', 'events'].includes(partition.anchorKind) && /^SESS-\d+$/.test(anchor))
    const eventAnchor = anchor => partition.anchorKind === 'events' && /^(?:EVP|EVD) §/.test(anchor)
    for (const anchor of entry.anchors) {
      if (sessionAnchor(anchor)) {
        if (!anchorIds.has(anchor)) say(`${at}: anchor ${anchor} is not in ${SESSION_SPEC}`)
        if (partition.translate && !String(row.现行来源).includes(anchor)) say(`${at}: ${anchor} is not named in 现行来源`)
      } else if (eventAnchor(anchor)) {
        const [prefix, ...rest] = anchor.split(' ')
        const document = SEMANTIC_DOCS[EVENT_DOCS[prefix]]
        const section = rest.join(' ')
        if (!document) say(`${at}: anchor ${anchor}: ${prefix} names no document`)
        else if (!sectionExists(document.doc, section)) say(`${at}: anchor ${anchor}: ${document.doc} has no section ${section}`)
      } else {
        if (!registerSections.has(anchor)) { say(`${at}: anchor ${anchor} is not a row of ${REGISTER}`); continue }
        const section = registerSections.get(anchor)
        const wanted = sectionOf(entry.source)
        if (!wanted) {
          if (partition.anchorKind !== 'events') say(`${at}: anchor ${anchor} is a register row; the entry's source is ${entry.source}`)
        } else if (partition.anchorKind !== 'events' || String(entry.source).startsWith(`${REGISTER} `)) {
          if (section !== wanted) say(`${at}: anchor ${anchor} sits under §${section}, not §${wanted}`)
        }
      }
    }
    if (partition.anchorKind === 'register' && entry.anchors.length) {
      const base = registerBase(row.id)
      if (base && !entry.anchors.includes(base.id)) say(`${at}: the register row is ${base.id}; the entry does not anchor on it`)
    }
    if (!partition.translate && entry.source === NO_SOURCE) {
      if (!entry.sourceEn) say(`${at}: a row with no current source needs sourceEn`)
      if (entry.anchors.length) say(`${at}: a row with no current source cannot be anchored`)
      if (entry.extra) say(`${at}: a row with no current source cannot name a second source`)
    }
    for (const source of [entry.source, entry.extra].filter(Boolean)) {
      if (source === NO_SOURCE) continue
      const problem = sourceProblem(row.id, source)
      if (problem) say(`${row.file}:${row.line}: ${problem}`)
    }
  }
  for (const id of Object.keys(partition.rows)) {
    if (!list.some(row => row.id === id)) say(`${partition.file}: the semantic entry ${id} has no row`)
  }
}

// 1. Every register row is in the table once, with the register's columns.
for (const registerRow of registerRows) {
  const row = byId.get(registerRow.id)
  if (!row) { say(`${REGISTER}:${registerRow.line}: ${registerRow.id}: no row in ${RULES}/`); continue }
  for (const [column, text] of Object.entries(automatic(registerRow))) {
    if (row[column] !== cell(text)) say(`${row.file}:${row.line}: ${registerRow.id}: ${column} differs from the register (${REGISTER}:${registerRow.line})`)
  }
}

// 2. Every row is a register row or a trace row, ids are unique, the words are
//    in the vocabulary.
const registerIds = new Set(registerRows.map(row => row.id))
const seen = new Set()
for (const row of rows) {
  if (seen.has(row.id)) say(`${row.file}:${row.line}: ${row.id}: a second row with this id`)
  seen.add(row.id)
  if (!registerIds.has(row.id) && !/^[A-Z]{2,6}(?:-[A-Z]{2,12})*-\d+$/.test(row.id)) {
    say(`${row.file}:${row.line}: ${row.id}: not a register row and not an id of the form TRC-…-1 or ECA-1`)
  }
  if (!STATUSES.includes(row.状态)) say(`${row.file}:${row.line}: ${row.id}: status ${row.状态 || '(empty)'} is not one of ${STATUSES.join(', ')}`)
  if (!LEVELS.includes(row.级别)) say(`${row.file}:${row.line}: ${row.id}: level ${row.级别 || '(empty)'} is not one of ${LEVELS.join(', ')}`)
}

// 3. The evidence resolves, every relative link of this directory resolves,
//    and the index is current.
for (const row of rows) evidence(row, counts)
const protocolFiles = PROTOCOL.map(meta => `${RULES}/${meta.file}`)
for (const file of [INDEX, RECHECK, SEMANTIC_PAGE.pageEn, 'docs/formal-trace/README.en.md', ...FILES.map(([name]) => `${RULES}/${name}`), ...HANDWRITTEN.map(name => `${RULES}/${name}`), ...protocolFiles]) {
  if (exists(file)) for (const broken of links(file)) say(`broken link ${broken}`)
}

// 4. The protocol rows: the exported columns and ids, the issue's status
//    words, #634's per-section totals, and the evidence they name.
const protocolSeen = new Map()
for (const meta of PROTOCOL) {
  const file = `${RULES}/${meta.file}`
  if (!exists(file)) { say(`${file}: missing protocol file`); continue }
  const text = read(file)
  const headerLine = text.split('\n').find(line => line.startsWith('| 编号 |'))
  if (!headerLine) { say(`${file}: no table with a 编号 column`); continue }
  const header = cellsOf(headerLine)
  if (header.length !== PROTOCOL_COLUMNS.length || header.some((name, at) => name !== PROTOCOL_COLUMNS[at])) {
    say(`${file}: the columns are ${header.join(' | ')}, not ${PROTOCOL_COLUMNS.join(' | ')}`)
  }
  const list = protocolTable(text)
  const idPattern = meta.id === 'registry'
    ? new RegExp(`^(?:${REGISTRY_FAMILIES.join('|')})/[A-Za-z0-9_.-]+(?: [a-z])?$`)
    : new RegExp(`^(?:${PROTOCOL_PREFIXES.join('|')})-\\d+(?:\\.\\d+)*-(?:[A-Z]?\\d+[a-z]?)$`)
  for (const row of list) {
    if (row.cells !== row.width) say(`${file}:${row.line}: ${row.id}: ${row.cells} cells, not ${row.width}`)
    for (const column of PROTOCOL_COLUMNS) if (!(column in row)) say(`${file}:${row.line}: ${row.id}: column ${column} missing`)
    if (protocolSeen.has(row.id)) say(`${file}:${row.line}: ${row.id}: a second protocol row with this id (first at ${protocolSeen.get(row.id)})`)
    protocolSeen.set(row.id, `${file}:${row.line}`)
    if (!idPattern.test(row.id)) say(`${file}:${row.line}: ${row.id}: not an id of the form ${meta.id === 'registry' ? '<family>/<name>' : '<PREFIX>-<section>-<number>'}`)
    if (!ISSUE_STATUSES.includes(row.状态)) say(`${file}:${row.line}: ${row.id}: issue status ${row.状态 || '(empty)'} is not one of ${ISSUE_STATUSES.join(', ')}`)
    const mapped = protocolStatus(row)
    if (!STATUSES.includes(mapped)) say(`${file}:${row.line}: ${row.id}: mapped status ${mapped} is not one of ${STATUSES.join(', ')}`)
    const checked = recheckStatus(row)
    if (!STATUSES.includes(checked)) say(`${file}:${row.line}: ${row.id}: re-checked status ${checked} is not one of ${STATUSES.join(', ')}`)
    protocolEvidence(row, protocolCounts, file)
  }
  const current = list.filter(row => row.状态 !== '已取代').length
  const replaced = list.filter(row => row.状态 === '已取代').length
  if (current !== meta.current || replaced !== meta.replaced) {
    say(`${file}: ${current} current and ${replaced} replaced rows; #634's summary table says ${meta.current} and ${meta.replaced}`)
  }
}
const protocolReplaced = protocol.filter(row => row.issueStatus === '已取代').length
const protocolCurrent = protocol.length - protocolReplaced
if (protocolCurrent !== 1368 || protocolReplaced !== 184) {
  say(`${RULES}: ${protocolCurrent} current and ${protocolReplaced} replaced protocol rows; #634 says 1368 and 184`)
}

// 5. The re-check page: every current rule once, the anchors exist in the
//    current specification, and no rule is listed as removed twice.
for (const [pattern, ids] of ANCHORS) {
  for (const id of ids) if (!anchorIds.has(id)) say(`${RECHECK}: anchor ${id} (${pattern}) is not in ${SESSION_SPEC}`)
}
const seenSuperseded = new Set()
for (const entry of SUPERSEDED) {
  if (!exists(entry.by)) say(`${RECHECK}: ${entry.by} does not exist`)
  for (const id of entry.ids) {
    if (seenSuperseded.has(id)) say(`${RECHECK}: ${id} is listed as removed more than once`)
    seenSuperseded.add(id)
  }
}
for (const [id, entry] of superseded) {
  if (!protocol.some(row => row.id === id)) say(`${RECHECK}: ${id} is listed as removed but is not a protocol row`)
  if (!exists(entry.by)) say(`${RECHECK}: ${id}: ${entry.by} does not exist`)
}
if (process.argv.includes('--write')) {
  refreshIndex(allRows)
  refreshRecheck(protocol, semanticLists)
  refreshSemantic(semanticLists)
} else if (exists(INDEX)) {
  const block = indexBlock(read(INDEX))
  if (block !== null && block !== index(allRows)) say(`${INDEX}: the index between the markers is stale; run node tools/formal-trace.mjs --write`)
}
if (exists(RECHECK)) {
  const wanted = recheckPage(protocol, semanticLists)
  if (read(RECHECK) !== wanted) say(`${RECHECK}: stale or not generated; run node tools/formal-trace.mjs --write`)
} else say(`${RECHECK}: missing; run node tools/formal-trace.mjs --write`)
if (exists(SEMANTIC_PAGE.pageEn)) {
  if (read(SEMANTIC_PAGE.pageEn) !== englishRecheckPage(semanticLists)) say(`${SEMANTIC_PAGE.pageEn}: stale or not generated; run node tools/formal-trace.mjs --write`)
} else say(`${SEMANTIC_PAGE.pageEn}: missing; run node tools/formal-trace.mjs --write`)

if (!process.argv.includes('--check')) {
  const tally = list => {
    const counted = new Map()
    for (const row of list) counted.set(row.状态, (counted.get(row.状态) ?? 0) + 1)
    return STATUSES.filter(status => counted.has(status)).map(status => `${status} ${counted.get(status)}`).join(' · ')
  }
  const issueTally = list => {
    const counted = new Map()
    for (const row of list) counted.set(row.issueStatus, (counted.get(row.issueStatus) ?? 0) + 1)
    return ISSUE_STATUSES.filter(status => counted.has(status)).map(status => `${status} ${counted.get(status)}`).join(' · ')
  }
  const protocolIds = new Set(protocol.map(row => row.id))
  const registerOverlap = [...protocolIds].filter(id => registerIds.has(id)).length
  console.log(`\nrows: ${allRows.length} (register ${registerRows.length}, hand-written ${rows.length - registerRows.length}, protocol ${protocol.length})\n`)
  console.log('by file:')
  for (const file of [...FILES.map(([name]) => name), ...HANDWRITTEN, ...PROTOCOL.map(meta => meta.file)]) {
    const list = allRows.filter(row => row.file === `${RULES}/${file}`)
    if (list.length) console.log(`  ${file}: ${list.length} — ${tally(list)}`)
  }
  console.log(`\nby status: ${tally(allRows)}`)
  console.log('\nby level:')
  for (const level of LEVELS) {
    const list = allRows.filter(row => row.级别 === level)
    console.log(`  ${level} ${list.length}（${tally(list) || '—'}）`)
  }
  const unverified = allRows.filter(row => row.级别 === '核心承诺' && row.状态 === '未验证').length
  const unverifiedRegister = registerRows.filter(row => row.level === '核心承诺' && status(row) === '未验证').length
  console.log(`\nunverified core commitments: ${unverified} (register rows: ${unverifiedRegister})`)
  const scoped = allRows.filter(row => row.适用范围 !== '未登记').length
  console.log(`scope filled: ${scoped} / ${allRows.length} (未登记 ${allRows.length - scoped})`)
  console.log(`evidence (register/trace): ${counts.references} references (names ${counts.names}, line ranges ${counts.ranges}); missing files ${counts.missing}, missing names ${counts.namesMissing}, lines beyond the file ${counts.rangesMissing}`)
  console.log(`evidence (protocol): ${protocolCounts.references} references (resolved ${protocolCounts.found}, unresolved shorthand ${protocolCounts.unresolved}, ambiguous ${protocolCounts.ambiguous}); missing explicit paths ${protocolCounts.missing}, missing names ${protocolCounts.namesMissing}, lines beyond the file ${protocolCounts.rangesMissing}`)
  console.log(`protocol issue status: ${issueTally(protocol)}`)
  const recheckedCurrent = protocol.filter(row => row.issueStatus !== '已取代')
  const recheckTally = new Map()
  for (const row of recheckedCurrent) recheckTally.set(row.recheck.status, (recheckTally.get(row.recheck.status) ?? 0) + 1)
  console.log(`protocol re-check (${recheckedCurrent.length} current): ${STATUSES.filter(status => recheckTally.has(status)).map(status => `${status} ${recheckTally.get(status)}`).join(' · ')}`)
  const withdrawnNow = recheckedCurrent.filter(row => row.recheck.status === '已撤回').length
  const uncovered = recheckedCurrent.filter(row => !row.recheck.covered).length
  const anchored = recheckedCurrent.filter(row => row.recheck.anchors.length).length
  console.log(`protocol re-check: withdrawn by 0.19/0.22 ${withdrawnNow} (issue snapshot already ${protocol.filter(row => row.issueStatus === '已取代').length}); session anchors ${anchored}, no current source ${uncovered}`)
  console.log(`protocol ids also in the register: ${registerOverlap} (kept as separate rows; the totals are rows, not distinct rules)`)
  for (const { partition, rows: list } of semanticLists) {
    const sessionAnchored = list.filter(row => row.semantic.anchors.some(anchor => /^SESS-\d+$/.test(anchor))).length
    const eventAnchored = list.filter(row => row.semantic.anchors.some(anchor => /^(?:EVP|EVD) §/.test(anchor))).length
    const registerAnchored = list.filter(row => row.semantic.anchors.some(anchor => !/^SESS-\d+$/.test(anchor) && !/^(?:EVP|EVD) §/.test(anchor))).length
    const noSource = list.filter(row => row.semantic.source === NO_SOURCE).length
    const without = list.filter(row => !row.semantic.anchors.length).length
    console.log(`semantic re-extraction: ${list.length} rows (${partition.label}), ${sessionAnchored} with SESS anchors, ${eventAnchored} with event-protocol anchors, ${registerAnchored} with register anchors, ${without} without (why named), ${noSource} with no current source`)
  }
  const reExtractedCurrent = SEMANTIC_PARTITIONS.filter(partition => !partition.translate)
    .reduce((total, partition) => total + semanticsOf(partition).filter(row => row.issueStatus !== '已取代').length, 0)
  console.log(`not semantically re-extracted: ${protocolCurrent - reExtractedCurrent} protocol rules, ${registerRows.length} register rows, 10 ECA rows\n`)
}

if (process.argv.includes('--check') && (protocolCounts.unresolved || protocolCounts.ambiguous)) {
  console.log(`protocol evidence: ${protocolCounts.unresolved} unresolved shorthand, ${protocolCounts.ambiguous} ambiguous (reported, not problems)`)
}
const unique = [...new Set(problems)]
for (const line of unique) console.log(line)
console.log(`${unique.length} problem(s)`)
process.exitCode = unique.length ? 1 : 0
