import Foundation

// Events in the runtime (#464): every listener a row registers is
// registered with the host too; an emission whose listeners are all in this
// process is dispatched here (the fast path), any other by the host. The
// result is the same either way.

/// A listener of a row of this process.
final class LocalListener: EventEntry, @unchecked Sendable {
    unowned let runtime: Runtime
    let id: String
    let name: String
    let row: Row
    let body: ListenerBody
    let prepend: Bool
    let global: Bool
    /// Withdrawn (runtime.lock): no invocation starts any more.
    var withdrawn = false
    let queue = SerialQueue()

    init(runtime: Runtime, id: String, name: String, row: Row, body: @escaping ListenerBody, prepend: Bool, global: Bool) {
        self.runtime = runtime
        self.id = id
        self.name = name
        self.row = row
        self.body = body
        self.prepend = prepend
        self.global = global
    }

    var info: Rutis.Listener {
        let runtimeName = runtime.lock.withLock { runtime.runtimeName }
        return Rutis.Listener(name: name, plugin: row.definition.name, row: row.name, runtime: runtimeName, scope: row.scope,
                              global: global)
    }

    /// Whether invocations may start; runtime.lock is held.
    var live: Bool { !withdrawn && runtime.rows[row.key] === row }

    /// Counts an invocation in the row's, unless withdrawn.
    func reserve() -> Bool {
        runtime.lock.withLock { live } && row.reserve()
    }

    func begin() -> EventCall? {
        guard reserve() else { return nil }
        return { [self] mode, args, next in
            defer { row.release() }
            return try await run(args, next)
        }
    }

    func emit(_ args: [Value]) -> Bool {
        guard reserve() else { return false }
        let chain = CallChain.path
        queue.push { [self] in
            defer { row.release() }
            guard runtime.lock.withLock({ live }) else { return }
            do {
                _ = try await CallChain.$path.withValue(chain) { try await run(args, nil) }
            } catch {
                failed(error)
            }
        }
        return true
    }

    /// Reports an `emit` invocation's failure where the listener is (§5.6).
    func failed(_ error: Error) {
        // Cancelled because its row unloads (§7.1): not a failure.
        if error is CancellationError, row.isCancelled { return }
        Events.report("rutis: listener of \(name) in row \(row.name) failed: \(Events.failure(error))")
    }

    /// Calls the listener in a Task of its row's: cancelled when the
    /// caller gives up or the row unloads.
    func run(_ args: [Value], _ next: NextFunction?) async throws -> Value {
        let body = self.body
        return try await row.track { try await body(args, next) }
    }
}

extension Runtime {
    /// Registers a listener of `row` (ctx.on).
    func listen(_ row: Row, _ name: String, prepend: Bool, global: Bool, _ body: @escaping ListenerBody) -> () -> Void {
        do {
            try Events.checkListen(name)
        } catch {
            fail(row, error)
            return {}
        }
        lock.lock()
        guard rows[row.key] === row else {
            lock.unlock()
            fail(row, RutisError(name: "Error", message: "row \(row.key) is not active"))
            return {}
        }
        listenerSequence += 1
        let listener = LocalListener(runtime: self, id: String(listenerSequence), name: name, row: row, body: body,
                                     prepend: prepend, global: global)
        var list = listeners[name] ?? []
        if prepend {
            first -= 1
            list.insert(listener, at: 0)
        } else {
            last += 1
            list.append(listener)
        }
        listeners[name] = list
        byID[listener.id] = listener
        // The host hears of it before any emission this process makes after
        // it (queued in order); a row loading answers once the host has it,
        // and fails when the host refuses it (§4.4). Refused outside a load,
        // it is reported (§11).
        if live, let session {
            let registration: [Value] = [.data(.object([
                ("id", .string(listener.id)), ("name", .string(name)), ("row", .string(row.key)),
                ("plugin", .string(row.definition.name)), ("kind", .string("sync")), ("prepend", .bool(prepend)),
                ("global", .bool(global)),
            ]))]
            let loading = row.loading
            let answered = Event()
            let refusal = ErrorBox()
            let rowName = row.name
            post {
                do {
                    let call = try session.send(target: "", method: "events.on", args: registration, sync: false)
                    Task.detached {
                        do {
                            _ = try await call.value()
                        } catch {
                            if !session.isClosed {
                                if loading {
                                    refusal.set(error)
                                } else {
                                    Events.report("rutis: registration of listener of \(name) in row \(rowName) failed: \(Events.failure(error))")
                                }
                            }
                        }
                        answered.fire()
                    }
                } catch {
                    refusal.set(error)
                    answered.fire()
                }
            }
            if loading {
                row.pending.append {
                    await answered.wait()
                    return refusal.get()
                }
            }
        }
        lock.unlock()
        flush()
        return { [weak self] in
            guard let self else { return }
            lock.lock()
            if !listener.withdrawn {
                withdraw(listener)
                if live, let session {
                    post { session.notify(target: "", method: "events.off", args: [.data(.array([.string(listener.id)]))]) }
                }
            }
            lock.unlock()
            flush()
        }
    }

