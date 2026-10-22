// `listen:ws://…` / `listen:wss://…`: the runtime waits for a rutis
// controller to connect (design §6.1, S14), with the rules of every
// runtime's listener (Go's listen.go, rutis-bridge's): a non-loopback
// address needs TLS (RUTIS_CERT, RUTIS_KEY); the controller presents
// RUTIS_TOKEN as a Bearer token, compared in constant time; the session
// protocol is `rutis.3`; one controller at a time, a newer connection taking
// over. The listener is Network.framework's; the upgrade and the frames are
// this SDK's (WebSocket.swift), so every refusal has its own status.
#if canImport(Network)
import CryptoKit
import Foundation
import Network
import Security

/// Why a dialer that offered none of this endpoint's session protocol is
/// refused, and which end to upgrade: rutis-bridge's wording.
func protocolRefusal(_ ours: String, offered: [String]) -> String {
    func version(_ name: String) -> Int? {
        guard name.hasPrefix("rutis.") else { return nil }
        let number = name.dropFirst("rutis.".count)
        guard !number.isEmpty, number.allSatisfy(\.isASCII), number.allSatisfy(\.isNumber) else { return nil }
        return Int(number)
    }
    var reason = "this endpoint speaks \(ours)"
    let theirs = offered.compactMap { name in version(name).map { (name, $0) } }
    guard let n = version(ours), !theirs.isEmpty else { return reason }
    reason += ", the dialer offered " + theirs.map(\.0).joined(separator: ", ")
    if theirs.allSatisfy({ $0.1 > n }) {
        return reason + ": upgrade this endpoint (the Swift package Rutis of github.com/arcships/rutis)"
    }
    if theirs.allSatisfy({ $0.1 < n }) { return reason + ": upgrade the dialer" }
    return reason
}

/// A WebSocket address as a listener takes it.
struct ListenAddress {
    let secure: Bool
    let host: String
    let port: UInt16
    let path: String
    let scheme: String

    init(_ spec: String) throws {
        guard let url = URLComponents(string: spec), let scheme = url.scheme, scheme == "ws" || scheme == "wss" else {
            throw ChannelError("\(spec) is not a ws:// or wss:// address")
        }
        self.scheme = scheme
        secure = scheme == "wss"
        var host = url.host ?? ""
        if host.hasPrefix("["), host.hasSuffix("]") { host = String(host.dropFirst().dropLast()) }
        if host.isEmpty || host == "localhost" { host = host.isEmpty ? "127.0.0.1" : host }
        if !secure, !["127.0.0.1", "::1", "localhost"].contains(host) {
            throw ChannelError("\(spec): only a loopback listener may go without TLS")
        }
        self.host = host == "localhost" ? "127.0.0.1" : host
        port = UInt16(url.port ?? (secure ? 443 : 80))
        path = url.path.isEmpty ? "/" : url.path
    }

    /// The address as announced, with the port bound.
    func bound(_ port: UInt16) -> String {
        let shown = host.contains(":") ? "[\(host)]" : host
        return "\(scheme)://\(shown):\(port)\(path)"
    }
}

/// The TLS identity in the PEM files `cert` (the certificate, then its
/// chain) and `key` (its private key).
func tlsIdentity(cert: String, key: String) throws -> sec_identity_t {
    func pemBlocks(_ text: String, _ label: String) -> [Data] {
        var blocks: [Data] = []
        var rest = Substring(text)
        while let begin = rest.range(of: "-----BEGIN \(label)-----"),
              let end = rest.range(of: "-----END \(label)-----", range: begin.upperBound..<rest.endIndex)
        {
            let body = rest[begin.upperBound..<end.lowerBound].filter { !$0.isWhitespace }
            if let data = Data(base64Encoded: String(body)) { blocks.append(data) }
            rest = rest[end.upperBound...]
        }
        return blocks
    }
    guard let certText = try? String(contentsOfFile: cert, encoding: .utf8) else {
        throw ChannelError("RUTIS_CERT \(cert): cannot read it")
    }
    let certificates = pemBlocks(certText, "CERTIFICATE").compactMap { SecCertificateCreateWithData(nil, $0 as CFData) }
    guard let leaf = certificates.first else { throw ChannelError("RUTIS_CERT \(cert): no PEM certificate in it") }
    guard let keyData = FileManager.default.contents(atPath: key) else {
        throw ChannelError("RUTIS_KEY \(key): cannot read it")
    }
    guard let privateKey = privateKey(pem: String(decoding: keyData, as: UTF8.self)) else {
        throw ChannelError("RUTIS_KEY \(key): no private key in it (PKCS#8, SEC1 EC or PKCS#1 RSA, PEM)")
    }
    typealias Create = @convention(c) (CFAllocator?, SecCertificate, SecKey) -> Unmanaged<SecIdentity>?
    guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "SecIdentityCreate"),
          let identity = unsafeBitCast(symbol, to: Create.self)(nil, leaf, privateKey)?.takeRetainedValue()
    else {
        throw ChannelError("RUTIS_CERT, RUTIS_KEY: the key is not the certificate's, or this system cannot pair them")
    }
    guard let tls = sec_identity_create_with_certificates(identity, Array(certificates.dropFirst()) as CFArray) else {
        throw ChannelError("RUTIS_CERT \(cert): cannot use the chain")
    }
    return tls
}

