// A Cordis plugin's config checked with its own `Config` (a Standard
// Schema, such as schemastery), as Cordis will: its problems as a
// `bad-config` error written as every runtime writes them (config schema
// design §2.3), the path from each issue's `path`, the text from its
// `message`, instead of Cordis's ValidationError (which keeps no issues).
// Checked before the fiber runs, on the config as the row has it: a hook
// of Cordis's `internal/config` waterfall that would make it valid does not
// run here, and a schemastery transform runs again when Cordis resolves
// the config itself.
export function cordisConfig(plugin, config) {
  const standard = plugin?.Config?.['~standard']
  if (typeof standard?.validate !== 'function') return
  const result = standard.validate(config)
  if (!result || typeof result.then === 'function' || !result.issues?.length) return
  const issues = result.issues.map(issue => {
    const path = (issue.path ?? []).map(segment => (segment !== null && typeof segment === 'object' ? segment.key : segment))
      .map(segment => (typeof segment === 'number' ? segment : String(segment)))
    return { path, message: `${pathText(path)} ${issue.message}` }
  })
  throw configError(issues)
}

// At most this many problems are written; the rest as one last line.
const SHOWN = 20

// The error a row fails with when its config does not match: a
// BindingError with code `bad-config`, its message the lines
// `config: <problem>` (at most 20, then `config: and N more problems`), its
// data `{ issues }`, as the kernel writes a JSON Schema's (config schema
// design §5.2).
export function configError(issues) {
  const lines = issues.slice(0, SHOWN).map(problem => `config: ${problem.message}`)
  if (issues.length > SHOWN) lines.push(`config: and ${issues.length - SHOWN} more problems`)
  const error = new Error(lines.join('\n'))
  Object.defineProperty(error, 'name', { value: 'BindingError', configurable: true, writable: true, enumerable: false })
  return Object.assign(error, { code: 'bad-config', data: { issues: issues.map(({ path, message }) => ({ path, message })) } })
}

// How a path is written: `home.lat`, `cities[2]`, `headers["x-api"]`;
// `config` for the root, and before a first index or non-identifier key.
export function pathText(path) {
  let out = ''
  path.forEach((segment, index) => {
    if (typeof segment === 'number') out += `${index === 0 ? 'config' : ''}[${segment}]`
    else if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(segment)) out += `${index === 0 ? '' : '.'}${segment}`
    else out += `${index === 0 ? 'config' : ''}[${JSON.stringify(segment)}]`
  })
  return out || 'config'
}
