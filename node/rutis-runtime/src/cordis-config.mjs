import { configError, pathText } from './config.mjs'

// A Cordis plugin's config checked with its own `Config` (a Standard
// Schema, such as schemastery), as Cordis will: its problems as a
// `bad-config` error written as every runtime writes them (config schema
// design §2.3), the path from each issue's `path`, the text from its
// `message`, instead of Cordis's ValidationError.
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
