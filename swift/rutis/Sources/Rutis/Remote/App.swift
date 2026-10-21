// An app hosting plugins (design §7): the plugins are compiled into the
// app, rutis drives them, and the app only starts and stops the connection.
// `RutisApp` dials out to a rutis access point (recommended) or listens for
// rutis to dial in, by the rules every runtime follows (dialing:
// docs/design-remote-runtime-dial-2026-10-14.md §六; listening: design
// §6.1); a binary's `dial:` and `listen:` channels run on it too, their
// settings read from the environment instead (Serve.swift).
#if canImport(Network)
import Foundation
import Network
import Observation
import Security

/// Plugins compiled into an app, served to rutis while started.
///
/// ```swift
/// let rutis = RutisApp(
///     plugins: [health, device],
///     connect: "wss://rutis.example.com:7443/rutis",
///     id: "kim-iphone",
///     token: secrets.rutisToken)
/// try await rutis.start()   // connects; rutis then loads the plugins its configuration names
/// await rutis.stop()        // every plugin cleans up, latest first; then it disconnects
/// ```
///
/// While started it reconnects by itself whenever the connection is lost,
/// until `stop()`; every connection is new, and rutis loads the plugins
/// again. `state` says where it is, for the app to show: nothing here acts
/// on it, nor on the app going to the background (the app's business).
public final class RutisApp: Observable, @unchecked Sendable {
    /// Where the connection is.
    public enum State: Equatable, Sendable, CustomStringConvertible {
        /// Not started, stopped, or stopped by itself: `reason` says why
        /// then (rutis is incompatible, and retrying cannot fix that).
        case stopped(reason: String?)
        /// Dialing rutis, or waiting before dialing again; listening, rutis
        /// not connected.
        case connecting
        /// Connected: rutis loads, uses and unloads the plugins.
        case connected

        public var description: String {
            switch self {
            case .stopped(reason: nil): return "stopped"
            case let .stopped(reason: reason?): return "stopped (\(reason))"
            case .connecting: return "connecting"
            case .connected: return "connected"
            }
        }
    }

    enum Mode {
        case dial(String, trust: [SecCertificate])
        case listen(String, identity: () throws -> sec_identity_t)
    }

    /// How a run of the connection ended.
    enum Ending {
        /// `stop()`.
        case stopped
        /// rutis is incompatible: stopped by itself.
        case refused(String)
        /// Stopped, but the cleanups did not finish in time.
        case failed(Error)
    }

    /// A started connection.
    final class Run: @unchecked Sendable {
        let stop = StopSignal()
        var task: Task<Ending, Never>?
        /// Wakes the listening loop, to see the stop.
        var wake: (() -> Void)?
    }

    let plugins: [Rutis.Definition]
    let mode: Mode
    let id: String
    let peer: String?
    let token: String
    let project: String
    let settings: Events.Settings

    private let lock = Lock()
    private let registrar = ObservationRegistrar()
    private var current: State = .stopped(reason: nil)
    private var watchers: [UUID: AsyncStream<State>.Continuation] = [:]
    /// The run going on, and the latest one (going on or ended).
    private var run: Run?
    private var latest: Run?
    /// A `start()` is under way.
    private var starting = false

    /// Dials the rutis access point at `connect` (`wss://…`; `ws://` only
    /// on loopback) as `id` (lower-case letters, digits and `-`), presenting
    /// `token`.
    ///
    /// - Parameters:
    ///   - plugins: the plugins rutis may load.
    ///   - peer: the id rutis greets with; when given, another one is refused.
    ///   - trust: CA certificates trusted besides the system's.
    ///   - shutdownTimeout: how long the plugins' cleanups may take, in
    ///     seconds, once the connection ends.
    ///   - log: where the connection's and the plugins' lines go (stderr).
    public convenience init(plugins: [Rutis.Definition], connect: String, id: String, token: String,
                            peer: String? = nil, trust: [SecCertificate] = [], shutdownTimeout: Double = 10,
                            log: @escaping @Sendable (String) -> Void = RutisApp.standardError)
    {
        self.init(plugins: plugins, mode: .dial(connect, trust: trust), id: id, peer: peer, token: token,
                  project: Bundle.main.bundlePath,
                  settings: Events.Settings(log: log, shutdownTimeout: shutdownTimeout))
    }

    /// Listens on `listen` (`wss://0.0.0.0:7443/rutis`; `ws://` only on
    /// loopback) for rutis to dial in as `id`, presenting `token`; one
    /// connection at a time, a newer one taking over. For a device at a
    /// fixed address: dialing out is the usual way.
    ///
    /// - Parameters:
    ///   - certificate: the TLS identity of a `wss://` address.
    ///   - The others: as dialing.
    public convenience init(plugins: [Rutis.Definition], listen: String, id: String, token: String,
                            certificate: SecIdentity? = nil, peer: String? = nil, shutdownTimeout: Double = 10,
                            log: @escaping @Sendable (String) -> Void = RutisApp.standardError)
    {
        let identity = { () throws -> sec_identity_t in
            guard let certificate, let identity = sec_identity_create(certificate) else {
                throw ChannelError("\(listen): wss needs a certificate (RutisApp's certificate:)")
            }
            return identity
        }
        self.init(plugins: plugins, mode: .listen(listen, identity: identity), id: id, peer: peer, token: token,
                  project: Bundle.main.bundlePath,
                  settings: Events.Settings(log: log, shutdownTimeout: shutdownTimeout))
    }

