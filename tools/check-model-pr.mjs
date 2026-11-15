#!/usr/bin/env node
// A pull request that changes code a model in specs/ stands for either changes
// that model or says in its description why it does not (#635):
//
//   模型：specs/events 不涉及（只改了错误文字）
//
//   node tools/check-model-pr.mjs --body-file body.md --files-from-git BASE
//   node tools/check-model-pr.mjs --body-file body.md --files changed.txt
//   node tools/check-model-pr.mjs --self-test
//
// Rules:
// - a pull request that changes specs/** passes: the model is in the change;
// - one that changes none of the files the models stand for passes: the
//   documentation, the tooling, or code no model covers;
// - one that changes any of them needs a `模型：` line in its description,
//   saying 不涉及 and why.
//
// "The files the models stand for" are the paths written in the
// "对应的设计和代码" table of each specs/<dir>/README.md, so the list follows
// the models themselves and is not kept by hand here.

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const exists = (path) => existsSync(join(root, path));

// A path of the repository, not a bare file name of the model directory.
const ROOTS = ['crates/', 'node/', 'bun/', 'python/', 'go/', 'swift/', 'kotlin/', 'tests/', 'tools/', 'examples/'];
// The section of a model README that maps the model to the design and the code.
const SECTION = /^## +(对应的设计和代码|对应的实现|corresponding implementation|(the )?design and (the )?code.*)$/i;
// The line a description adds: `模型：<目录> 不涉及（理由）`.
const NOTE = /^模型\s*[：:]/;

// The paths the models stand for: the code spans of every specs/<dir>/README.md
// "对应的设计和代码" table that name a file of this repository.
export function modelledPaths() {
  const specs = join(root, 'specs');
  const paths = new Set();
  for (const entry of readdirSync(specs, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const readme = join(specs, entry.name, 'README.md');
    if (!existsSync(readme)) continue;
    let inside = false;
    for (const line of readFileSync(readme, 'utf8').split('\n')) {
      if (line.startsWith('## ')) {
        inside = SECTION.test(line);
        continue;
      }
      if (!inside) continue;
      for (const [, span] of line.matchAll(/`([^`]+)`/g)) {
        // `path:12`, `path:12-34`, `path:12-34,56`: the file, not the lines.
        const path = span.replace(/:\d[\d,\-]*$/, '');
        if (!ROOTS.some((prefix) => path.startsWith(prefix))) continue;
        if (!exists(path)) continue;
        paths.add(path);
      }
    }
  }
  return paths;
}

// Whether the change needs a `模型：` line, and why. `body` is the pull
// request description, `files` its changed files, `modelled` the paths above.
export function check(body, files, modelled) {
  const specs = files.filter((file) => file === 'specs' || file.startsWith('specs/'));
  if (specs.length) {
    return { ok: true, reason: `specs/ changed (${specs.length} ${specs.length === 1 ? 'file' : 'files'}): the model is in the change` };
  }
  const touched = files.filter((file) => modelled.has(file));
  if (!touched.length) {
    return { ok: true, reason: 'none of the files the models stand for changed' };
  }
  const note = (body || '')
    .split('\n')
    .map((line) => line.trim())
    .find((line) => NOTE.test(line));
  if (!note) {
    return {
      ok: false,
      reason: [
        `${touched.length} file(s) the models stand for changed, and the description has no \`模型：\` line:`,
        ...touched.map((file) => `  ${file}`),
        '',
        'Add one line to the description, for example:',
        '  模型：specs/events 不涉及（只改了错误文字）',
      ].join('\n'),
    };
  }
  const value = note.replace(/^模型\s*[：:]\s*/, '');
  if (!value.includes('不涉及')) {
    return { ok: false, reason: `the \`模型：\` line has to say 不涉及 when the model is not changed: ${note}` };
  }
  if (!/[（(][^）)\n]+[）)]/.test(value)) {
    return { ok: false, reason: `the \`模型：\` line needs the reason in parentheses: ${note}` };
  }
  return { ok: true, reason: `the description says: ${note}` };
}

