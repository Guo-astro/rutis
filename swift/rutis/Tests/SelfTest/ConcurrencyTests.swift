import Foundation
@testable import Rutis

// The concurrency boundaries of the SDK (#1273): the primitives the session
// and the runtime share state with, and the session's own calls. Every one
// of these runs under ThreadSanitizer in CI too (`runtimes-swift`).

struct ConcurrencyTests {
    /// Every waiter is resumed once by `finish`, and a later `finish` says
    /// it was already done.
    func testWaitersResumeEachWaiterOnce() async throws {
        let waiters = Waiters()
        let resumed = Locked(0)
        await withTaskGroup(of: Void.self) { group in
            for _ in 0..<32 {
                group.addTask {
                    await waiters.wait()
                    resumed.withLock { $0 += 1 }
                }
            }
            try? await Task.sleep(nanoseconds: 20_000_000)
            XCTAssertTrue(waiters.finish())
            await group.waitForAll()
        }
        XCTAssertEqual(resumed.read { $0 }, 32)
        XCTAssertFalse(waiters.finish())
        XCTAssertTrue(waiters.isDone)
    }

    /// A waiter cancelled before the finish stops waiting, and the finish
    /// does not try to resume it twice.
    func testCancellingAWaitRemovesIt() async throws {
        let waiters = Waiters()
        let waiting = Event()
        let task = Task {
            waiting.fire()
            await waiters.wait()
        }
        await waiting.wait()
        task.cancel()
        await task.value
        XCTAssertTrue(waiters.finish())
    }

    /// Each abort handler runs once, in both orders: registered before the
    /// abort, and after it (then at once).
    func testSignalRunsEachAbortHandlerOnce() async throws {
        let signal = Signal()
        let runs = Locked(0)
        for _ in 0..<8 {
            signal.onAbort { runs.withLock { $0 += 1 } }
        }
        signal.abort()
        signal.abort()
        signal.onAbort { runs.withLock { $0 += 1 } }
        XCTAssertEqual(runs.read { $0 }, 9)
        XCTAssertTrue(signal.isAborted)
        await signal.wait()
    }

    /// The serial queue runs its work one at a time; work pushed while it
    /// runs is taken in order, after what was queued before.
    func testSerialQueueRunsOneAtATime() async throws {
        let queue = SerialQueue()
        let running = Locked(0)
        let overlaps = Locked(0)
        let order = Locked<[Int]>([])
        let finished = Event()
        for index in 0..<100 {
            Task.detached {
                queue.push {
                    let now = running.withLock { running -> Int in
                        running += 1
                        return running
                    }
                    if now > 1 { overlaps.withLock { $0 += 1 } }
                    order.withLock { $0.append(index) }
                    await Task.yield()
                    running.withLock { $0 -= 1 }
                    if order.read({ $0.count }) == 100 { finished.fire() }
                }
            }
        }
        let fired = await finished.wait(seconds: 5)
        XCTAssertTrue(fired)
        XCTAssertEqual(overlaps.read { $0 }, 0)
        XCTAssertEqual(order.read { $0 }.count, 100)
    }

    /// A flag is set once; the first test-and-set says it was not set.
    func testFlagIsSetOnce() {
        let flag = Flag()
        XCTAssertFalse(flag.isSet)
        XCTAssertFalse(flag.testAndSet())
        XCTAssertTrue(flag.isSet)
        XCTAssertTrue(flag.testAndSet())
    }

    /// Calls made at once keep their own answers: one reader thread, one
    /// writer thread, and a waiter per call, with no answer crossing to
    /// another call.
    func testConcurrentCallsKeepTheirAnswers() async throws {
        let (host, runtime) = try await pairOfPeers { _, method, args in
            guard method == "echo" else { throw RutisError(name: "Nope", message: "no \(method)") }
            return args.first ?? .undefined
        }
        defer {
            host.close()
            runtime.close()
        }
        let count = 64
        try await withThrowingTaskGroup(of: (Int, Value).self) { group in
            for index in 0..<count {
                group.addTask {
                    let value = try await host.call(target: "t", method: "echo",
                                                    args: [.data(.int(Int64(index)))], sync: false)
                    return (index, value)
                }
            }
            var answered = 0
            for try await (index, value) in group {
                XCTAssertEqual(value.plainJSON, .number(String(index)))
                answered += 1
            }
            XCTAssertEqual(answered, count)
        }
    }
}
