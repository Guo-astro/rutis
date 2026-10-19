// A WebSocket connection carrying the session's text messages (RFC 6455),
// framed here over a byte stream, as Go's runtime frames it: the listener
// (Listen.swift) decides the HTTP status of every refusal itself, which
// Network.framework's WebSocket protocol does not let it do. One message is
// at most 16 MiB; pings keep a silent far end from holding the connection.
#if canImport(Network)
import Foundation
import Network

/// Close codes the runtimes use.
enum CloseCode {
    static let normal = 1000
    static let goingAway = 1001
    static let protocolError = 1002
    static let unsupported = 1003
    static let noStatus = 1005
    static let invalidData = 1007
    static let tooBig = 1009
    /// A newer connection took this one's place.
    static let replaced = 4002
    /// An access point rejecting the id a dialer greeted with.
    static let rejected = 4003
}

/// The ping interval and how long a silent far end is kept
/// (`RUTIS_HEARTBEAT=<ping ms>,<timeout ms>` shortens them for tests).
func heartbeat() -> (ping: Double, timeout: Double) {
    if let configured = ProcessInfo.processInfo.environment["RUTIS_HEARTBEAT"] {
        let parts = configured.split(separator: ",").compactMap { Int($0.trimmingCharacters(in: .whitespaces)) }
        if parts.count == 2 { return (Double(parts[0]) / 1000, Double(parts[1]) / 1000) }
    }
    return (10, 30)
}

/// A connected NWConnection as a blocking byte stream: the session reads
/// on its own thread.
final class ByteStream: @unchecked Sendable {
    let connection: NWConnection
    private let lock = Lock()
    private var buffer: [UInt8] = []
    private var ended: Error?
    private var finished = false

    init(_ connection: NWConnection) { self.connection = connection }

    /// Exactly `count` bytes; throws once the stream ended before them.
    func read(_ count: Int) throws -> [UInt8] {
        while true {
            let ready: [UInt8]? = lock.withLock {
                guard buffer.count >= count else { return nil }
                let bytes = Array(buffer[..<count])
                buffer.removeFirst(count)
                return bytes
            }
            if let ready { return ready }
            try fill()
        }
    }

    /// The bytes up to and including the first `delimiter`, at most `limit`.
    func read(until delimiter: [UInt8], limit: Int) throws -> [UInt8] {
        var scanned = 0
        while true {
            let found: [UInt8]? = try lock.withLock {
                if let at = Self.find(delimiter, in: buffer, from: max(0, scanned - delimiter.count)) {
                    let bytes = Array(buffer[..<(at + delimiter.count)])
                    buffer.removeFirst(at + delimiter.count)
                    return bytes
                }
                scanned = buffer.count
                if buffer.count > limit { throw ChannelError("the request is too large") }
                return nil
            }
            if let found { return found }
            try fill()
        }
    }

    private static func find(_ needle: [UInt8], in haystack: [UInt8], from start: Int) -> Int? {
        guard haystack.count >= needle.count else { return nil }
        var index = start
        while index + needle.count <= haystack.count {
            if haystack[index] == needle[0], Array(haystack[index..<(index + needle.count)]) == needle { return index }
            index += 1
        }
        return nil
    }

    /// Waits for more bytes.
    private func fill() throws {
        if let ended = lock.withLock({ ended }) { throw ended }
        if lock.withLock({ finished }) { throw ChannelError("the connection closed") }
        let done = DispatchSemaphore(value: 0)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [self] data, _, complete, error in
            lock.withLock {
                if let data, !data.isEmpty { buffer.append(contentsOf: data) }
                if let error { ended = ended ?? ChannelError("\(error)") }
                if complete, data?.isEmpty ?? true { finished = true }
            }
            done.signal()
        }
        done.wait()
    }

    /// Writes `bytes`, waiting until they are handed to the network.
    func write(_ bytes: [UInt8]) throws {
        let done = DispatchSemaphore(value: 0)
        let failure = ErrorBox()
        connection.send(content: Data(bytes), completion: .contentProcessed { error in
            if let error { failure.set(ChannelError("\(error)")) }
            done.signal()
        })
        if done.wait(timeout: .now() + 30) == .timedOut { throw ChannelError("the far end stopped reading") }
        if let error = failure.get() { throw error }
    }

    func close() { connection.cancel() }
}

/// A WebSocket connection: accepted by a listener, or (`client`) dialed; a
/// client masks what it sends.
final class WebSocket: Channel, @unchecked Sendable {
    private let stream: ByteStream
    private let client: Bool
    private let writeLock = Lock()
    private let stateLock = Lock()
    private var shut = false
    private var lastSeen = Date()
    /// The code of the far end's close frame, once it closed with one.
    private(set) var farClose: Int?
    private var heart: DispatchSourceTimer?

