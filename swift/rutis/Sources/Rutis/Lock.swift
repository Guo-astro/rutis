import Foundation

/// A mutual exclusion lock, held only for short sections that never await.
///
/// `@unchecked Sendable`, and safe: the only stored property is an `NSLock`,
/// which is unchanged after `init` and documented to serialize its callers
/// itself; this type adds no state of its own. Every `lock()` has its
/// `unlock()` in the same scope (or `withLock`), and no section waits.
package final class Lock: @unchecked Sendable {
    private let mutex = NSLock()

    package init() {}

    package func lock() { mutex.lock() }
    package func unlock() { mutex.unlock() }

    package func withLock<R>(_ body: () throws -> R) rethrows -> R {
        mutex.lock()
        defer { mutex.unlock() }
        return try body()
    }
}

/// Mutable state its owner shares between tasks, with the lock that guards
/// it: the value is private to the box, and every read and write goes
/// through `withLock`, so no access can forget the lock.
///
/// This is the SDK's one unchecked container, and it is what lets the
/// classes that hold one (a socket's buffers, the session's tables, the
/// listener counts) be `Sendable` by the compiler's own rule instead of
/// `@unchecked Sendable` themselves: they hold a `let` box and no other
/// mutable state. `@unchecked Sendable`, and safe: `value` is `private` and
/// only reachable inside `lock`; the lock is unchanged after `init` and
/// serializes its callers.
///
/// A `body` must not await: the lock is not reentrant, and its holders call
/// back into the box.
package final class Locked<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value

    package init(_ value: Value) { self.value = value }

    /// Reads or changes the value; `body` runs while the lock is held.
    package func withLock<R>(_ body: (inout Value) throws -> R) rethrows -> R {
        lock.lock()
        defer { lock.unlock() }
        return try body(&value)
    }

    /// Reads the value; `body` runs while the lock is held.
    package func read<R>(_ body: (Value) throws -> R) rethrows -> R {
        lock.lock()
        defer { lock.unlock() }
        return try body(value)
    }
}