    init(plugins: [Rutis.Definition], mode: Mode, id: String, peer: String?, token: String, project: String,
         settings: Events.Settings)
    {
        self.plugins = plugins
        self.mode = mode
        self.id = id
        self.peer = peer
        self.token = token
        self.project = project
        self.settings = settings
    }

    /// Writes `line` on stderr: the default `log`.
    @Sendable public static func standardError(_ line: String) {
        Events.standardError(line)
    }

    // ── State ────────────────────────────────────────────────────────

    /// Where the connection is: observable (`@Observable`'s observation,
    /// SwiftUI), and as `states`.
    public var state: State {
        registrar.access(self, keyPath: \.state)
        return lock.withLock { current }
    }

    /// The state now, then every change, until the stream is dropped.
    public var states: AsyncStream<State> {
        AsyncStream { continuation in
            let key = UUID()
            lock.withLock {
                watchers[key] = continuation
                continuation.yield(current)
            }
            continuation.onTermination = { [weak self] _ in
                guard let self else { return }
                _ = lock.withLock { self.watchers.removeValue(forKey: key) }
            }
        }
    }

    private func publish(_ state: State) {
        guard lock.withLock({ current != state }) else { return }
        registrar.withMutation(of: self, keyPath: \.state) {
            lock.withLock {
                current = state
                for watcher in watchers.values { watcher.yield(state) }
            }
        }
    }

    private func say(_ line: String) { settings.log("rutis: \(line)") }

    // ── Start, stop ──────────────────────────────────────────────────

    /// Starts serving the plugins: dialing rutis, or listening for it.
    /// Returns once dialing has started or the address is bound, and
    /// throws what retrying cannot fix (an invalid id or address, no
    /// certificate for `wss://`, an address taken); a connection that
    /// fails is retried, as `state` shows. Nothing when started already.
    public func start() async throws {
        let first: Bool = lock.withLock {
            if run != nil || starting { return false }
            starting = true
            return true
        }
        guard first else { return }
        defer { lock.withLock { starting = false } }
        try Rutis.check(plugins)
        guard validEndpointID(id) else {
            throw RutisError(name: "Error", message: "invalid endpoint id \(id): use lower-case letters, digits and -")
        }
        let started = Run()
        let body: @Sendable () async -> Ending
        switch mode {
        case let .dial(spec, trust):
            do {
                _ = try dialHost(spec)
            } catch {
                throw RutisError(name: "Error", message: "\(error)")
            }
            body = { [self] in await dial(spec, anchors: spec.hasPrefix("wss:") ? trust : [], stop: started.stop) }
        case let .listen(spec, identity):
            let listener = try WebSocketListener(spec, protocol: "rutis.\(endpointProtocol)", token: token, identity: identity)
            let (events, continuation) = AsyncStream<ListenEvent>.makeStream()
            let address = try listener.start { socket in continuation.yield(.accepted(socket)) }
            say("listening on \(address)")
            started.wake = { continuation.yield(.stop) }
            body = { [self] in await listen(listener, events, continuation, stop: started.stop) }
        }
        publish(.connecting)
        lock.withLock {
            run = started
            latest = started
            started.task = Task.detached { [self] in
                let ending = await body()
                lock.withLock { if run === started { run = nil } }
                if case let .refused(reason) = ending {
                    publish(.stopped(reason: reason))
                } else {
                    publish(.stopped(reason: nil))
                }
                return ending
            }
        }
    }

    /// Stops: every plugin cleans up (latest first, rutis still connected
    /// to hear it), then the connection closes, and nothing reconnects.
    /// Returns once done; `start()` may follow.
    public func stop() async {
        _ = await stopped()
    }

    /// Stops; how the run ended (nil: not started).
    func stopped() async -> Ending? {
        guard let started = lock.withLock({ run }) else { return nil }
        started.stop.stop()
        started.wake?()
        return await started.task?.value
    }

    /// Waits for the latest run to end by itself or be stopped; how it
    /// ended (nil: never started).
    func ended() async -> Ending? {
        guard let started = lock.withLock({ latest }) else { return nil }
        return await started.task?.value
    }

    // ── Dialing ──────────────────────────────────────────────────────