/// The private key in `pem`: an EC key (PKCS#8 or SEC1, P-256 / P-384 /
/// P-521) or an RSA key (PKCS#8 or PKCS#1), as tools write them. Kept in
/// this process: no keychain.
func privateKey(pem: String) -> SecKey? {
    func create(_ data: Data, _ type: CFString) -> SecKey? {
        let attributes: [CFString: Any] = [kSecAttrKeyType: type, kSecAttrKeyClass: kSecAttrKeyClassPrivate]
        return SecKeyCreateWithData(data as CFData, attributes as CFDictionary, nil)
    }
    let ec: [(String) throws -> Data] = [
        { try P256.Signing.PrivateKey(pemRepresentation: $0).x963Representation },
        { try P384.Signing.PrivateKey(pemRepresentation: $0).x963Representation },
        { try P521.Signing.PrivateKey(pemRepresentation: $0).x963Representation },
    ]
    for curve in ec {
        if let x963 = try? curve(pem) { return create(x963, kSecAttrKeyTypeECSECPrimeRandom) }
    }
    func block(_ label: String) -> Data? {
        guard let begin = pem.range(of: "-----BEGIN \(label)-----"),
              let end = pem.range(of: "-----END \(label)-----", range: begin.upperBound..<pem.endIndex)
        else { return nil }
        return Data(base64Encoded: String(pem[begin.upperBound..<end.lowerBound].filter { !$0.isWhitespace }))
    }
    if let pkcs1 = block("RSA PRIVATE KEY") { return create(pkcs1, kSecAttrKeyTypeRSA) }
    // PKCS#8: SEQUENCE { INTEGER, SEQUENCE { OID, … }, OCTET STRING key }.
    guard let pkcs8 = block("PRIVATE KEY") else { return nil }
    var reader = DERReader(Array(pkcs8))
    guard var outer = reader.next(0x30), outer.next(0x02) != nil, outer.next(0x30) != nil,
          let inner = outer.next(0x04)
    else { return nil }
    return create(Data(inner.bytes), kSecAttrKeyTypeRSA)
}

/// Reads DER: one element at a time, of the tag expected.
struct DERReader {
    var bytes: [UInt8]
    private var offset = 0

    init(_ bytes: [UInt8]) { self.bytes = bytes }

    /// The contents of the next element, if its tag is `tag`.
    mutating func next(_ tag: UInt8) -> DERReader? {
        guard offset + 2 <= bytes.count, bytes[offset] == tag else { return nil }
        var length = Int(bytes[offset + 1])
        var start = offset + 2
        if length & 0x80 != 0 {
            let count = length & 0x7F
            guard count <= 4, start + count <= bytes.count else { return nil }
            length = bytes[start..<(start + count)].reduce(0) { $0 << 8 | Int($1) }
            start += count
        }
        guard start + length <= bytes.count else { return nil }
        offset = start + length
        return DERReader(Array(bytes[start..<(start + length)]))
    }
}

/// Listens on a WebSocket address for controllers presenting `token` and
/// speaking `protocol`, handing each upgraded connection to `accepted`.
final class WebSocketListener: @unchecked Sendable {
    let address: ListenAddress
    private let listener: NWListener
    private let token: [UInt8]
    private let protocolName: String
    private let queue = DispatchQueue(label: "rutis.listen")
    /// How long a connection may take to present its upgrade request
    /// (TLS included), as Go's ReadHeaderTimeout: then it is dropped.
    let upgradeTimeout: Double
    /// At most this many connections upgrading at once (each reads on a
    /// thread of its own); more are dropped at once.
    let maxUpgrading: Int
    private let stopped = Flag()
    private let upgradingLock = Lock()
    private var upgrading = 0

