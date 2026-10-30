#!/usr/bin/env node
// The GitHub Release notes of a version: docs/releases/<version>.en.md with
// its relative links made absolute, pointing into the repository at the
// version's tag. On the release page a relative link resolves against
// /releases/tag/, where none of the files are.
//
//   node tools/release-notes.mjs 0.19.1 > notes.md
import { readFileSync } from 'node:fs'
import { posix } from 'node:path'

const REPO = 'https://github.com/arcships/rutis'
const version = process.argv[2]
if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
  console.error('usage: node tools/release-notes.mjs <X.Y.Z>')
  process.exit(2)
}

const source = `docs/releases/${version}.en.md`
const text = readFileSync(new URL(`../${source}`, import.meta.url), 'utf8')
process.stdout.write(absolute(text, posix.dirname(source), `v${version}`))

/** `text` with each relative Markdown link target under `dir` made absolute
 * at `tag`; code spans and fenced blocks are left as they are. */
export function absolute(text, dir, tag) {
  let fenced = false
  return text
    .split('\n')
    .map((line) => {
      if (/^\s*(```|~~~)/.test(line)) {
        fenced = !fenced
        return line
      }
      if (fenced) return line
      // Outside inline code: split on backtick spans and rewrite the rest.
      return line
        .split(/(`[^`]*`)/)
        .map((part, i) => (i % 2 ? part : part.replace(/\]\(([^)\s]+)\)/g, (all, target) => {
          if (/^([a-z][a-z0-9+.-]*:|#|\/)/i.test(target)) return all
          const [path, anchor] = target.split(/(?=#)/)
          const resolved = posix.normalize(posix.join(dir, path))
          return `](${REPO}/blob/${tag}/${resolved}${anchor ?? ''})`
        })))
        .join('')
    })
    .join('\n')
}
