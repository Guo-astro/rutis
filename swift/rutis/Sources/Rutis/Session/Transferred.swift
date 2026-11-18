import Foundation

/// A value a task takes over: the sender makes the box, passes it to one
/// task, and touches `value` no more.
///
/// Swift 6's `sending` says this in a parameter; the SDK needs it where the
/// value is captured by a task it starts itself (`Task { }` around a
/// plugin's handler, `Thread.detachNewThread`). The plugin API hands the
/// SDK closures that are not `@Sendable` — a Swift plugin's `ctx.on` and
/// `Rutis.define` handlers may capture the plugin's own state — and the
/// session runs each in one task, exactly as it would on the caller's
/// thread.
///
/// `@unchecked Sendable`, and safe by the rule above: the box is built at
/// the hand-over, its `value` is read only from the one task it was handed
/// to, and the sender keeps no copy it can reach after the hand-over.
package struct Transferred<Value>: @unchecked Sendable {
    package let value: Value

    package init(_ value: Value) { self.value = value }
}