    /// `identity` gives the TLS identity of a wss:// address, or says why
    /// there is none.
    init(_ spec: String, protocol protocolName: String, token: String, identity: () throws -> sec_identity_t,
         upgradeTimeout: Double = 10, maxUpgrading: Int = 32) throws
    {
        address = try ListenAddress(spec)
        self.upgradeTimeout = upgradeTimeout
        self.maxUpgrading = maxUpgrading
        self.token = Array(token.utf8)
        self.protocolName = protocolName
        let parameters: NWParameters
        if address.secure {
            let identity = try identity()
            let tls = NWProtocolTLS.Options()
            sec_protocol_options_set_local_identity(tls.securityProtocolOptions, identity)
            sec_protocol_options_set_min_tls_protocol_version(tls.securityProtocolOptions, .TLSv12)
            parameters = NWParameters(tls: tls)
        } else {
            parameters = NWParameters.tcp
        }
        parameters.allowLocalEndpointReuse = true
        parameters.requiredLocalEndpoint = .hostPort(host: NWEndpoint.Host(address.host), port: NWEndpoint.Port(rawValue: address.port)!)
        do {
            listener = try NWListener(using: parameters)
        } catch {
            throw ChannelError("\(spec): \(error)")
        }
    }

    /// Starts listening; returns the address it is bound to once it is.
    /// `failed` hears of the listener failing after that.
    func start(failed: @escaping @Sendable (Error) -> Void = { _ in },
               _ accepted: @escaping @Sendable (WebSocket) -> Void) throws -> String
    {
        let ready = DispatchSemaphore(value: 0)
        let failure = ErrorBox()
        let bound = Flag()
        let shown = address.bound(address.port)
        listener.stateUpdateHandler = { state in
            switch state {
            case .ready:
                bound.set()
                ready.signal()
            case let .failed(error), let .waiting(error):
                if bound.isSet {
                    if case .failed = state { failed(ChannelError("\(error)")) }
                    return
                }
                failure.set(ChannelError("\(shown): \(error)"))
                ready.signal()
            default: break
            }
        }
        listener.newConnectionHandler = { [self] connection in
            // Room for it, and a deadline for its upgrade request: a
            // connection presenting none is dropped, so that unauthenticated
            // ones cannot hold the app's threads.
            guard reserve() else {
                connection.start(queue: queue)
                connection.cancel()
                return
            }
            let attempt = UpgradeAttempt { [self] in release() }
            queue.asyncAfter(deadline: .now() + upgradeTimeout) {
                if attempt.finish() { connection.cancel() }
            }
            connection.stateUpdateHandler = { [self] state in
                switch state {
                case .ready:
                    connection.stateUpdateHandler = nil
                    Thread.detachNewThread { [self] in
                        let socket = upgrade(connection)
                        guard attempt.finish() else {
                            // Too late: the deadline dropped it.
                            connection.cancel()
                            return
                        }
                        if let socket { accepted(socket) }
                    }
                case .failed, .cancelled:
                    connection.stateUpdateHandler = nil
                    connection.cancel()
                    _ = attempt.finish()
                default: break
                }
            }
            connection.start(queue: queue)
        }
        listener.start(queue: queue)
        ready.wait()
        if let error = failure.get() {
            listener.cancel()
            throw error
        }
        return address.bound(listener.port?.rawValue ?? address.port)
    }

    private func reserve() -> Bool {
        upgradingLock.withLock {
            guard upgrading < maxUpgrading else { return false }
            upgrading += 1
            return true
        }
    }

    private func release() { upgradingLock.withLock { upgrading -= 1 } }

    /// Connections upgrading now.
    var pendingUpgrades: Int { upgradingLock.withLock { upgrading } }

    /// Stops listening; returns once the listener is cancelled (its port
    /// free, so that a `start()` right after a `stop()` binds it again),
    /// or after 2 s. Never from the listener's queue.
    func stop() {
        guard !stopped.testAndSet() else { return }
        let cancelled = DispatchSemaphore(value: 0)
        listener.stateUpdateHandler = { state in
            if case .cancelled = state { cancelled.signal() }
        }
        listener.cancel()
        _ = cancelled.wait(timeout: .now() + 2)
    }

