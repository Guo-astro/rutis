// A plugin whose apply fails (#584): with or without a row-level `inject`,
// the row reports this error.
export const name = 'failing-apply'
export async function apply(_ctx, config) {
  await Promise.resolve()
  throw Object.assign(new Error(`apply failed: ${config.label}`), { name: 'ApplyFailed' })
}
