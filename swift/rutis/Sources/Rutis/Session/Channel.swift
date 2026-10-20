import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// The largest message a channel carries, as on the WebSocket channel.
package let maxMessage = 16 * 1024 * 1024

/// Carries ordered, reliable messages with their boundaries kept.
package protocol Channel: AnyObject, Sendable {
    func send(_ message: [UInt8]) throws
    /// The next message; nil once the far end finished. Blocks: only the
    /// session's reading thread calls it.
    func receive() throws -> [UInt8]?
    /// Ends the channel: a blocked `receive` returns.
    func close()
}

package struct ChannelError: Error, CustomStringConvertible {
    package let description: String
    package init(_ description: String) { self.description = description }
}

private func errnoText(_ what: String) -> ChannelError {
    ChannelError("\(what): \(String(cString: strerror(errno)))")
}

/// A stream socket carrying one message per line.
package final class LineChannel: Channel, @unchecked Sendable {
    private let fd: Int32
    private let writeLock = Lock()
    private let stateLock = Lock()
    private var buffer: [UInt8] = []
    private var start = 0
    /// Where the search for the next newline resumes.
    private var scanned = 0
    private var shut = false
    private var released = false
    private var reading = false

    /// Takes `fd`, a connected stream socket.
    package init(fd: Int32) {
        self.fd = fd
        var on: Int32 = 1
        #if canImport(Darwin)
        // A far end that went away fails a write instead of a SIGPIPE.
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        #endif
        // A far end that stops reading ends the session instead of blocking
        // the writer forever.
        var timeout = timeval(tv_sec: 60, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        _ = on
    }

    deinit { release() }

    package func send(_ message: [UInt8]) throws {
        if message.contains(0x0A) { throw ChannelError("a message contains a raw newline") }
        if message.count > maxMessage {
            throw ChannelError("message of \(message.count) bytes exceeds the limit of \(maxMessage)")
        }
        var line = message
        line.append(0x0A)
        writeLock.lock()
        defer { writeLock.unlock() }
        var offset = 0
        while offset < line.count {
            let written = line.withUnsafeBytes { raw in
                write(fd, raw.baseAddress! + offset, line.count - offset)
            }
            if written < 0 {
                if errno == EINTR { continue }
                throw errnoText("write")
            }
            offset += written
        }
    }

    package func receive() throws -> [UInt8]? {
        stateLock.lock()
        reading = true
        stateLock.unlock()
        defer {
            stateLock.lock()
            reading = false
            let release = shut && !released
            stateLock.unlock()
            if release { self.release() }
        }
        while true {
            if let newline = buffer[max(start, scanned)...].firstIndex(of: 0x0A) {
                let line = Array(buffer[start ..< newline])
                start = newline + 1
                scanned = start
                if start > 65536, start * 2 > buffer.count {
                    buffer.removeFirst(start)
                    start = 0
                    scanned = 0
                }
                return line
            }
            scanned = buffer.count
            if buffer.count - start > maxMessage + 1 {
                throw ChannelError("a message exceeds the limit of \(maxMessage) bytes")
            }
            // Wait in poll, not in read: on macOS a read blocked on this
            // (non-main) thread is now and then not woken when the far end
            // shuts the socket down, and the process would wait forever for
            // a session that ended (#387). poll is woken; its timeout looks
            // again all the same.
            var ready = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
            let polled = poll(&ready, 1, 1000)
            if polled == 0 { continue }
            if polled < 0 {
                if errno == EINTR { continue }
                if isShut { return nil }
                throw errnoText("poll")
            }
            var chunk = [UInt8](repeating: 0, count: 65536)
            let count = chunk.withUnsafeMutableBytes { raw in read(fd, raw.baseAddress!, raw.count) }
            if count < 0 {
                if errno == EINTR { continue }
                if isShut { return nil }
                throw errnoText("read")
            }
            if count == 0 {
                if buffer.count > start { throw ChannelError("stream ended inside a message") }
                return nil
            }
            buffer.append(contentsOf: chunk[0 ..< count])
        }
    }

    private var isShut: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return shut
    }

    package func close() {
        stateLock.lock()
        if shut {
            stateLock.unlock()
            return
        }
        shut = true
        let busy = reading
        stateLock.unlock()
        // Wakes a blocked read; the descriptor itself goes once nothing
        // reads it.
        shutdown(fd, Int32(SHUT_RDWR))
        if !busy { release() }
    }

    private func release() {
        stateLock.lock()
        if released {
            stateLock.unlock()
            return
        }
        released = true
        stateLock.unlock()
        _ = Darwin.close(fd)
    }
}

