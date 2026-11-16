// The session's writing thread: frames are queued in the order they are taken
// and written by one thread of the session's own. A far end that stops reading
// holds up that thread alone: the frame's sender (a plugin's Task, and the
// thread that flushed it) returns once its frame is queued (#630).
import Foundation

/// How long a session's end waits for the frames it already took to be
/// written. A far end that stopped reading ends the session at the channel's
/// own write timeout instead (30s on a connection, 60s on a socket), which no
/// `close()` waits for.
package let writerDrainSeconds = 5.0

/// The frames a session has for the far end, written in order on a thread of
/// their own.
package final class Writer: @unchecked Sendable {
    private let channel: Channel
    /// Holds the queue, and the thread waits on it.
    private let condition = NSCondition()
    /// The frames not written yet, from `head` on. No limit of its own: the
    /// queue only grows while this thread is held by a far end that stopped
    /// reading, and that write times out.
    private var queue: [[UInt8]] = []
    private var head = 0
    /// The thread is in `channel.send` for the frame it took.
    private var writing = false
    private var running = false
    /// `drain` came: write what is queued, then stop.
    private var stopping = false
    /// The thread is gone: nothing is taken from now on.
    private var ended = false
    private var failure: Error?
    /// What the session does with a failed write; see `onFailure`.
    private var failureHandler: (@Sendable (Error) -> Void)?

    package init(_ channel: Channel) { self.channel = channel }

    /// What the session does when a write fails: told once, from this thread,
    /// and the session ends there (as a write that failed on the sending
    /// thread used to, #630). Set before `start()` and `enqueue`.
    package func onFailure(_ handler: @escaping @Sendable (Error) -> Void) {
        condition.lock()
        failureHandler = handler
        condition.unlock()
    }

    /// Starts the thread; the frames queued from then on are written.
    package func start() {
        condition.lock()
        if running || ended {
            condition.unlock()
            return
        }
        running = true
        condition.unlock()
        let thread = Thread { [self] in write() }
        thread.name = "rutis writer"
        // One frame's bytes at a time: no recursion, no deep values here.
        thread.stackSize = 1 << 20
        thread.start()
    }

    /// Queues `frame` behind the frames taken before it. The caller does not
    /// wait for the write: this is what keeps a far end that stopped reading
    /// off the cooperative pool (#630).
    package func enqueue(_ frame: [UInt8]) throws {
        condition.lock()
        defer { condition.unlock() }
        if let failure { throw failure }
        if ended || stopping { throw ChannelError("the session's writer stopped") }
        guard running else { throw ChannelError("the session's writer is not started") }
        queue.append(frame)
        condition.broadcast()
    }

    /// The session is over: the frames taken before it go out all the same,
    /// and this waits (at most `seconds`) for that to be done. Whether nothing
    /// is left with the writer: written, or given up on with the channel.
    ///
    /// The wait is what keeps a session's end from dropping the last frames:
    /// a process that ends next, as a runtime's does, takes none of them along
    /// (#630). A far end that reads nothing costs the bound, no more.
    @discardableResult
    package func drain(within seconds: Double) -> Bool {
        condition.lock()
        stopping = true
        condition.broadcast()
        let deadline = Date().addingTimeInterval(seconds)
        while !ended, !(head >= queue.count && !writing) {
            if !condition.wait(until: deadline) { break }
        }
        let drained = ended || (head >= queue.count && !writing)
        condition.unlock()
        return drained
    }

    private func write() {
        condition.lock()
        while true {
            while head >= queue.count, !stopping {
                condition.wait()
            }
            // Stopping with nothing left, under the same lock `enqueue`
            // appends with: no frame taken just before this is left behind.
            if head >= queue.count {
                ended = true
                queue = []
                head = 0
                condition.broadcast()
                condition.unlock()
                return
            }
            let frame = queue[head]
            head += 1
            writing = true
            if head >= queue.count {
                queue = []
                head = 0
            }
            condition.unlock()
            do {
                try channel.send(frame)
            } catch {
                var handler: (@Sendable (Error) -> Void)? = nil
                condition.lock()
                if !ended {
                    ended = true
                    failure = error
                    queue = []
                    head = 0
                    handler = failureHandler
                }
                writing = false
                condition.broadcast()
                condition.unlock()
                // Not under the lock: the session answers by ending, and
                // ends nothing else of this thread.
                handler?(error)
                return
            }
            condition.lock()
            writing = false
            condition.broadcast()
        }
    }
}
