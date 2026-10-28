#!/usr/bin/env node
// Runnable examples in the guides (Q2.13, #193). A code block in
// docs/guide/*.md that a reader can follow as written gets a marker on the
// line before it (HTML comments: GitHub does not show them):
//
//   <!-- example: <name> run -->                 run each line of the shell
//                                                block in order; `cd <dir>`
//                                                changes the directory for
//                                                the lines and blocks after it
//   <!-- example: <name> run until="<text>" -->  a command that keeps running
//                                                (dev, run): start it, wait for
//                                                <text> in its output, Ctrl-C,
//                                                exit code 0
//   <!-- example: <name> file=<path> -->         write the block to <path>,
//                                                relative to the example's root
//
// Any marker may add `order=<n>` to run out of document order (a command
// that needs files the text shows after it). A block's language may carry
// attributes after a comma (```rust,no_run is a rust block). Unmarked
// blocks are not checked. tests/e2e/tests/doc_examples.rs runs the
// examples with the published packages, after a release (install.yml): the
// Swift guides' examples on macOS, the others on Linux.
//
//   node tools/doc-examples.mjs --check          the markers parse
//   node tools/doc-examples.mjs --out <file>     write the examples as JSON
import { readdirSync, readFileSync, writeFileSync } from 'node:fs'

const guides = new URL('../docs/guide/', import.meta.url)
const MARKER = /^<!-- example: ([a-z0-9-]+) (.*?) ?-->$/

export function examples() {
  const found = new Map()
  const errors = []
  for (const file of readdirSync(guides).filter(name => name.endsWith('.md')).sort()) {
    const lines = readFileSync(new URL(file, guides), 'utf8').split('\n')
    lines.forEach((line, index) => {
      const marker = line.match(MARKER)
      if (!marker) {
        if (line.startsWith('<!-- example:')) errors.push(`${file}:${index + 1}: not a marker: ${line}`)
        return
      }
      const [, name, rest] = marker
      const where = `${file}:${index + 1}`
      const fence = lines[index + 1]?.match(/^```(\w*)(?:,[\w-]+)*$/)
      if (!fence) return errors.push(`${where}: a marker goes right before a code block`)
      const end = lines.findIndex((text, at) => at > index + 1 && text === '```')
      if (end < 0) return errors.push(`${where}: the code block does not end`)
      const body = lines.slice(index + 2, end).join('\n') + '\n'
      const step = { where, body }
      for (const [, key, quoted, plain] of rest.matchAll(/([a-z]+)(?:="([^"]*)"|=(\S+))?/g)) {
        const value = quoted ?? plain
        if (key === 'run') step.run = true
        else if (key === 'until') step.until = value
        else if (key === 'file') step.file = value
        else if (key === 'order') step.order = Number(value)
        else errors.push(`${where}: unknown marker field ${key}`)
      }
      if (step.run === !!step.file) errors.push(`${where}: a marker says run or file=<path>, not both`)
      if (step.until !== undefined && !step.run) errors.push(`${where}: until goes with run`)
      if (step.file?.startsWith('/') || step.file?.split('/').includes('..')) errors.push(`${where}: file= is relative to the example`)
      if (step.order !== undefined && !Number.isInteger(step.order)) errors.push(`${where}: order is a whole number`)
      if (step.run && fence[1] !== 'bash' && fence[1] !== 'sh') errors.push(`${where}: run needs a bash block`)
      const example = found.get(name) ?? { name, guide: file, steps: [] }
      if (example.guide !== file) errors.push(`${where}: example ${name} is also in ${example.guide}`)
      step.position = example.steps.length
      example.steps.push(step)
      found.set(name, example)
    })
  }
  for (const example of found.values()) {
    example.steps.sort((a, b) => (a.order ?? a.position) - (b.order ?? b.position) || a.position - b.position)
  }
  return { examples: [...found.values()], errors }
}

const [mode, out] = process.argv.slice(2)
const { examples: found, errors } = examples()
if (errors.length) {
  for (const error of errors) console.error(`docs/guide/${error}`)
  process.exit(1)
}
if (mode === '--out' && out) {
  writeFileSync(out, JSON.stringify(found, null, 2) + '\n')
} else if (mode !== '--check') {
  console.error('usage: node tools/doc-examples.mjs --check | --out <file>')
  process.exit(2)
}
const steps = found.reduce((count, example) => count + example.steps.length, 0)
console.log(`${found.length} examples, ${steps} steps: ${found.map(example => example.name).join(', ') || 'none'}`)