    /// Dials `spec` and stays connected, by the rules, until stopped or
    /// refused as incompatible.
    private func dial(_ spec: String, anchors: [SecCertificate], stop: StopSignal) async -> Ending {
        let endpoint = Endpoint(local: id, expected: peer, declare: ["runtime"])
        let retry = Retry()
        var backoff = Backoff(retry)
        let address = "\(spec) as \(id)"
        let protocolName = "rutis.\(endpointProtocol)"
        while true {
            var failure = DialFailure.retryable
            var reason = ""
            var lost = "cannot connect"
            publish(.connecting)
            do {
                guard let socket = try await stop.race({ [token] in
                    try await DialedSocket.dial(spec, protocol: protocolName, token: token, anchors: anchors, timeout: retry.handshake)
                }) else { return .stopped }
                let (peer, runtime) = try Rutis.startSession(channel: socket, project: project, plugins, endpoint: endpoint,
                                                             settings: settings)
                let session = ControllerSession(peer: peer, runtime: runtime, socket: socket)
                // The greeting, within the handshake's time.
                let late = StopSignal()
                DispatchQueue.global().asyncAfter(deadline: .now() + retry.handshake) { late.stop() }
                let greeted = try await stop.race({ () -> Bool in
                    try await late.race({ await peer.ready(); return true }) ?? false
                })
                guard let greeted else { return await Self.stop(session) }
                if !greeted {
                    peer.close(ChannelError("the access point did not greet in time"))
                    socket.close("no greeting")
                } else if peer.error == nil {
                    say("connected to \(address)")
                    publish(.connected)
                    let since = Date()
                    guard try await stop.race({ await peer.closed(); return true }) != nil else {
                        return await Self.stop(session)
                    }
                    if Date().timeIntervalSince(since) >= retry.stable { backoff.reset() }
                }
                await peer.closed()
                publish(.connecting)
                let ended = peer.error ?? ChannelError("the session ended")
                (failure, reason, lost) = (sessionFailure(ended, closeCode: socket.closeCode), "\(ended)", "connection lost")
                if let refused = session.refused { failure = .incompatible; reason = refused.reason }
                await session.end()
            } catch let error as DialError {
                (failure, reason) = (error.failure, error.reason)
            } catch {
                reason = "\(error)"
            }
            let wait: Double
            switch failure {
            case .incompatible:
                say("\(Refused(reason: reason))")
                return .refused(reason)
            case .authRejected:
                wait = retry.rejected
                say("refused: auth-rejected (\(reason)); retrying in \(stateSeconds(wait))")
            case .retryable:
                wait = backoff.wait()
                say("\(lost) (\(reason)); retrying in \(stateSeconds(wait))")
            }
            let slept: Bool?? = try? await stop.race { try await Task.sleep(nanoseconds: UInt64(wait * 1e9)); return true }
            guard slept == .some(.some(true)) else { return .stopped }
        }
    }

    /// Stops `session` for good: its rows go while rutis still hears.
    private static func stop(_ session: ControllerSession) async -> Ending {
        do {
            try await session.stop()
            return .stopped
        } catch {
            return .failed(error)
        }
    }

    // ── Listening ────────────────────────────────────────────────────

    /// Serves one controller at a time on `listener`, until stopped or a
    /// controller is refused (an old host).
    private func listen(_ listener: WebSocketListener, _ events: AsyncStream<ListenEvent>,
                        _ continuation: AsyncStream<ListenEvent>.Continuation, stop: StopSignal) async -> Ending
    {
        let endpoint = Endpoint(local: id, expected: peer, declare: ["runtime"])
        defer {
            listener.stop()
            continuation.finish()
        }
        if stop.isStopped { return .stopped }
        var current: (id: Int, session: ControllerSession)?
        var next = 0
        for await event in events {
            switch event {
            case .stop:
                guard let (_, session) = current else { return .stopped }
                return await Self.stop(session)
            case let .accepted(socket):
                if stop.isStopped {
                    socket.closeWith(CloseCode.goingAway, "the runtime stopped")
                    continue
                }
                if let (_, session) = current {
                    (session.socket as? WebSocket)?.closeWith(CloseCode.replaced, "replaced by a new connection")
                    await session.end()
                    current = nil
                }
                do {
                    let (peer, runtime) = try Rutis.startSession(channel: socket, project: project, plugins, endpoint: endpoint,
                                                                 settings: settings)
                    next += 1
                    let id = next
                    current = (id, ControllerSession(peer: peer, runtime: runtime, socket: socket))
                    publish(.connected)
                    Task {
                        await peer.closed()
                        continuation.yield(.closed(id))
                    }
                } catch {
                    socket.closeWith(CloseCode.goingAway, "\(error)")
                }
            case let .closed(id):
                guard let (currentID, session) = current, currentID == id else { continue }
                await session.end()
                current = nil
                publish(.connecting)
                if let refused = session.refused {
                    say("\(refused)")
                    return .refused(refused.reason)
                }
            }
        }
        return .stopped
    }
}

enum ListenEvent {
    case accepted(WebSocket)
    case closed(Int)
    case stop
}
#endif
