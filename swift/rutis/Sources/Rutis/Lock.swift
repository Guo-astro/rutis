import Foundation

/// A mutual exclusion lock, held only for short sections that never await.
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
