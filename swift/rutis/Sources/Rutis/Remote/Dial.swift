// `dial:ws://…` / `dial:wss://…`: the runtime connects out to a rutis
// access point and stays connected, by the rules every runtime follows
// (docs/design-remote-runtime-dial-2026-10-14.md §六; Go's dial.go):
//
// A failure is retryable (backoff from 0.5 s, doubling up to 30 s, ± 20 %,
// reset by a session that lasted 60 s), an authentication failure (again
// every 30 s: 401 / 403 at the upgrade, a server certificate this side
// refuses, the access point rejecting this id with close code 4003 after
// the hello, or greeting as another endpoint), or incompatible (stop, exit
// status 1: 400 / 404 / 405 / 426 at the upgrade, another session
// protocol, a greeting this side cannot speak). Every session is new: its
// rows go when it ends. SIGTERM and Ctrl-C unload the rows, close, and end
// with status 0. The connection is URLSession's WebSocket task (design
// §17.3 #12): the status of a refused upgrade is its response's.
#if canImport(Network)
import Foundation
import Security

/// What a failed dial or an ended session means for retrying.
enum DialFailure: String {
    case retryable
    case authRejected = "auth-rejected"
    case incompatible
}

struct DialError: Error, CustomStringConvertible {
    let failure: DialFailure
    let reason: String
    var description: String { reason }
}

/// Reconnection: rutis-bridge's `Retry` defaults.
struct Retry {
    var initial = 0.5
    var max = 30.0
    var jitter = 0.2
    var stable = 60.0
    var rejected = 30.0
    var handshake = 10.0

    /// The defaults, unless tests shorten them
    /// (`RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>`).
    init(_ configured: String? = ProcessInfo.processInfo.environment["RUTIS_RETRY"]) {
        guard let configured else { return }
        let parts = configured.split(separator: ",").map { Int($0.trimmingCharacters(in: .whitespaces)) }
        guard parts.count == 4, parts.allSatisfy({ ($0 ?? -1) >= 0 }) else { return }
        let ms = parts.map { Double($0!) / 1000 }
        (initial, max, stable, rejected) = (ms[0], ms[1], ms[2], ms[3])
    }
}

/// Exponential backoff with jitter.
struct Backoff {
    let retry: Retry
    private(set) var next: Double
    var random: () -> Double = { Double.random(in: 0..<1) }

    init(_ retry: Retry) {
        self.retry = retry
        next = retry.initial
    }

    mutating func reset() { next = retry.initial }

    /// The next wait: the current step ± jitter; the step doubles up to the
    /// maximum.
    mutating func wait() -> Double {
        let base = next
        next = Swift.min(next * 2, retry.max)
        return base * (1 + retry.jitter * (2 * random() - 1))
    }
}

/// `wait` for a state line: 0.5s, 1s, 30s.
func stateSeconds(_ wait: Double) -> String {
    let tenths = (wait * 10).rounded() / 10
    return tenths == tenths.rounded() ? "\(Int(tenths))s" : "\(tenths)s"
}

/// The host of `spec`, an access point to dial: an error unless it is a
/// ws:// or wss:// address; a non-loopback one needs wss://.
func dialHost(_ spec: String) throws -> String {
    guard let url = URLComponents(string: spec), let scheme = url.scheme, scheme == "ws" || scheme == "wss",
          var host = url.host, !host.isEmpty
    else {
        throw ChannelError("\(spec) is not a ws:// or wss:// address")
    }
    if host.hasPrefix("["), host.hasSuffix("]") { host = String(host.dropFirst().dropLast()) }
    if scheme == "ws", !isLoopback(host) { throw ChannelError("\(spec): a non-loopback address needs wss://") }
    return host
}

func isLoopback(_ host: String) -> Bool {
    if host == "localhost" || host == "::1" { return true }
    let parts = host.split(separator: ".")
    return parts.count == 4 && parts[0] == "127" && parts.allSatisfy { UInt8($0) != nil }
}

/// The CA certificates in the PEM file `path` (RUTIS_CA).
func caCertificates(_ path: String) throws -> [SecCertificate] {
    guard let text = try? String(contentsOfFile: path, encoding: .utf8) else {
        throw ChannelError("RUTIS_CA \(path): cannot read it")
    }
    var certificates: [SecCertificate] = []
    var rest = Substring(text)
    while let begin = rest.range(of: "-----BEGIN CERTIFICATE-----"),
          let end = rest.range(of: "-----END CERTIFICATE-----", range: begin.upperBound..<rest.endIndex)
    {
        let body = rest[begin.upperBound..<end.lowerBound].filter { !$0.isWhitespace }
        if let data = Data(base64Encoded: String(body)), let certificate = SecCertificateCreateWithData(nil, data as CFData) {
            certificates.append(certificate)
        }
        rest = rest[end.upperBound...]
    }
    guard !certificates.isEmpty else { throw ChannelError("RUTIS_CA \(path): no PEM certificate in it") }
    return certificates
}

