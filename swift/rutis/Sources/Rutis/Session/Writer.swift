// The session's writing thread: frames are queued in the order they are taken
// and written by one thread of the session's own. A far end that stops reading
// holds up that thread alone: the frame's sender (a plugin's Task, and the
// thread that flushed it) returns once its frame is queued (#630).
import Foundation

/// The frames a session has for the far end, written in order on a thread of
/// their own.
final class Writer: @unchecked Sendable {
    private let channel: Channel
    private let lock = Lock()
    /// The frames not written yet, from `head` on. No limit of its own: the
    /// queue only grows while this thread is held by a far end that stopped
    /// reading, and the channel's write timeout (30s on a connection, 60s on
    /// a socket) ends the session past it (#630).
    private var queue: [[UInt8]] = []
    private var head = 0
    private var running = false
    /// `finish()` came: write what is queued, then stop.
    private var stopping = false
    /// The thread is gone: nothing is taken from now on.
    private var ended = false
    private var failure: Error?
    /// What the session does with a failed write; see `onFailure`.
    private var failureHandler: (@Sendable (Error) -> Void)?
    /// Wakes the thread for a queued frame, or for `finish()`.
    private let ready = DispatchSemaphore(value: 0)

    init(_ channel: Channel) { self.channel = channel }

    /// What the session does when a write fails: told once, from this thread,
    /// and the session ends there (as a write that failed on the sending
    /// thread used to, #630). Set before `start()` and `enqueue`.
    func onFailure(_ handler: @escaping @Sendable (Error) -> Void) {
        lock.withLock { failureHandler = handler }
    }

    /// Starts the thread; the frames queued from then on are written.
    func start() {
        lock.lock()
        if running || ended {
            lock.unlock()
            return
        }
        running = true
        lock.unlock()
        let thread = Thread { [self] in write() }
        thread.name = "rutis writer"
        // One frame's bytes at a time: no recursion, no deep values here.
        thread.stackSize = 1 << 20
        thread.start()
    }

    /// Queues `frame` behind the frames taken before it. The caller does not
    /// wait for the write: this is what keeps a far end that stopped reading
    /// off the cooperative pool (#630).
    func enqueue(_ frame: [UInt8]) throws {
        try lock.withLock {
            if let failure { throw failure }
            if ended || stopping { throw ChannelError("the session's writer stopped") }
            guard running else { throw ChannelError("the session's writer is not started") }
            queue.append(frame)
        }
        ready.signal()
    }

    /// The session is over: the frames taken before it go out all the same,
    /// and the thread stops once the queue is empty. Returns at once: the
    /// thread finishes the writes, not whoever ends the session.
    func finish() {
        lock.lock()
        let wake = !ended && !stopping
        stopping = true
        lock.unlock()
        if wake { ready.signal() }
    }

    /// The next frame to write; nil once none is queued.
    private func take() -> [UInt8]? {
        lock.withLock {
            guard head < queue.count else {
                queue = []
                head = 0
                return nil
            }
            defer { head += 1 }
            return queue[head]
        }
    }

    private func write() {
        while true {
            ready.wait()
            while let frame = take() {
                do {
                    try channel.send(frame)
                } catch {
                    var handler: (@Sendable (Error) -> Void)? = nil
                    lock.withLock {
                        if ended { return }
                        ended = true
                        failure = error
                        queue = []
                        head = 0
                        handler = failureHandler
                    }
                    // Not under the lock: the session answers by ending, and
                    // ends this thread with it.
                    handler?(error)
                    return
                }
            }
            // Stopping with the queue empty, under the same lock `enqueue`
            // appends with: no frame taken just before this is left behind.
            let done = lock.withLock { () -> Bool in
                guard stopping, head >= queue.count else { return false }
                ended = true
                queue = []
                head = 0
                return true
            }
            if done { return }
        }
    }
}