    init(_ stream: ByteStream, client: Bool) {
        self.stream = stream
        self.client = client
        let (ping, timeout) = heartbeat()
        let timer = DispatchSource.makeTimerSource(queue: .global())
        timer.schedule(deadline: .now() + ping, repeating: ping)
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            if Date().timeIntervalSince(stateLock.withLock { self.lastSeen }) > timeout {
                closeWith(CloseCode.goingAway, "the far end stopped answering")
                return
            }
            try? writeFrame(0x9, [])
        }
        heart = timer
        timer.resume()
    }

    deinit { heart?.cancel() }

    var closeCode: Int? { stateLock.withLock { farClose } }

    func send(_ message: [UInt8]) throws {
        if message.count > maxMessage {
            let reason = "message of \(message.count) bytes exceeds the limit of \(maxMessage)"
            closeWith(CloseCode.tooBig, reason)
            throw ChannelError(reason)
        }
        if stateLock.withLock({ shut }) { throw ChannelError("connection closed") }
        try writeFrame(0x1, message)
    }

    func receive() throws -> [UInt8]? {
        var message: [UInt8] = []
        while true {
            let (fin, opcode, payload): (Bool, UInt8, [UInt8])
            do {
                (fin, opcode, payload) = try readFrame()
            } catch {
                if stateLock.withLock({ shut }) { return nil }
                end()
                throw error
            }
            stateLock.withLock { lastSeen = Date() }
            switch opcode {
            case 0x8:
                let code = payload.count >= 2 ? Int(payload[0]) << 8 | Int(payload[1]) : CloseCode.noStatus
                let reason = String(decoding: payload.dropFirst(2), as: UTF8.self)
                stateLock.withLock { farClose = code }
                closeWith(code == CloseCode.noStatus ? CloseCode.normal : code, "")
                switch code {
                case CloseCode.normal, CloseCode.goingAway, CloseCode.noStatus: return nil
                case CloseCode.replaced: throw ChannelError("replaced by a new connection")
                case CloseCode.rejected: throw ChannelError("rejected: \(reason)")
                default: throw ChannelError("closed (\(code)): \(reason)")
                }
            case 0x9:
                try? writeFrame(0xA, payload)
            case 0xA:
                continue
            case 0x2:
                closeWith(CloseCode.unsupported, "binary messages are reserved for a binary encoding")
                throw ChannelError("received a binary message")
            case 0x1, 0x0:
                message += payload
                if message.count > maxMessage {
                    closeWith(CloseCode.tooBig, "message too big")
                    throw ChannelError("a message exceeds the limit of \(maxMessage) bytes")
                }
                if fin {
                    guard String(data: Data(message), encoding: .utf8) != nil else {
                        closeWith(CloseCode.invalidData, "invalid UTF-8")
                        throw ChannelError("a message is not UTF-8")
                    }
                    return message
                }
            default:
                closeWith(CloseCode.protocolError, "unknown opcode")
                throw ChannelError("protocol error")
            }
        }
    }

    func close() { closeWith(CloseCode.goingAway, "") }

    /// Sends a close frame with `code` and closes the connection.
    func closeWith(_ code: Int, _ reason: String) {
        let first: Bool = stateLock.withLock {
            if shut { return false }
            shut = true
            return true
        }
        guard first else { return }
        heart?.cancel()
        var payload = [UInt8(code >> 8 & 0xFF), UInt8(code & 0xFF)]
        payload += Array(reason.utf8.prefix(123))
        try? writeFrame(0x8, payload)
        let stream = self.stream
        DispatchQueue.global().asyncAfter(deadline: .now() + 0.2) { stream.close() }
    }

    /// Ends the connection without a close frame (it is already gone).
    private func end() {
        stateLock.withLock { shut = true }
        heart?.cancel()
        stream.close()
    }

    private func writeFrame(_ opcode: UInt8, _ payload: [UInt8]) throws {
        var frame: [UInt8] = [0x80 | opcode]
        let mask: UInt8 = client ? 0x80 : 0
        let count = payload.count
        if count < 126 {
            frame.append(mask | UInt8(count))
        } else if count <= 0xFFFF {
            frame += [mask | 126, UInt8(count >> 8), UInt8(count & 0xFF)]
        } else {
            frame.append(mask | 127)
            for shift in stride(from: 56, through: 0, by: -8) { frame.append(UInt8(UInt64(count) >> UInt64(shift) & 0xFF)) }
        }
        var body = payload
        if client {
            var key = [UInt8](repeating: 0, count: 4)
            for index in 0..<4 { key[index] = UInt8.random(in: 0...255) }
            frame += key
            for index in body.indices { body[index] ^= key[index % 4] }
        }
        try writeLock.withLock { try stream.write(frame + body) }
    }

    private func readFrame() throws -> (Bool, UInt8, [UInt8]) {
        let head = try stream.read(2)
        let fin = head[0] & 0x80 != 0
        let opcode = head[0] & 0x0F
        let masked = head[1] & 0x80 != 0
        var length = UInt64(head[1] & 0x7F)
        if length == 126 {
            let extended = try stream.read(2)
            length = UInt64(extended[0]) << 8 | UInt64(extended[1])
        } else if length == 127 {
            length = try stream.read(8).reduce(0) { $0 << 8 | UInt64($1) }
        }
        if masked == client {
            closeWith(CloseCode.protocolError, client ? "server frames must not be masked" : "client frames must be masked")
            throw ChannelError(client ? "a masked server frame" : "an unmasked client frame")
        }
        if length > UInt64(maxMessage) {
            closeWith(CloseCode.tooBig, "message too big")
            throw ChannelError("a message exceeds the limit of \(maxMessage) bytes")
        }
        let key = masked ? try stream.read(4) : []
        var payload = try stream.read(Int(length))
        if masked { for index in payload.indices { payload[index] ^= key[index % 4] } }
        return (fin, opcode, payload)
    }
}
#endif
