// Moves the baseline to the newest published Cordis and dsh plugins, for the
// nightly report (risk F4, docs/ci.md): `node latest.mjs` rewrites this
// package.json's dependencies (never committed) and prints a Markdown table
// of the pinned and the newest versions; `npm install --no-package-lock`
// then installs them.
//
// The newest version of a package is the higher of its `latest` and `next`
// dist-tags: dsh publishes its releases under `next` (its `latest` is an old
// 0.0.1 build), Cordis under `latest`. Alpha builds (other tags) are left out.
import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'

const file = new URL('./package.json', import.meta.url)
const manifest = JSON.parse(readFileSync(file, 'utf8'))

// Semantic version order, prerelease identifiers included (2.0.0 > 2.0.0-rc.1).
function compare(a, b) {
  const split = version => {
    const [core, pre] = version.split('+')[0].split(/-(.*)/s)
    return [core.split('.').map(Number), pre === undefined ? undefined : pre.split('.')]
  }
  const [coreA, preA] = split(a)
  const [coreB, preB] = split(b)
  for (let i = 0; i < 3; i++) if (coreA[i] !== coreB[i]) return coreA[i] - coreB[i]
  if (preA === undefined || preB === undefined) return (preA === undefined) - (preB === undefined)
  for (let i = 0; i < Math.max(preA.length, preB.length); i++) {
    if (preA[i] === undefined) return -1
    if (preB[i] === undefined) return 1
    const [numA, numB] = [/^\d+$/.test(preA[i]), /^\d+$/.test(preB[i])]
    if (numA && numB && Number(preA[i]) !== Number(preB[i])) return Number(preA[i]) - Number(preB[i])
    if (numA !== numB) return numA ? -1 : 1
    if (preA[i] !== preB[i]) return preA[i] < preB[i] ? -1 : 1
  }
  return 0
}

const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm'
const rows = []
for (const [name, pinned] of Object.entries(manifest.dependencies)) {
  const tags = JSON.parse(execFileSync(npm, ['view', name, 'dist-tags', '--json'], { encoding: 'utf8', shell: process.platform === 'win32' }))
  const newest = [tags.latest, tags.next].filter(Boolean).sort(compare).at(-1)
  manifest.dependencies[name] = newest
  rows.push(`| \`${name}\` | ${pinned} | ${newest}${newest === pinned ? '' : ' (new)'} |`)
}
writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`)
process.stdout.write(['| package | pinned | newest |', '| --- | --- | --- |', ...rows, ''].join('\n'))
