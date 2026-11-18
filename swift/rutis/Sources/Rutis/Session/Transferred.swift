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

/// A value that never changes after it is made, shared by any number of
/// tasks: the box the SDK puts a CoreFoundation object in when the compiler
/// cannot see that it is immutable.
///
/// `Security`'s certificates and identities (`SecCertificate`,
/// `SecIdentity`) are CF objects: made once, never written, and safe to
/// read from several threads — but their Swift types are not `Sendable`.
/// `RutisApp` holds them for the life of the app and hands them to the
/// network stacks from its tasks, so they travel in this box.
///
/// `@unchecked Sendable`, and safe: `value` is a `let`, assigned once at
/// `init`; nothing here writes it, and the types it carries are documented
/// to be immutable.
package struct Immutable<Value>: @unchecked Sendable {
    package let value: Value

    package init(_ value: Value) { self.value = value }
}
