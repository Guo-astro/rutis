import Foundation
@testable import Rutis

/// The session's writing thread (#630).
struct WriterTests {
    /// The frames the writer took are in the channel before the session's end
    /// returns: a process that ends next, as a runtime's does, loses none.
    func theDrainWritesWhatTheWriterTook() throws {
        let (own, far) = try Channels.pair()
        defer { far.close() }
        let channel = StallingChannel(own)
        let writer = Writer(channel)
        writer.onFailure { _ in }
        writer.start()
        // A far end that reads, but slowly: the writes take a while.
        channel.slow(50)
        for frame in ["one", "two", "three"] { try writer.enqueue(Array(frame.utf8)) }
        let drained = writer.drain(within: 5)
        XCTAssertTrue(drained, "the writer left frames behind")
        XCTAssertEqual(channel.written, ["one", "two", "three"], "the frames were not written")
    }

    /// A far end that reads nothing costs the drain its bound, not the frames
    /// it took: they go out once the far end reads.
    func theDrainGivesUpAtItsBoundAndWritesWhatItTook() throws {
        let (own, far) = try Channels.pair()
        defer { far.close() }
        let channel = StallingChannel(own)
        let writer = Writer(channel)
        writer.onFailure { _ in }
        writer.start()
        channel.hold()
        for frame in ["one", "two"] { try writer.enqueue(Array(frame.utf8)) }
        let drained = writer.drain(within: 0.2)
        XCTAssertFalse(drained, "the drain waited for a far end that reads nothing")
        channel.unhold()
        for _ in 0 ..< 200 where channel.written.count < 2 { Thread.sleep(forTimeInterval: 0.01) }
        XCTAssertEqual(channel.written, ["one", "two"], "the frames the writer took were dropped")
    }
}

/// A channel whose writes the test holds or slows, as a far end that reads
/// nothing or slowly; everything else goes to `inner`, and the frames written
/// are kept in order (#630).
final class StallingChannel: Channel, @unchecked Sendable {
    private let inner: Channel
    private let condition = NSCondition()
    private var flowing = true
    private var closed = false
    private var failing = false
    private var delay: TimeInterval = 0
    private var sent: [[UInt8]] = []
    /// Fired once a write is held.
    let held = Event()

    init(_ inner: Channel) { self.inner = inner }

    /// What reached `inner`, in order.
    var written: [String] {
        condition.lock()
        defer { condition.unlock() }
        return sent.map { String(decoding: $0, as: UTF8.self) }
    }

    /// Every write from now on waits, as one to a far end that reads nothing.
    func hold() {
        condition.lock()
        flowing = false
        condition.unlock()
    }

    /// The held write, and the ones queued after it, go out.
    func unhold() {
        condition.lock()
        flowing = true
        condition.broadcast()
        condition.unlock()
    }

    /// Every write takes `milliseconds`, as one to a far end that reads
    /// slowly.
    func slow(_ milliseconds: Int) {
        condition.lock()
        delay = Double(milliseconds) / 1000
        condition.unlock()
    }

    /// A write that is let go fails: the far end went away while it waited.
    func failWrites() {
        condition.lock()
        failing = true
        condition.unlock()
    }

    func send(_ message: [UInt8]) throws {
        condition.lock()
        if !flowing, !closed {
            condition.unlock()
            held.fire()
            condition.lock()
            while !flowing, !closed { condition.wait() }
        }
        let (wasClosed, fails, pause) = (closed, failing, delay)
        condition.unlock()
        if wasClosed { throw ChannelError("the channel is closed") }
        if fails { throw ChannelError("the far end stopped reading") }
        if pause > 0 { Thread.sleep(forTimeInterval: pause) }
        try inner.send(message)
        condition.lock()
        sent.append(message)
        condition.unlock()
    }

    func receive() throws -> [UInt8]? { try inner.receive() }

    func close() {
        condition.lock()
        closed = true
        condition.broadcast()
        condition.unlock()
        inner.close()
    }
}
