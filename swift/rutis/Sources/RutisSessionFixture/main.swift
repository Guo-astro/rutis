// The session conformance target (rutis_bridge::session::testing::session),
// served by the Swift session: connects to the channel given as the first
// argument (a Unix socket path, or `tcp:<host>:<port>`, presenting the token
// in RUTIS_CHANNEL_TOKEN first) as endpoint `swift`, expecting `main`, and
// serves `conformance` until the session ends.
import Foundation
import Rutis

final class Conformance: @unchecked Sendable {
    let lock = Lock()
    var held: FarFunction?
    var kept: [Int64: FarFunction] = [:]
    var started = false
    var aborted = false
    weak var session: Peer?

    func failure(_ message: String) -> RutisError { RutisError(name: "Error", message: message) }

    func dispatch(_ target: String, _ method: String, _ args: [Value]) async throws -> Value {
        guard target == "conformance" else { throw failure("no target \(target)") }
        func arg(_ i: Int) -> Value { i < args.count ? args[i] : .undefined }
        func function(_ i: Int) throws -> FarFunction {
            guard case let .remoteFunction(f) = arg(i) else { throw failure("\(method) needs a function") }
            return f
        }
        func integer(_ i: Int) throws -> Int64 {
            guard case let .data(json) = arg(i), let n = json.int64Value else { throw failure("\(method) needs an integer") }
            return n
        }
        switch method {
        case "echo":
            return arg(0)
        case "apply":
            return try await function(0).call([arg(1)], sync: true)
        case "later":
            try await Task.sleep(nanoseconds: 5_000_000)
            return arg(0)
        case "fail":
            throw RutisError(name: arg(0).string ?? "Error", message: arg(1).string ?? "")
        case "hold":
            let f = try function(0)
            lock.lock()
            held = f
            lock.unlock()
            return .undefined
        case "fire":
            lock.lock()
            let f = held
            lock.unlock()
            guard let f else { throw RutisError(name: "LookupError", message: "nothing held") }
            return try await f.call([arg(0)], sync: true)
        case "drop":
            lock.lock()
            let f = held
            held = nil
            lock.unlock()
            f?.release()
            return .undefined
        case "keep":
            let slot = try integer(0)
            let f = try function(1)
            lock.lock()
            kept[slot] = f
            lock.unlock()
            return .undefined
        case "free":
            let slot = try integer(0)
            lock.lock()
            let f = kept.removeValue(forKey: slot)
            lock.unlock()
            f?.release()
            return .undefined
        case "make":
            return .function(SessionFunction { args in args.first ?? .undefined })
        case "tables":
            let tables = session?.referenceTables ?? (exports: 0, imports: 0)
            return .data(.object([("exports", .int(tables.exports)), ("imports", .int(tables.imports))]))
        case "abortable":
            guard case let .signal(signal) = arg(0) else { throw failure("abortable needs a signal") }
            lock.lock()
            started = true
            lock.unlock()
            await signal.wait()
            lock.lock()
            aborted = true
            lock.unlock()
            return .undefined
        case "started":
            lock.lock()
            defer { lock.unlock() }
            return .data(.bool(started))
        case "aborted":
            lock.lock()
            defer { lock.unlock() }
            return .data(.bool(aborted))
        // The value one level deeper, returned or passed to a callback: past
        // the depth limit, this side's call or reply fails (#279).
        case "wrap":
            return .list([arg(0)])
        case "applyWrapped":
            return try await function(0).call([.list([arg(1)])], sync: true)
        case "reenter":
            return try await function(0).call([], sync: true)
        default:
            throw failure("no method \(method)")
        }
    }
}

let arguments = CommandLine.arguments
guard arguments.count >= 2 else {
    FileHandle.standardError.write("usage: rutis-session-fixture <socket path | tcp:host:port>\n".data(using: .utf8)!)
    exit(2)
}
do {
    let channel = try Channels.open(arguments[1].hasPrefix("tcp:") ? arguments[1] : "unix:" + arguments[1])
    let service = Conformance()
    var options = PeerOptions()
    options.endpoint = Endpoint(local: "swift", expected: "main")
    options.implementation = [("name", "conformance"), ("version", "0")]
    options.capabilities = runtimeCapabilities
    options.dispatch = { target, method, args in try await service.dispatch(target, method, args) }
    let session = try Peer(channel: channel, options: options)
    service.session = session
    try session.start()
    Event.blockUntil { await session.closed() }
} catch {
    FileHandle.standardError.write("rutis-session-fixture: \(error)\n".data(using: .utf8)!)
    exit(1)
}