    /// Takes a listener off the table; `lock` is held.
    func withdraw(_ listener: LocalListener) {
        listener.withdrawn = true
        byID[listener.id] = nil
        var list = listeners[listener.name] ?? []
        list.removeAll { $0 === listener }
        listeners[listener.name] = list.isEmpty ? nil : list
    }

    /// Withdraws a row's listeners as it unloads; `lock` is held, and
    /// events.off is queued.
    func forgetListeners(_ row: Row) {
        let mine = byID.values.filter { $0.row === row }.sorted { (Int($0.id) ?? 0) < (Int($1.id) ?? 0) }
        for listener in mine { withdraw(listener) }
        if !mine.isEmpty, live, let session {
            let ids = JSON.array(mine.map { .string($0.id) })
            post { session.notify(target: "", method: "events.off", args: [.data(ids)]) }
        }
    }

    /// Whether an emission of `name` goes through the host (§5.2): a
    /// listener of it may be elsewhere, or an earlier emit of it through the
    /// host is unanswered. `lock` is held.
    func viaHost(_ name: String, _ mode: Mode) -> Bool {
        guard live, let session, session.error == nil else { return false }
        if elsewhere.contains(name) { return true }
        if elsewherePrefixes.contains(where: { name.hasPrefix($0) }) { return true }
        return mode == .emit && (hostEmits[name] ?? 0) > 0
    }

    /// The fast path's snapshot (§5.1); `lock` is held.
    func snapshot(_ row: Row, _ name: String, keep: ((Rutis.Listener) async throws -> Bool)?) -> Dispatch {
        let entries = (listeners[name] ?? []).filter { Events.within($0.row.scope, row.scope) }
        return Dispatch(name: name, entries: entries, keep: keep.map(protectedKeep))
    }

    func protectedKeep(_ keep: @escaping (Rutis.Listener) async throws -> Bool) -> (Rutis.Listener) async throws -> Bool { keep }

    /// An `emit` (§5.4): returns at once.
    func emitNow(_ row: Row, _ name: String, _ args: [Value], keep: ((Rutis.Listener) async throws -> Bool)?) throws -> Value {
        try Events.checkEmit(name)
        lock.lock()
        guard rows[row.key] === row else {
            lock.unlock()
            throw RutisError(name: RutisError.Name.bindingError, message: "row \(row.key) is not loaded in this runtime")
        }
        if viaHost(name, .emit) {
            try emitViaHost(row, name, args, keep: keep)
            return .undefined
        }
        let dispatch = snapshot(row, name, keep: keep)
        lock.unlock()
        if keep == nil {
            // Started now, in order (§5.5).
            for entry in dispatch.entries { entry.emit(args) }
            return .undefined
        }
        // A filter may wait (it is async): it runs before any start, in a
        // queue of this row's, so this row's emits still start in order.
        let chain = CallChain.path
        row.emits.push {
            do {
                try await CallChain.$path.withValue(chain) { try await dispatch.emit(args) }
            } catch {
                Events.report("rutis: filter of \(name) from row \(row.name) failed: \(Events.failure(error))")
            }
        }
        return .undefined
    }