    /// Reads the upgrade request on `connection` and answers it: a refusal
    /// with its status (nil), or the switch to the session protocol.
    private func upgrade(_ connection: NWConnection) -> WebSocket? {
        let stream = ByteStream(connection)
        guard let head = try? stream.read(until: Array("\r\n\r\n".utf8), limit: 16384),
              let text = String(data: Data(head), encoding: .utf8)
        else {
            stream.close()
            return nil
        }
        var lines = text.components(separatedBy: "\r\n")
        let requestLine = lines.removeFirst().split(separator: " ")
        var headers: [(String, String)] = []
        for line in lines where !line.isEmpty {
            guard let colon = line.firstIndex(of: ":") else { continue }
            headers.append((line[..<colon].lowercased(), line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)))
        }
        func header(_ name: String) -> String? { headers.first { $0.0 == name }?.1 }
        func refuse(_ status: Int, _ phrase: String, _ body: String) -> WebSocket? {
            let content = Array((body + "\n").utf8)
            let response = "HTTP/1.1 \(status) \(phrase)\r\nContent-Type: text/plain; charset=utf-8\r\n" +
                "Content-Length: \(content.count)\r\nConnection: close\r\n\r\n"
            try? stream.write(Array(response.utf8) + content)
            DispatchQueue.global().asyncAfter(deadline: .now() + 0.2) { stream.close() }
            return nil
        }
        guard requestLine.count >= 2, requestLine[0] == "GET" else {
            return refuse(405, "Method Not Allowed", "a WebSocket upgrade is required")
        }
        let path = requestLine[1].split(separator: "?", maxSplits: 1).first.map(String.init) ?? ""
        guard path == address.path else { return refuse(404, "Not Found", "no rutis endpoint here") }
        guard let authorization = header("authorization"), authorization.hasPrefix("Bearer ") else {
            return refuse(401, "Unauthorized", "credentials required")
        }
        let presented = Array(authorization.dropFirst("Bearer ".count).utf8)
        guard !token.isEmpty, constantTimeEqual(presented, token) else {
            return refuse(403, "Forbidden", "not accepted here")
        }
        let offered = headers.filter { $0.0 == "sec-websocket-protocol" }
            .flatMap { $0.1.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) } }
        guard offered.contains(protocolName) else {
            return refuse(400, "Bad Request", protocolRefusal(protocolName, offered: offered))
        }
        guard header("upgrade")?.lowercased() == "websocket", let challenge = header("sec-websocket-key"), !challenge.isEmpty else {
            return refuse(400, "Bad Request", "a WebSocket upgrade is required")
        }
        let sum = Insecure.SHA1.hash(data: Data((challenge + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").utf8))
        let response = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
            "Sec-WebSocket-Accept: \(Data(sum).base64EncodedString())\r\n" +
            "Sec-WebSocket-Protocol: \(protocolName)\r\n\r\n"
        guard (try? stream.write(Array(response.utf8))) != nil else {
            stream.close()
            return nil
        }
        return WebSocket(stream, client: false)
    }
}

/// One connection's upgrade: ended once, by its deadline, its failure or
/// its answer; the first to end it acts.
final class UpgradeAttempt: @unchecked Sendable {
    private let lock = Lock()
    private var ended = false
    private let done: () -> Void

    init(_ done: @escaping () -> Void) { self.done = done }

    /// Whether this call ended it.
    func finish() -> Bool {
        let first: Bool = lock.withLock {
            defer { ended = true }
            return !ended
        }
        if first { done() }
        return first
    }
}

func constantTimeEqual(_ a: [UInt8], _ b: [UInt8]) -> Bool {
    guard a.count == b.count else { return false }
    var difference: UInt8 = 0
    for index in a.indices { difference |= a[index] ^ b[index] }
    return difference == 0
}

/// One controller's session: its runtime and its connection.
final class ControllerSession: @unchecked Sendable {
    let peer: Peer
    let runtime: Runtime
    let socket: Channel

    init(peer: Peer, runtime: Runtime, socket: Channel) {
        self.peer = peer
        self.runtime = runtime
        self.socket = socket
    }

    /// Ends the lease: nothing more is read, then every row goes, within
    /// the shutdown deadline, then the connection closes.
    func end() async {
        try? await Rutis.end(peer, runtime)
        socket.close()
    }

    /// Stops for good: every row goes while the controller still hears
    /// from this end, then the connection closes.
    func stop() async throws {
        runtime.lock.withLock { runtime.closing = true }
        let budget = runtime.settings.shutdownTimeout
        let finished = await runtime.dispose(within: budget)
        peer.close(ChannelError("the runtime stopped"))
        socket.close()
        if !finished {
            throw RutisError(name: "Error", message: "cleanups did not finish within \(Events.seconds(budget))s after the runtime stopped")
        }
    }

    /// The refusal this side ended the session with (an old controller).
    var refused: Refused? {
        guard let ended = peer.error as? SessionEnded, ended.kind == .refused else { return nil }
        return Refused(reason: ended.reason)
    }
}

#endif