/// A dialed WebSocket: URLSession's task as the session's channel.
final class DialedSocket: NSObject, Channel, URLSessionWebSocketDelegate, @unchecked Sendable {
    private let lock = Lock()
    private var session: URLSession!
    private var task: URLSessionWebSocketTask!
    private let host: String
    private let anchors: [SecCertificate]
    private var opened: CheckedContinuation<Void, Error>?
    private var untrusted = false
    private var shut = false
    private var lastSeen = Date()
    private var heart: DispatchSourceTimer?
    /// The code of the far end's close frame, once it closed with one.
    private var farCode: Int?
    private var farReason = ""

    private init(host: String, anchors: [SecCertificate]) {
        self.host = host
        self.anchors = anchors
    }

    /// Dials the access point at `spec`, presenting `token` and speaking
    /// `protocol`; a failure is a `DialError`, classified by its status or
    /// type, never its text.
    static func dial(_ spec: String, protocol protocolName: String, token: String, anchors: [SecCertificate],
                     timeout: Double) async throws -> DialedSocket
    {
        let host: String
        do {
            host = try dialHost(spec)
        } catch {
            throw DialError(failure: .incompatible, reason: "\(error)")
        }
        let socket = DialedSocket(host: host, anchors: anchors)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = timeout
        socket.session = URLSession(configuration: configuration, delegate: socket, delegateQueue: nil)
        var request = URLRequest(url: URL(string: spec)!, timeoutInterval: timeout)
        if !token.isEmpty { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        request.setValue(protocolName, forHTTPHeaderField: "Sec-WebSocket-Protocol")
        let task = socket.session.webSocketTask(with: request)
        task.maximumMessageSize = maxMessage
        socket.task = task
        do {
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                    socket.lock.withLock { socket.opened = continuation }
                    task.resume()
                }
            } onCancel: {
                task.cancel()
            }
        } catch {
            socket.session.invalidateAndCancel()
            throw error
        }
        socket.startHeartbeat()
        return socket
    }

    func urlSession(_: URLSession, webSocketTask _: URLSessionWebSocketTask, didOpenWithProtocol chosen: String?) {
        let continuation = lock.withLock { () -> CheckedContinuation<Void, Error>? in
            defer { opened = nil }
            return opened
        }
        let protocolName = task.originalRequest?.value(forHTTPHeaderField: "Sec-WebSocket-Protocol") ?? ""
        if chosen == protocolName {
            continuation?.resume()
            return
        }
        task.cancel(with: .protocolError, reason: Data("no session protocol".utf8))
        let said = chosen.map { "the access point chose \"\($0)\"" } ?? "the access point chose none"
        continuation?.resume(throwing: DialError(failure: .incompatible, reason: "\(host): session protocol: \(said)"))
    }

    func urlSession(_: URLSession, webSocketTask _: URLSessionWebSocketTask, didCloseWith code: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        lock.withLock {
            farCode = code.rawValue
            farReason = reason.map { String(decoding: $0, as: UTF8.self) } ?? ""
        }
    }

    func urlSession(_: URLSession, task completed: URLSessionTask, didCompleteWithError error: Error?) {
        let continuation = lock.withLock { () -> CheckedContinuation<Void, Error>? in
            defer { opened = nil }
            return opened
        }
        guard let continuation else { return }
        continuation.resume(throwing: classify(completed, error))
    }

    /// Why the upgrade failed, for retrying.
    private func classify(_ task: URLSessionTask, _ error: Error?) -> DialError {
        if let response = task.response as? HTTPURLResponse, response.statusCode != 101 {
            let reason = "\(host): upgrade refused with \(response.statusCode)"
            switch response.statusCode {
            case 401, 403: return DialError(failure: .authRejected, reason: reason)
            case 400, 404, 405, 426: return DialError(failure: .incompatible, reason: reason)
            default: return DialError(failure: .retryable, reason: reason)
            }
        }
        let nsError = error as NSError?
        let certificate = [NSURLErrorServerCertificateUntrusted, NSURLErrorServerCertificateHasBadDate,
                           NSURLErrorServerCertificateHasUnknownRoot, NSURLErrorServerCertificateNotYetValid,
                           NSURLErrorSecureConnectionFailed]
        if lock.withLock({ untrusted }) || (nsError.map { $0.domain == NSURLErrorDomain && certificate.contains($0.code) && $0.code != NSURLErrorSecureConnectionFailed } ?? false) {
            return DialError(failure: .authRejected, reason: "\(host): TLS: the access point's certificate is not trusted")
        }
        return DialError(failure: .retryable, reason: "\(host): \(nsError?.localizedDescription ?? "the connection failed")")
    }

    /// RUTIS_CA: trusted besides the system's roots.
    func urlSession(_: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void)
    {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust
        else {
            return completionHandler(.performDefaultHandling, nil)
        }
        if !anchors.isEmpty {
            SecTrustSetAnchorCertificates(trust, anchors as CFArray)
            SecTrustSetAnchorCertificatesOnly(trust, false)
        }
        if SecTrustEvaluateWithError(trust, nil) {
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            lock.withLock { untrusted = true }
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }

    private func startHeartbeat() {
        let (ping, timeout) = heartbeat()
        let timer = DispatchSource.makeTimerSource(queue: .global())
        timer.schedule(deadline: .now() + ping, repeating: ping)
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            if Date().timeIntervalSince(lock.withLock { self.lastSeen }) > timeout {
                close("the far end stopped answering")
                return
            }
            task.sendPing { [weak self] error in
                if error == nil { self?.lock.withLock { self?.lastSeen = Date() } }
            }
        }
        lock.withLock { heart = timer }
        timer.resume()
    }

    /// The far end's close code, once it closed with one.
    var closeCode: Int? { lock.withLock { farCode } }

    func send(_ message: [UInt8]) throws {
        if message.count > maxMessage {
            close("message too big")
            throw ChannelError("message of \(message.count) bytes exceeds the limit of \(maxMessage)")
        }
        guard let text = String(data: Data(message), encoding: .utf8) else { throw ChannelError("a message is not UTF-8") }
        let done = DispatchSemaphore(value: 0)
        let failure = ErrorBox()
        task.send(.string(text)) { error in
            if let error { failure.set(ChannelError("\(error.localizedDescription)")) }
            done.signal()
        }
        if done.wait(timeout: .now() + 60) == .timedOut { throw ChannelError("the far end stopped reading") }
        if let error = failure.get() { throw error }
    }

    func receive() throws -> [UInt8]? {
        let done = DispatchSemaphore(value: 0)
        var result: Result<URLSessionWebSocketTask.Message, Error>!
        task.receive { received in
            result = received
            done.signal()
        }
        done.wait()
        lock.withLock { lastSeen = Date() }
        switch result! {
        case let .success(.string(text)):
            return Array(text.utf8)
        case .success(.data):
            close("binary messages are reserved for a binary encoding", code: .unsupportedData)
            throw ChannelError("received a binary message")
        case .success:
            throw ChannelError("an unknown message")
        case let .failure(error):
            if lock.withLock({ shut }) { return nil }
            // The far end's close frame says why, when it sent one.
            let (code, reason) = lock.withLock { (farCode ?? task.closeCode.rawValue, farReason) }
            let said = reason.isEmpty ? (task.closeReason.map { String(decoding: $0, as: UTF8.self) } ?? "") : reason
            switch code {
            case CloseCode.normal, CloseCode.goingAway: return nil
            case CloseCode.replaced: throw ChannelError("replaced by a new connection")
            case CloseCode.rejected: throw ChannelError("rejected: \(said)")
            case 0: throw ChannelError(error.localizedDescription)
            default: throw ChannelError("closed (\(code)): \(said)")
            }
        }
    }

    func close() { close("") }

    func close(_ reason: String, code: URLSessionWebSocketTask.CloseCode = .goingAway) {
        let first: Bool = lock.withLock {
            if shut { return false }
            shut = true
            return true
        }
        guard first else { return }
        lock.withLock { heart }?.cancel()
        task.cancel(with: code, reason: Data(reason.utf8.prefix(123)))
        session.finishTasksAndInvalidate()
    }
}