    /// A dispatch other than `emit`.
    func dispatch(_ row: Row, _ name: String, _ mode: Mode, _ args: [Value], keep: ((Rutis.Listener) async throws -> Bool)?,
                  terminal: (([Value]) async throws -> Value)?) async throws -> Value
    {
        try Events.checkEmit(name)
        lock.lock()
        guard rows[row.key] === row else {
            lock.unlock()
            throw RutisError(name: RutisError.Name.bindingError, message: "row \(row.key) is not loaded in this runtime")
        }
        if viaHost(name, mode) {
            return try await dispatchViaHost(row, name, mode, args, keep: keep, terminal: terminal)
        }
        let dispatch = snapshot(row, name, keep: keep)
        lock.unlock()
        switch mode {
        case .parallel:
            try await dispatch.parallel(args)
            return .undefined
        case .waterfall:
            return try await dispatch.waterfall(mode, args, terminal: terminal)
        default:
            return try await dispatch.serial(mode, args)
        }
    }

    /// The `events.dispatch` record (§5.3).
    private func record(_ row: Row, _ name: String, _ mode: Mode, _ args: [Value], keep: ((Rutis.Listener) async throws -> Bool)?,
                        terminal: (([Value]) async throws -> Value)?) -> Value
    {
        var fields: [(String, Value)] = [("row", .data(.string(row.key))), ("name", .data(.string(name))),
                                         ("mode", .data(.string(mode.rawValue))), ("args", .list(args))]
        if let keep { fields.append(("filter", .function(selector(keep)))) }
        if let terminal, mode == .waterfall {
            // The host calls it with the event's arguments.
            fields.append(("terminal", .function(SessionFunction { args in try await terminal(args) })))
        }
        return .record(fields)
    }

    /// Sends an `emit` through the host; `lock` is held, and released. Its
    /// later emits of `name` follow it through the host until it is
    /// answered (§5.5). The emitter neither waits for nor cancels it.
    private func emitViaHost(_ row: Row, _ name: String, _ args: [Value], keep: ((Rutis.Listener) async throws -> Bool)?) throws {
        guard let session else {
            lock.unlock()
            return
        }
        let record = record(row, name, .emit, args, keep: keep, terminal: nil)
        hostEmits[name, default: 0] += 1
        let answered: @Sendable (Error?) -> Void = { [weak self] error in
            guard let self else { return }
            lock.withLock {
                self.hostEmits[name, default: 1] -= 1
                if self.hostEmits[name]! <= 0 { self.hostEmits[name] = nil }
            }
            if let error {
                Events.report("rutis: emit of \(name) from row \(row.name) failed: \(Events.failure(error))")
            }
        }
        // The answer is taken as its frame is read, before the next one: an
        // emit a later frame starts sees it answered (§5.5).
        let taken: @Sendable (Result<Value, Error>) -> Void = { result in
            switch result {
            case let .failure(error):
                answered(error)
            case let .success(.remoteFuture(future)):
                // An older host answers with a future: its settlement.
                Task.detached {
                    do {
                        _ = try await settle(.remoteFuture(future))
                        answered(nil)
                    } catch {
                        answered(error)
                    }
                }
            case .success:
                answered(nil)
            }
        }
        let sent = SentBox<Void>()
        post { sent.set(Result { try session.sendAnswered(target: "", method: "events.dispatch", args: [record], answered: taken) }) }
        lock.unlock()
        flush()
        if case let .failure(error) = sent.get() {
            answered(nil)
            throw error
        }
    }

    /// Sends `events.dispatch`; `lock` is held, and released. The frame is
    /// queued behind this process's earlier frames, such as the events.on
    /// of a listener registered before (§4.4).
    private func dispatchViaHost(_ row: Row, _ name: String, _ mode: Mode, _ args: [Value], keep: ((Rutis.Listener) async throws -> Bool)?,
                                 terminal: (([Value]) async throws -> Value)?) async throws -> Value
    {
        guard let session else {
            lock.unlock()
            throw RutisError.binding("the runtime has no session")
        }
        let record = record(row, name, mode, args, keep: keep, terminal: terminal)
        let sent = SentBox<OutgoingCall>()
        let chain = CallChain.path
        post {
            sent.set(Result {
                try CallChain.$path.withValue(chain) {
                    try session.send(target: "", method: "events.dispatch", args: [record], sync: mode == .bail)
                }
            })
        }
        lock.unlock()
        flush()
        let call = try sent.get().get()
        let value = try await settle(try await call.value())
        return mode == .parallel ? .undefined : value
    }