// ── Opening channels ─────────────────────────────────────────────────────

/// The environment variable holding the token a process presents first on a
/// `tcp:` channel.
package let channelTokenVariable = "RUTIS_CHANNEL_TOKEN"

package enum Channels {
    /// A pair of connected channels in this process (a socket pair).
    package static func pair() throws -> (LineChannel, LineChannel) {
        var fds: [Int32] = [0, 0]
        guard socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0 else { throw errnoText("socketpair") }
        return (LineChannel(fd: fds[0]), LineChannel(fd: fds[1]))
    }

    /// The local channel `spec` names: `fd:<n>` (an inherited socket),
    /// `tcp:<host>:<port>` (dialed, presenting RUTIS_CHANNEL_TOKEN first), or
    /// `unix:<path>` / a bare path (dialed).
    package static func open(_ spec: String) throws -> LineChannel {
        if spec.hasPrefix("fd:") {
            guard let fd = Int32(spec.dropFirst(3)), fd >= 0 else { throw ChannelError("invalid channel \(spec)") }
            guard fcntl(fd, F_GETFD) != -1 else { throw ChannelError("no file descriptor \(fd)") }
            // Not passed on to what this process starts.
            _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
            return LineChannel(fd: fd)
        }
        if spec.hasPrefix("tcp:") {
            let address = String(spec.dropFirst(4))
            let token = ProcessInfo.processInfo.environment[channelTokenVariable] ?? ""
            // Spent once connected: what this process starts does not inherit it.
            unsetenv(channelTokenVariable)
            guard !token.isEmpty else { throw ChannelError("\(channelTokenVariable) is not set for \(spec)") }
            let fd = try dialTCP(address)
            let channel = LineChannel(fd: fd)
            var line = Array(token.utf8)
            line.append(0x0A)
            var offset = 0
            while offset < line.count {
                let written = line.withUnsafeBytes { raw in write(fd, raw.baseAddress! + offset, line.count - offset) }
                if written < 0 {
                    if errno == EINTR { continue }
                    throw errnoText("write")
                }
                offset += written
            }
            return channel
        }
        let path = spec.hasPrefix("unix:") ? String(spec.dropFirst(5)) : spec
        return LineChannel(fd: try dialUnix(path))
    }

    static func dialUnix(_ path: String) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw errnoText("socket") }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard bytes.count < capacity else {
            _ = Darwin.close(fd)
            throw ChannelError("socket path too long: \(path)")
        }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        #if canImport(Darwin)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        #endif
        let result = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard result == 0 else {
            let error = errnoText("connect \(path)")
            _ = Darwin.close(fd)
            throw error
        }
        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
        return fd
    }

    static func dialTCP(_ address: String) throws -> Int32 {
        guard let colon = address.lastIndex(of: ":") else { throw ChannelError("invalid address \(address)") }
        var host = String(address[..<colon])
        let port = String(address[address.index(after: colon)...])
        if host.hasPrefix("["), host.hasSuffix("]") { host = String(host.dropFirst().dropLast()) }
        var hints = addrinfo()
        hints.ai_socktype = SOCK_STREAM
        hints.ai_family = AF_UNSPEC
        var list: UnsafeMutablePointer<addrinfo>?
        let status = getaddrinfo(host, port, &hints, &list)
        guard status == 0, let first = list else {
            throw ChannelError("cannot resolve \(address): \(String(cString: gai_strerror(status)))")
        }
        defer { freeaddrinfo(list) }
        var lastError = ChannelError("cannot connect to \(address)")
        var info: UnsafeMutablePointer<addrinfo>? = first
        while let entry = info {
            let fd = socket(entry.pointee.ai_family, entry.pointee.ai_socktype, entry.pointee.ai_protocol)
            if fd >= 0 {
                if connect(fd, entry.pointee.ai_addr, entry.pointee.ai_addrlen) == 0 {
                    var on: Int32 = 1
                    setsockopt(fd, Int32(IPPROTO_TCP), TCP_NODELAY, &on, socklen_t(MemoryLayout<Int32>.size))
                    _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
                    return fd
                }
                lastError = errnoText("connect \(address)")
                _ = Darwin.close(fd)
            }
            info = entry.pointee.ai_next
        }
        throw lastError
    }
}
