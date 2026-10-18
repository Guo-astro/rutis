// The plugin `conformance-events` (conformance-events.ts) for emitter rows
// labelled `e`: it declares `emitter-e` alone. Copied out of the repository
// together with conformance-events.ts.
import { EMITTER } from './conformance-events.ts'

export { name, inject, apply } from './conformance-events.ts'
export const provides = { 'emitter-e': EMITTER }