    /// The function the host calls with the descriptors of a dispatch's
    /// candidates, answering one boolean each (§6).
    private func selector(_ keep: @escaping (Rutis.Listener) async throws -> Bool) -> SessionFunction {
        SessionFunction { args in
            let descriptors = try Rutis.Arguments(args).decode(0, as: [Rutis.Listener]?.self) ?? []
            var answers: [JSON] = []
            for descriptor in descriptors { answers.append(.bool(try await keep(descriptor))) }
            return .data(.array(answers))
        }
    }

    // ── From the host ────────────────────────────────────────────────

    /// Takes the host's event calls on the reading thread, in frame order
    /// (§4.1, §5.5); the rest runs as any call does.
    func arrive(_ method: String, _ args: [Value]) -> (() async throws -> Value)? {
        switch method {
        case "events.elsewhere":
            let change = args.first ?? .undefined
            func names(_ key: String) -> [String] { (try? ValueCodec.decode([String]?.self, from: change[key] ?? .undefined)) ?? [] }
            lock.withLock {
                for name in names("add") { elsewhere.insert(name) }
                for name in names("remove") { elsewhere.remove(name) }
                for prefix in names("addPrefixes") { elsewherePrefixes.insert(prefix) }
                for prefix in names("removePrefixes") { elsewherePrefixes.remove(prefix) }
            }
            return { .undefined }
        case "events.call":
            return eventCall(args)
        default:
            return nil
        }
    }

    /// One invocation of a listener here (§5.3): an `emit` one is queued
    /// now, after the earlier ones of that listener (§5.5).
    private func eventCall(_ args: [Value]) -> () async throws -> Value {
        func failed(_ error: Error) -> () async throws -> Value { { throw error } }
        let record = args.first ?? .undefined
        guard let id = record["listener"]?.string, let name = record["name"]?.string,
              let modeText = record["mode"]?.string else {
            return failed(RutisError(name: "Error", message: "events.call: a listener, name and mode"))
        }
        guard let mode = Mode(rawValue: modeText) else {
            return failed(RutisError(name: "Error", message: "events.call: unknown mode \(modeText)"))
        }
        let values = record["args"]?.items ?? []
        let withdrawn = RutisError(name: RutisError.Name.bindingError, message: "listener \(id) of \(name) is withdrawn")
        guard let listener = lock.withLock({ byID[id] }) else { return failed(withdrawn) }
        if mode == .emit {
            guard listener.reserve() else { return failed(withdrawn) }
            let done = ErrorEvent()
            listener.queue.push {
                defer { listener.row.release() }
                guard self.lock.withLock({ listener.live }) else {
                    done.finish(withdrawn)
                    return
                }
                do {
                    _ = try await listener.run(values, nil)
                } catch {
                    listener.failed(error)
                }
                done.finish(nil)
            }
            // Answered once it ran; its failure is reported here.
            return {
                if let error = await done.wait() { throw error }
                return .undefined
            }
        }
        var next: NextFunction?
        if mode.isWaterfall {
            guard case let .remoteFunction(far) = record["next"] ?? .undefined else {
                return failed(RutisError(name: "Error", message: "events.call: a \(modeText) needs next"))
            }
            next = { try await settle(try await far.call([], sync: mode == .waterfallSync)) }
        }
        guard let call = listener.begin() else { return failed(withdrawn) }
        return { try await call(mode, values, next) }
    }
}

final class ErrorBox: @unchecked Sendable {
    private let lock = Lock()
    private var error: Error?
    func set(_ value: Error) { lock.withLock { if error == nil { error = value } } }
    func get() -> Error? { lock.withLock { error } }
}

final class SentBox<Sent>: @unchecked Sendable {
    private let lock = Lock()
    private var result: Result<Sent, Error> = .failure(RutisError.binding("not sent"))
    func set(_ value: Result<Sent, Error>) { lock.withLock { result = value } }
    func get() -> Result<Sent, Error> { lock.withLock { result } }
}

/// An event carrying an optional error.
final class ErrorEvent: @unchecked Sendable {
    private let event = Event()
    private let box = ErrorBox()
    func finish(_ error: Error?) {
        if let error { box.set(error) }
        event.fire()
    }

    func wait() async -> Error? {
        await event.wait()
        return box.get()
    }
}
