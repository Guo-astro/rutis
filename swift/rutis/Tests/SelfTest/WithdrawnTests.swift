// A call for a listener this runtime withdrew (#1138): the host may have
// picked it for a dispatch before `events.off` reached it, and the call
// answers nothing; an unknown id, and one a row's unload took, still fail.
@testable import Rutis
import Foundation

/// A disposer the plugin hands to the test.
private final class Holder: @unchecked Sendable {
    private let lock = NSLock()
    private var value: (() -> Void)?
    var off: (() -> Void)? {
        get { lock.withLock { value } }
        set { lock.withLock { value = newValue } }
    }
}

final class WithdrawnTests {
    private func loaded(_ plugin: Rutis.Definition, _ key: String) async throws -> Runtime {
        let runtime = Runtime([plugin], project: "", settings: Events.Settings(log: { _ in }))
        try await runtime.load(key: key, name: nil, entry: plugin.name, config: .undefined, exports: [:], isolate: [:],
                               scope: [])
        return runtime
    }

    /// What the host's `events.call` answered: its value, or what it threw.
    private func call(_ runtime: Runtime, _ id: String, _ name: String = "t/x") async -> Result<Value, Error> {
        let record: [Value] = [.record([
            ("listener", .data(.string(id))), ("dispatch", .data(.string("d"))), ("own", .data(.bool(false))),
            ("name", .data(.string(name))), ("mode", .data(.string("serial"))), ("args", .list([])),
        ])]
        guard let then = runtime.arrive("events.call", record) else {
            return .failure(RutisError(name: "Error", message: "not an events.call"))
        }
        do {
            return .success(try await then())
        } catch {
            return .failure(error)
        }
    }

    private func expectBinding(_ result: Result<Value, Error>, _ message: String) {
        guard case let .failure(error) = result else {
            record("the call answered \(result), not \(message)")
            return
        }
        expect(error is RutisError, "\(error) is not a RutisError")
        expect((error as? RutisError)?.name == RutisError.Name.bindingError, "\(error) is not a BindingError")
        expect((error as? RutisError)?.message == message, "\(error) is not \(message)")
    }

    func aListenerWithdrawnHereAnswersACallAlreadyOnItsWayWithNothing() async throws {
        let holder = Holder()
        let plugin = Rutis.define(Plugin(name: "withdrawn") { ctx in
            holder.off = ctx.on("t/x") { () async -> String in "A" }
            ctx.on("t/y") { () async -> String in "Y" }
        })
        let runtime = try await loaded(plugin, "a")
        guard case .success(.data(.string("A"))) = await call(runtime, "1") else {
            record("the live listener ran")
            return
        }
        holder.off?()
        // The listener is gone, and the call answers as one that returns
        // nothing, so the dispatch under way goes on instead of failing.
        guard case .success(.undefined) = await call(runtime, "1") else {
            record("a listener withdrawn here answers a call already on its way with nothing")
            return
        }
        // An id this process never registered, and one a row's unload took
        // (the host withdrew it itself), stay the protocol failure they were.
        expectBinding(await call(runtime, "9"), "listener 9 of t/x is withdrawn")
        try await runtime.unload("a")
        expectBinding(await call(runtime, "2", "t/y"), "listener 2 of t/y is withdrawn")
    }

    func aRestartWithdrawnHereAnswersACallAlreadyOnItsWayWithNothing() async throws {
        let plugin = Rutis.define(Plugin(name: "restarted") { ctx in
            ctx.on("t/x") { () async -> String in "A" }
        })
        let runtime = try await loaded(plugin, "b")
        // `rows.update` keeps the row loaded on the host, whose table still
        // has the old listener until `events.off`.
        try await runtime.update("b", config: .undefined)
        guard case .success(.undefined) = await call(runtime, "1") else {
            record("a listener a restart withdrew answers a call already on its way with nothing")
            return
        }
    }
}
