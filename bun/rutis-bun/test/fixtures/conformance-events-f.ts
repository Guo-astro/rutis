// The plugin `conformance-events` (conformance-events.ts) for emitter rows
// labelled `f`: it declares `emitter-f` alone. Copied out of the repository
// together with conformance-events.ts.
const EMITTER = { dispatch: 'async', dispatch_sync: 'sync' }

export { name, inject, apply } from './conformance-events.ts'
export const provides = { 'emitter-f': EMITTER }