/// Becomes true once, when the runtime is told to stop; tasks wait for it.
final class StopSignal: @unchecked Sendable {
    private let lock = Lock()
    private var stopped = false
    private var waiters: [UUID: CheckedContinuation<Void, Never>] = [:]

    var isStopped: Bool { lock.withLock { stopped } }

    func stop() {
        let waiting: [CheckedContinuation<Void, Never>] = lock.withLock {
            if stopped { return [] }
            stopped = true
            defer { waiters = [:] }
            return Array(waiters.values)
        }
        for waiter in waiting { waiter.resume() }
    }

    /// Returns once stopped, or once the waiting Task is cancelled.
    func wait() async {
        let id = UUID()
        await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                let now = lock.withLock { () -> Bool in
                    if stopped || Task.isCancelled { return true }
                    waiters[id] = continuation
                    return false
                }
                if now { continuation.resume() }
            }
        } onCancel: {
            lock.withLock { waiters.removeValue(forKey: id) }?.resume()
        }
    }

    /// The first of `operation` and the stop: nil when the stop came first.
    /// The loser is cancelled, and not waited for.
    func race<T: Sendable>(_ operation: @escaping @Sendable () async throws -> T) async throws -> T? {
        let decided = Lock()
        var done = false
        func first() -> Bool {
            decided.withLock {
                defer { done = true }
                return !done
            }
        }
        return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<T?, Error>) in
            let tasks = Lock()
            var other: Task<Void, Never>?
            let work = Task {
                let result: Result<T, Error>
                do { result = .success(try await operation()) } catch { result = .failure(error) }
                guard first() else { return }
                tasks.withLock { other }?.cancel()
                continuation.resume(with: result.map { Optional($0) })
            }
            let stopped = Task {
                await self.wait()
                guard !Task.isCancelled, first() else { return }
                work.cancel()
                continuation.resume(returning: nil)
            }
            tasks.withLock { other = stopped }
            if decided.withLock({ done }) { stopped.cancel() }
        }
    }
}

