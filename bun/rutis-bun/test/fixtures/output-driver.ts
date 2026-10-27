// For test/output.test.ts (#678): a runtime in a process of its own, whose
// stdout and stderr the test reads. Two rows (`weather`, `chat`) of a plugin
// that writes; `bun output-driver.ts <project> <step>…` runs each step:
// `say`, `shout`, `later`, `pieces`, `block:<ms>`, `wait:<ms>`.
import { Runtime } from '../../src/runner.ts'

const [project, ...steps] = process.argv.slice(2)
const runtime = new Runtime(project)
const control = (method: string, args: unknown) => runtime.dispatch('', method, args) as Promise<unknown>
await control('mount', {})
const METHODS = { say: 'async', shout: 'async', later: 'async', pieces: 'async', block: 'async', wait: 'async' }
await control('rows.load', ['a', './talk.ts', { label: 'a' }, [], [], { 'talk-a': METHODS }, { scope: [], id: 'weather' }])
await control('rows.load', ['b', './talk.ts', { label: 'b' }, [], [], { 'talk-b': METHODS }, { scope: [], id: 'chat' }])
for (const step of steps) {
  const [name, arg] = step.split(':')
  const target = name === 'shout' || name === 'pieces' ? 'talk-b' : 'talk-a'
  await runtime.dispatch(target, name, arg !== undefined ? [Number(arg)] : [name])
}
await new Promise<void>(resolve => process.stdout.write('', () => resolve()))
process.exit(0)
