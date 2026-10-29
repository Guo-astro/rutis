import Foundation

// Events in the runtime (#464): every listener a row registers is
// registered with the host, and every emission is dispatched by the host,
// which calls the listeners here back (`events.call`).

/// A listener of a row of this process.
final class LocalListener: @unchecked Sendable {
    /// Weak: an invocation queued before the runtime ended may run after
    /// it is gone, and then starts nothing.
    weak var runtime: Runtime?
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

    var info: Rutis.Advanced.Listener {
        let runtimeName = runtime.flatMap { runtime in runtime.lock.withLock { runtime.runtimeName } }
        return Rutis.Advanced.Listener(name: name, plugin: row.definition.name, row: row.name, runtime: runtimeName, scope: row.scope,
                              global: global)
    }

    /// Whether invocations may start; runtime.lock is held.
    var live: Bool { !withdrawn && runtime?.rows[row.key] === row }

    /// Counts an invocation in the row's, unless withdrawn.
    func reserve() -> Bool {
        guard let runtime else { return false }
        return runtime.lock.withLock { live } && row.reserve()
    }

    func begin() -> EventCall? {
        guard reserve() else { return nil }
        return { [self] mode, args, next in
            defer { row.release() }
            return try await run(args, next)
        }
    }


    /// Reports an `emit` invocation's failure where the listener is (§5.6).
    func failed(_ error: Error) {
        // Cancelled because its row unloads (§7.1): not a failure.
        if error is CancellationError, row.isCancelled { return }
        Events.report("rutis: listener of \(name) in row \(row.name) failed: \(Events.failure(error))", to: row.settings.log)
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
            fail(row, RutisError(name: "Error", message: "row \(row.name) is not active"))
            return {}
        }
        listenerSequence += 1
        let listener = LocalListener(runtime: self, id: String(listenerSequence), name: name, row: row, body: body,
                                     prepend: prepend, global: global)
        byID[listener.id] = listener
        // The host hears of it before any emission this process makes after
        // it (queued in order); a row loading answers once the host has it,
        // and fails when the host refuses it (§4.4). Refused outside a load,
        // it is reported (§11).
        if live, let session {
            // An app's own plugin is no row of rutis's: its listeners are
            // the connection's.
            var fields: [(String, JSON)] = [("id", .string(listener.id)), ("name", .string(name))]
            if !row.own { fields.append(("row", .string(row.key))) }
            fields += [("plugin", .string(row.definition.name)), ("kind", .string("sync")), ("prepend", .bool(prepend)),
                       ("global", .bool(global))]
            let registration: [Value] = [.data(.object(fields))]
            let loading = row.loading
            let answered = Event()
            let refusal = ErrorBox()
            let rowName = row.name
            let log = row.settings.log
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
                                    Events.report("rutis: registration of listener of \(name) in row \(rowName) failed: \(Events.failure(error))", to: log)
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

    /// An `emit` (§5.4): sent to the host, returning at once.
    func emitNow(_ row: Row, _ name: String, _ args: [Value], keep: ((Rutis.Advanced.Listener) async throws -> Bool)?) throws -> Value {
        try Events.checkEmit(name)
        lock.lock()
        guard rows[row.key] === row else {
            lock.unlock()
            throw RutisError(name: RutisError.Name.bindingError, message: "row \(row.name) is not loaded in this runtime")
        }
        try emitViaHost(row, name, args, keep: keep)
        return .undefined
    }

    /// A dispatch other than `emit`, by the host.
    func dispatch(_ row: Row, _ name: String, _ mode: Mode, _ args: [Value], keep: ((Rutis.Advanced.Listener) async throws -> Bool)?,
                  terminal: (([Value]) async throws -> Value)?) async throws -> Value
    {
        try Events.checkEmit(name)
        lock.lock()
        guard rows[row.key] === row else {
            lock.unlock()
            throw RutisError(name: RutisError.Name.bindingError, message: "row \(row.name) is not loaded in this runtime")
        }
        return try await dispatchViaHost(row, name, mode, args, keep: keep, terminal: terminal)
    }

    /// The `events.dispatch` record (§5.3).
    private func record(_ row: Row, _ name: String, _ mode: Mode, _ args: [Value], keep: ((Rutis.Advanced.Listener) async throws -> Bool)?,
                        terminal: (([Value]) async throws -> Value)?) -> Value
    {
        // An app's own plugin emits as the connection.
        var fields: [(String, Value)] = row.own ? [] : [("row", .data(.string(row.key)))]
        fields += [("name", .data(.string(name))), ("mode", .data(.string(mode.rawValue))), ("args", .list(args))]
        if let keep { fields.append(("filter", .function(selector(keep)))) }
        if let terminal, mode == .waterfall {
            // The host calls it with the event's arguments.
            fields.append(("terminal", .function(SessionFunction { args in try await terminal(args) })))
        }
        return .record(fields)
    }

    /// Sends an `emit` through the host; `lock` is held, and released. The
    /// emitter neither waits for nor cancels it.
    private func emitViaHost(_ row: Row, _ name: String, _ args: [Value], keep: ((Rutis.Advanced.Listener) async throws -> Bool)?) throws {
        guard let session else {
            lock.unlock()
            return
        }
        let record = record(row, name, .emit, args, keep: keep, terminal: nil)
        let answered: @Sendable (Error?) -> Void = { error in
            if let error {
                Events.report("rutis: emit of \(name) from row \(row.name) failed: \(Events.failure(error))", to: row.settings.log)
            }
        }
        let taken: @Sendable (Result<Value, Error>) -> Void = { result in
            switch result {
            case let .failure(error):
                answered(error)
            case let .success(.remoteFuture(future)):
                // The host queued this emit behind one already running in
                // the row, and answered with a future: its settlement.
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
        if case let .failure(error) = sent.get() { throw error }
    }

    /// Sends `events.dispatch`; `lock` is held, and released. The frame is
    /// queued behind this process's earlier frames, such as the events.on
    /// of a listener registered before (§4.4).
    private func dispatchViaHost(_ row: Row, _ name: String, _ mode: Mode, _ args: [Value], keep: ((Rutis.Advanced.Listener) async throws -> Bool)?,
                                 terminal: (([Value]) async throws -> Value)?) async throws -> Value
    {
        guard let session else {
            lock.unlock()
            throw RutisError.hostGone()
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
    private func selector(_ keep: @escaping (Rutis.Advanced.Listener) async throws -> Bool) -> SessionFunction {
        SessionFunction { args in
            let descriptors = try Rutis._Generated.Arguments(args).decode(0, as: [Rutis.Advanced.Listener]?.self) ?? []
            var answers: [JSON] = []
            for descriptor in descriptors { answers.append(.bool(try await keep(descriptor))) }
            return .data(.array(answers))
        }
    }

    // ── From the host ────────────────────────────────────────────────

    /// Takes the host's event calls on the reading thread, in frame order
    /// (§4.1, §5.5); the rest runs as any call does.
    func arrive(_ method: String, _ args: [Value]) -> (() async throws -> Value)? {
        method == "events.call" ? eventCall(args) : nil
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
        if mode == .waterfall {
            guard case let .remoteFunction(far) = record["next"] ?? .undefined else {
                return failed(RutisError(name: "Error", message: "events.call: a \(modeText) needs next"))
            }
            next = { try await settle(try await far.call([], sync: false)) }
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
