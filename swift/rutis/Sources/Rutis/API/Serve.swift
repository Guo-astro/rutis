import Foundation

/// Why a host that does not declare events-v2 is refused.
let oldEventsHost = "this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later"

let serveUsage = "usage: <binary> --rutis-manifest | <channel> [--id <endpoint>] [--peer <endpoint>] <project>"

/// A session this side refused at its handshake (an old host): the runtime
/// stops, and does not retry.
struct Refused: Error, CustomStringConvertible {
    let reason: String
    var description: String { "refused: incompatible (\(reason)); stopping" }
}

extension Rutis {
    /// Runs `plugins` as a rutis runtime, as the command line says, and
    /// exits the process: with `--rutis-manifest` it prints what the plugins
    /// declare; otherwise it serves the session on the channel it is given
    /// until the session ends. Plugins are registered only here.
    public static func serve(_ plugins: Definition...) -> Never {
        do {
            try serveArgs(Array(CommandLine.arguments.dropFirst()), plugins)
            exit(0)
        } catch {
            FileHandle.standardError.write(Data("rutis: \(error is Refused ? "\(error)" : errorMessage(error))\n".utf8))
            exit(1)
        }
    }

    /// `serve` with the arguments given, returning instead of exiting: for
    /// binaries with commands of their own.
    public static func serveArgs(_ args: [String], _ plugins: [Definition]) throws {
        try check(plugins)
        if args == ["--rutis-manifest"] {
            FileHandle.standardOutput.write(Data(manifest(plugins).utf8 + [0x0A]))
            return
        }
        let (channel, project) = try parse(args)
        Responsibility.claim()
        if channel.hasPrefix("listen:") || channel.hasPrefix("dial:") || channel.hasPrefix("ws:") || channel.hasPrefix("wss:") {
            throw RutisError(name: "Error", message: "remote channels (listen:, dial:) are not in this version of the Swift SDK: \(channel)")
        }
        try serve(channel: try Channels.open(channel), project: project, plugins)
    }

    /// Serves `plugins` on `channel` (the compat protocol, one message per
    /// line) until the session ends, then unloads them.
    static func serve(channel: Channel, project: String, _ plugins: [Definition]) throws {
        let ended = ResultEvent()
        Task.detached {
            do {
                try await serveSession(channel: channel, project: project, plugins)
                ended.finish(nil)
            } catch {
                ended.finish(error)
            }
        }
        if let error = ended.block() { throw error }
    }

    package static func serveSession(channel: Channel, project: String, _ plugins: [Definition]) async throws {
        let (session, runtime) = try startSession(channel: channel, project: project, plugins)
        await session.closed()
        if let ended = session.error as? SessionEnded, ended.kind == .refused {
            channel.close()
            throw Refused(reason: ended.reason)
        }
        try await end(session, runtime)
    }

    /// Starts a session serving `plugins`: a host older than 0.13 is
    /// refused right after its handshake.
    static func startSession(channel: Channel, project: String, _ plugins: [Definition]) throws -> (Peer, Runtime) {
        let runtime = Runtime(plugins, project: project)
        var options = PeerOptions()
        options.dispatch = { [unowned runtime] target, method, args in try await runtime.dispatch(target, method, args) }
        options.arrive = { [unowned runtime] method, args in runtime.arrive(method, args) }
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
        let budget = Events.shutdownTimeout
        if await !runtime.dispose(within: budget) {
            throw RutisError(name: "Error", message: "cleanups did not finish within \(Events.seconds(budget))s after the session ended")
        }
    }

    static func check(_ plugins: [Definition]) throws {
        guard !plugins.isEmpty else { throw RutisError(name: "Error", message: "no plugins to serve") }
        var seen = Set<String>()
        for plugin in plugins where !seen.insert(plugin.name).inserted {
            throw RutisError(name: "Error", message: "two plugins are named \(plugin.name)")
        }
    }

    /// What `plugins` declare, as `--rutis-manifest` prints it.
    public static func manifest(_ plugins: [Definition]) -> String {
        JSONWriter.string(.object([
            ("manifest", .int(1)),
            ("sdk", .string(sdkVersion)),
            ("pluginApi", .int(pluginAPI)),
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

    static func parse(_ args: [String]) throws -> (channel: String, project: String) {
        guard let channel = args.first else { throw RutisError(name: "Error", message: serveUsage) }
        var rest = Array(args.dropFirst())
        while let flag = rest.first, flag.hasPrefix("--") {
            guard rest.count >= 2 else { throw RutisError(name: "Error", message: "\(flag) needs a value") }
            guard flag == "--id" || flag == "--peer" else {
                throw RutisError(name: "Error", message: "unknown flag \(flag); \(serveUsage)")
            }
            rest.removeFirst(2)
        }
        guard rest.count == 1 else { throw RutisError(name: "Error", message: serveUsage) }
        return (channel, rest[0])
    }
}

/// A one-shot result a thread can block on.
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
