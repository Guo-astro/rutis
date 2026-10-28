import Foundation

/// Why a host that does not declare events-v2 is refused.
let oldEventsHost = "this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later"

let serveUsage = "usage: <binary> --rutis-manifest | <channel> --id <endpoint> [--peer <endpoint>] <project>"

/// A session this side refused at its handshake (an old host): the runtime
/// stops, and does not retry.
struct Refused: Error, CustomStringConvertible {
    let reason: String
    /// Its line is written already.
    var said = false
    var description: String { "refused: incompatible (\(reason)); stopping" }
}

extension Rutis {
    /// Runs `plugins` as a rutis runtime, as the command line says, and
    /// exits the process: with `--rutis-manifest` it prints what the plugins
    /// declare; otherwise it serves the session on the channel it is given
    /// until the session ends. Plugins are registered only here.
    public static func serve(_ plugins: Definition...) -> Never {
        do {
            try serveArguments(Array(CommandLine.arguments.dropFirst()), plugins)
            exit(0)
        } catch {
            if (error as? Refused)?.said != true {
                FileHandle.standardError.write(Data("rutis: \(error is Refused ? "\(error)" : errorMessage(error))\n".utf8))
            }
            exit(1)
        }
    }

    /// `Advanced.serveArgs`.
    static func serveArguments(_ args: [String], _ plugins: [Definition]) throws {
        try check(plugins)
        if args == ["--rutis-manifest"] {
            FileHandle.standardOutput.write(Data(manifestText(plugins).utf8 + [0x0A]))
            return
        }
        let (channel, endpoint, project) = try parse(args)
        Responsibility.claim()
        if channel.hasPrefix("listen:") || channel.hasPrefix("dial:") {
            #if canImport(Network)
            let remote = String(channel.drop(while: { $0 != ":" }).dropFirst())
            try block { try await serveRemote(channel, remote, endpoint: endpoint, project: project, plugins) }
            return
            #else
            throw RutisError(name: "Error", message: "remote channels need Network.framework: \(channel)")
            #endif
        }
        if channel.hasPrefix("ws:") || channel.hasPrefix("wss:") {
            throw RutisError(name: "Error", message: "a WebSocket channel is listen:<address> or dial:<address>: \(channel)")
        }
        try serve(channel: try Channels.open(channel), endpoint: endpoint, project: project, plugins)
    }

    /// Serves `plugins` on `channel` (one message per line, greeting as
    /// `endpoint`) until the session ends, then unloads them.
    static func serve(channel: Channel, endpoint: Endpoint, project: String, _ plugins: [Definition]) throws {
        try block { try await serveSession(channel: channel, endpoint: endpoint, project: project, plugins) }
    }

    /// Runs `body` in a Task of its own, blocking this thread until it ends.
    static func block(_ body: @escaping @Sendable () async throws -> Void) throws {
        let ended = ResultEvent()
        Task.detached {
            do {
                try await body()
                ended.finish(nil)
            } catch {
                ended.finish(error)
            }
        }
        if let error = ended.block() { throw error }
    }

    /// `started` sees the session once it starts (RutisTest reads the
    /// plugin definitions it exports).
    package static func serveSession(channel: Channel, endpoint: Endpoint, project: String, _ plugins: [Definition],
                                     started: ((Peer) -> Void)? = nil) async throws
    {
        let (session, runtime) = try startSession(channel: channel, project: project, plugins, endpoint: endpoint)
        started?(session)
        await session.closed()
        if let ended = session.error as? SessionEnded, ended.kind == .refused {
            channel.close()
            throw Refused(reason: ended.reason)
        }
        try await end(session, runtime)
    }

    #if canImport(Network)
    /// Serves `plugins` on `listen:<remote>` or `dial:<remote>` as an app
    /// would, its settings from the environment: RUTIS_TOKEN (spent once
    /// read when dialing), RUTIS_CA, RUTIS_CERT and RUTIS_KEY; SIGTERM and
    /// Ctrl-C stop it. Returns once stopped; an incompatible rutis throws
    /// (status 1), as do cleanups that did not finish.
    static func serveRemote(_ channel: String, _ remote: String, endpoint: Endpoint, project: String,
                            _ plugins: [Definition]) async throws
    {
        let environment = ProcessInfo.processInfo.environment
        let token = environment["RUTIS_TOKEN"] ?? ""
        let mode: RutisApp.Mode
        if channel.hasPrefix("listen:") {
            let (cert, key) = (environment["RUTIS_CERT"] ?? "", environment["RUTIS_KEY"] ?? "")
            mode = .listen(remote, identity: {
                guard !cert.isEmpty, !key.isEmpty else {
                    throw ChannelError("\(remote): wss needs a certificate and key (RUTIS_CERT, RUTIS_KEY)")
                }
                return try tlsIdentity(cert: cert, key: key)
            })
        } else {
            // Spent once read: what this process starts does not inherit it.
            unsetenv("RUTIS_TOKEN")
            let ca = environment["RUTIS_CA"] ?? ""
            mode = .dial(remote, trust: remote.hasPrefix("wss:") && !ca.isEmpty ? try caCertificates(ca) : [])
        }
        let app = RutisApp(plugins: plugins, mode: mode, id: endpoint.local, peer: endpoint.expected, token: token,
                           project: project, settings: Events.Settings())
        let stop = StopSignal()
        let sources = onTermination(stop)
        defer { sources.forEach { $0.cancel() } }
        try await app.start()
        Task { await stop.wait(); await app.stop() }
        switch await app.ended() {
        case let .refused(reason): throw Refused(reason: reason, said: true)
        case let .failed(error): throw error
        case .stopped, nil: return
        }
    }
    #endif