/// SIGTERM and Ctrl-C stop `stop`.
func onTermination(_ stop: StopSignal) -> [DispatchSourceSignal] {
    [SIGTERM, SIGINT].map { number in
        signal(number, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: number, queue: .global())
        source.setEventHandler { stop.stop() }
        source.resume()
        return source
    }
}

extension Rutis {
    /// Dials the access point at `spec` and stays connected, by the rules,
    /// until stopped (SIGTERM, Ctrl-C: returns) or the access point is
    /// incompatible (throws `Refused`).
    static func dialOut(_ spec: String, endpoint: Endpoint, project: String, _ plugins: [Definition]) async throws {
        let environment = ProcessInfo.processInfo.environment
        // Spent once read: what this process starts does not inherit it.
        let token = environment["RUTIS_TOKEN"] ?? ""
        unsetenv("RUTIS_TOKEN")
        let ca = environment["RUTIS_CA"] ?? ""
        let anchors = spec.hasPrefix("wss:") && !ca.isEmpty ? try caCertificates(ca) : []
        let stop = StopSignal()
        let sources = onTermination(stop)
        defer { sources.forEach { $0.cancel() } }
        let retry = Retry()
        var backoff = Backoff(retry)
        let address = "\(spec) as \(endpoint.local)"
        let protocolName = "rutis.\(endpointProtocol)"
        func say(_ line: String) { FileHandle.standardError.write(Data("rutis: \(line)\n".utf8)) }
        while true {
            var failure = DialFailure.retryable
            var reason = ""
            var lost = "cannot connect"
            do {
                guard let socket = try await stop.race({
                    try await DialedSocket.dial(spec, protocol: protocolName, token: token, anchors: anchors, timeout: retry.handshake)
                }) else { return }
                let (peer, runtime) = try startSession(channel: socket, project: project, plugins, endpoint: endpoint)
                let session = ControllerSession(peer: peer, runtime: runtime, socket: socket)
                // The greeting, within the handshake's time.
                let late = StopSignal()
                DispatchQueue.global().asyncAfter(deadline: .now() + retry.handshake) { late.stop() }
                let greeted = try await stop.race({ () -> Bool in
                    try await late.race({ await peer.ready(); return true }) ?? false
                })
                guard let greeted else {
                    try await session.stop()
                    return
                }
                if !greeted {
                    peer.close(ChannelError("the access point did not greet in time"))
                    socket.close("no greeting")
                } else if peer.error == nil {
                    say("connected to \(address)")
                    let since = Date()
                    guard try await stop.race({ await peer.closed(); return true }) != nil else {
                        try await session.stop()
                        return
                    }
                    if Date().timeIntervalSince(since) >= retry.stable { backoff.reset() }
                }
                await peer.closed()
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
                throw Refused(reason: reason)
            case .authRejected:
                wait = retry.rejected
                say("refused: auth-rejected (\(reason)); retrying in \(stateSeconds(wait))")
            case .retryable:
                wait = backoff.wait()
                say("\(lost) (\(reason)); retrying in \(stateSeconds(wait))")
            }
            guard try await stop.race({ try await Task.sleep(nanoseconds: UInt64(wait * 1e9)); return true }) != nil else { return }
        }
    }
}

/// What a session's end means for retrying: the access point rejecting
/// this id (4003), or greeting as another endpoint, is an authentication
/// failure; a greeting this side cannot speak is incompatible; anything
/// else, being replaced (4002) too, is retryable.
func sessionFailure(_ error: Error, closeCode: Int?) -> DialFailure {
    if closeCode == CloseCode.rejected { return .authRejected }
    if let ended = error as? SessionEnded {
        switch ended.kind {
        case .incompatible, .refused: return .incompatible
        case .mismatch: return .authRejected
        default: break
        }
    }
    return .retryable
}
#endif