function selfTest() {
  const modelled = new Set(['crates/rutis/src/bus.rs', 'crates/rutis-bridge/src/session/rpc.rs', 'node/rutis-runtime/src/serve.mjs']);
  const cases = [
    ['specs changed', '', ['specs/events/README.md', 'crates/rutis/src/bus.rs'], true],
    ['no specs, a note', 'fix: a name\n\n模型：specs/events 不涉及（只改了名字）', ['crates/rutis/src/bus.rs'], true],
    ['no specs, no note', 'fix: a name', ['crates/rutis/src/bus.rs'], false],
    ['docs only', '', ['docs/ci.md', 'README.md', 'docs/design-events-2026-10-18.md'], true],
    ['runtime note', '模型: specs/remote-lease 不涉及（没有改监听端）', ['node/rutis-runtime/src/serve.mjs'], true],
    ['note without a reason', '模型：specs/events 不涉及', ['node/rutis-runtime/src/serve.mjs'], false],
    ['note that says the model changed', '模型：specs/events 改了', ['crates/rutis-bridge/src/session/rpc.rs'], false],
    ['paths no model stands for', '', ['crates/rutis-cli/src/main.rs', 'docs/ci.en.md'], true],
  ];
  let bad = 0;
  for (const [name, body, files, want] of cases) {
    const result = check(body, files, modelled);
    if (result.ok !== want) {
      bad += 1;
      console.error(`self-test: ${name}: want ${want ? 'pass' : 'fail'}, got ${result.ok ? 'pass' : 'fail'}: ${result.reason}`);
    }
  }
  // The list comes from the model READMEs: it has to be more than a few paths,
  // each one a file, and a path a model is built on has to be in it.
  const paths = modelledPaths();
  if (paths.size < 20) { bad += 1; console.error(`self-test: only ${paths.size} modelled paths; the README tables are not read`) }
  for (const path of paths) if (!exists(path)) { bad += 1; console.error(`self-test: ${path} is listed but not there`) }
  for (const path of ['crates/rutis-bridge/src/session/rpc.rs', 'node/rutis-runtime/src/serve.mjs']) {
    if (!paths.has(path)) { bad += 1; console.error(`self-test: ${path} is not in the modelled paths`) }
  }
  if (bad) {
    console.error(`self-test: ${bad} problem(s)`);
    process.exit(1);
  }
  console.log(`self-test: ${cases.length} cases, ${paths.size} modelled paths, ok`);
}

function git(...args) {
  return execFileSync('git', ['-C', root, ...args], { encoding: 'utf8', maxBuffer: 1 << 28 });
}

function main() {
  const args = process.argv.slice(2);
  const option = (name) => {
    const at = args.indexOf(name);
    return at >= 0 ? args[at + 1] : undefined;
  };
  if (args.includes('--self-test')) return selfTest();
  if (args.includes('--list')) {
    const paths = [...modelledPaths()].sort();
    console.log(`${paths.length} paths the models stand for:`);
    for (const path of paths) console.log(`  ${path}`);
    return;
  }
  const bodyFile = option('--body-file');
  const filesFile = option('--files');
  const base = option('--files-from-git');
  const body = bodyFile ? readFileSync(bodyFile, 'utf8') : option('--body') || '';
  let files;
  if (filesFile) files = readFileSync(filesFile, 'utf8').split('\n').map((line) => line.trim()).filter(Boolean);
  else if (base) files = git('diff', '--name-only', `${base}...HEAD`).split('\n').filter(Boolean);
  else {
    console.error('check-model-pr: give --files-from-git BASE, --files FILE or --self-test');
    process.exit(2);
  }
  const result = check(body, files, modelledPaths());
  if (!result.ok) {
    console.error(`check-model-pr: fail — ${result.reason}`);
    process.exit(1);
  }
  console.log(`check-model-pr: pass — ${files.length} changed files, ${result.reason}`);
}

const invoked = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked) main();