    /// Starts a session serving `plugins`: a host older than 0.13 is
    /// refused right after its handshake.
    static func startSession(channel: Channel, project: String, _ plugins: [Definition], endpoint: Endpoint,
                             settings: Events.Settings = Events.Settings()) throws -> (Peer, Runtime)
    {
        let runtime = Runtime(plugins, project: project, settings: settings)
        var options = PeerOptions(endpoint: endpoint)
        // Weak: a frame that arrives once the runtime is gone finds it gone.
        options.dispatch = { [weak runtime] target, method, args in
            guard let runtime else { throw RutisError(name: "Error", message: "runtime is closed") }
            return try await runtime.dispatch(target, method, args)
        }
        options.arrive = { [weak runtime] method, args in runtime?.arrive(method, args) }
        options.require = [eventsCapability]
        options.refuse = { _ in oldEventsHost }
        let session = try Peer(channel: channel, options: options)
        runtime.session = session
        try session.start()
        return (session, runtime)
    }

    /// Ends a session's lease: every row goes, within the shutdown
    /// deadline. It fails when cleanups were given up.
    static func end(_ session: Peer, _ runtime: Runtime) async throws {
        session.close()
        runtime.lock.withLock { runtime.closing = true }
        let budget = runtime.settings.shutdownTimeout
        if await !runtime.dispose(within: budget) {
            throw RutisError(name: "Error", message: "cleanups did not finish within \(Events.seconds(budget))s after the host disconnected")
        }
    }

    static func check(_ plugins: [Definition]) throws {
        guard !plugins.isEmpty else { throw RutisError(name: "Error", message: "no plugins to serve") }
        var seen = Set<String>()
        for plugin in plugins where !seen.insert(plugin.name).inserted {
            throw RutisError(name: "Error", message: "two plugins are named \(plugin.name)")
        }
    }

    /// `Advanced.manifest`.
    static func manifestText(_ plugins: [Definition]) -> String {
        JSONWriter.string(.object([
            ("manifest", .int(1)),
            ("sdk", .string(Advanced.sdkVersion)),
            ("pluginApi", .int(Advanced.pluginAPI)),
            ("runtime", .string(marker)),
            ("platform", .string(platform)),
            ("plugins", .object(plugins.sorted { $0.name < $1.name }.map { ($0.name, $0.described) })),
        ]))
    }

    static var platform: String {
        #if arch(arm64)
        return "macos/arm64"
        #elseif arch(x86_64)
        return "macos/amd64"
        #else
        return "macos/unknown"
        #endif
    }

    /// The channel, the endpoint `--id` names (a runtime, its controller
    /// managing its rows; required on every channel) and the project. What
    /// retrying cannot fix fails here, before any dial.
    static func parse(_ args: [String]) throws -> (channel: String, endpoint: Endpoint, project: String) {
        guard let channel = args.first else { throw RutisError(name: "Error", message: serveUsage) }
        var rest = Array(args.dropFirst())
        var flags: [String: String] = [:]
        while let flag = rest.first, flag.hasPrefix("--") {
            guard rest.count >= 2 else { throw RutisError(name: "Error", message: "\(flag) needs a value") }
            guard flag == "--id" || flag == "--peer" else {
                throw RutisError(name: "Error", message: "unknown flag \(flag); \(serveUsage)")
            }
            flags[String(flag.dropFirst(2))] = rest[1]
            rest.removeFirst(2)
        }
        guard rest.count == 1 else { throw RutisError(name: "Error", message: serveUsage) }
        let network = channel.hasPrefix("listen:") || channel.hasPrefix("dial:")
        guard let id = flags["id"], !id.isEmpty else {
            throw RutisError(name: "Error", message: network
                ? "a network channel needs --id <endpoint>: \(channel)"
                : "a local channel needs --id <endpoint>: \(channel); a host older than rutis 0.19 started this runtime: upgrade the host")
        }
        guard validEndpointID(id) else {
            throw RutisError(name: "Error", message: "invalid endpoint id \(id): use lower-case letters, digits and -")
        }
        let endpoint = Endpoint(local: id, expected: flags["peer"], declare: ["runtime"])
        #if canImport(Network)
        if channel.hasPrefix("dial:") {
            do {
                _ = try dialHost(String(channel.dropFirst("dial:".count)))
            } catch {
                throw RutisError(name: "Error", message: "\(error)")
            }
        }
        #endif
        return (channel, endpoint, rest[0])
    }
}

/// A one-shot result a thread can block on.
extension Rutis.Advanced {
    /// `Rutis.serve` with the arguments given, returning instead of
    /// exiting: for binaries with commands of their own.
    public static func serveArgs(_ args: [String], _ plugins: [Rutis.Definition]) throws {
        try Rutis.serveArguments(args, plugins)
    }

    /// What `plugins` declare, as `--rutis-manifest` prints it.
    public static func manifest(_ plugins: [Rutis.Definition]) -> String {
        Rutis.manifestText(plugins)
    }
}

final class ResultEvent: @unchecked Sendable {
    private let semaphore = DispatchSemaphore(value: 0)
    private let box = ErrorBox()

    func finish(_ error: Error?) {
        if let error { box.set(error) }
        semaphore.signal()
    }

    func block() -> Error? {
        semaphore.wait()
        return box.get()
    }
}
