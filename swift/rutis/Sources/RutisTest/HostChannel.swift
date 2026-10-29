// How the test process gets its session to `rutis-host test`: it starts
// `rutis-host test <driver> --channel <channel>` and hands it the channel as
// a host hands one to a local runtime: on Unix a socket pair, one end the
// host's fd 3 (`fd:3`); with RUTIS_LOCAL_HANDOVER=loopback a loopback
// address the host dials, presenting the one-time token in
// RUTIS_CHANNEL_TOKEN first (`tcp:127.0.0.1:<port>`).
#if os(macOS)
import Foundation
import Rutis

/// The host process, and the channel of its session for the runtime of
/// this process. `ended` hears the host's exit status.
func hostChannel(driver: String, ended: @escaping @Sendable (Int32) -> Void) throws -> (LineChannel, HostProcess) {
    let executable = try rutisHost()
    var environment = ProcessInfo.processInfo.environment
    if environment["RUTIS_LOCAL_HANDOVER"] == "loopback" {
        let listener = try LoopbackListener()
        let token = UUID().uuidString
        environment["RUTIS_CHANNEL_TOKEN"] = token
        let process = try HostProcess(executable, ["test", driver, "--channel", "tcp:127.0.0.1:\(listener.port)"], environment,
                                      channel: nil, ended: ended)
        return (LineChannel(fd: try listener.accept(token: token, seconds: 30)), process)
    }
    var fds: [Int32] = [0, 0]
    guard socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0 else { throw failure("socketpair") }
    _ = fcntl(fds[0], F_SETFD, FD_CLOEXEC)
    _ = fcntl(fds[1], F_SETFD, FD_CLOEXEC)
    defer { close(fds[1]) }
    let process = try HostProcess(executable, ["test", driver, "--channel", "fd:3"], environment, channel: fds[1], ended: ended)
    return (LineChannel(fd: fds[0]), process)
}

/// The rutis-host the tests use: RUTIS_HOST, or `rutis-host` on PATH.
func rutisHost() throws -> String {
    let environment = ProcessInfo.processInfo.environment
    if let given = environment["RUTIS_HOST"], !given.isEmpty { return given }
    for directory in (environment["PATH"] ?? "").split(separator: ":") {
        let candidate = "\(directory)/rutis-host"
        if FileManager.default.isExecutableFile(atPath: candidate) { return candidate }
    }
    throw RutisError(name: "Error", message: "RutisTest needs rutis-host: put it on PATH, or set RUTIS_HOST to its path")
}

func failure(_ what: String) -> RutisError {
    RutisError(name: "Error", message: "RutisTest: \(what): \(String(cString: strerror(errno)))")
}

/// `rutis-host test`, started with `channel` as its fd 3; its standard
/// output and error are this process's.
final class HostProcess: @unchecked Sendable {
    let pid: pid_t

    init(_ executable: String, _ arguments: [String], _ environment: [String: String], channel: Int32?,
         ended: @escaping @Sendable (Int32) -> Void) throws
    {
        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        if let channel { posix_spawn_file_actions_adddup2(&actions, channel, 3) }
        let argv = ([executable] + arguments).map { strdup($0) } + [nil]
        let envp = environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer {
            argv.forEach { free($0) }
            envp.forEach { free($0) }
        }
        var pid: pid_t = 0
        let status = posix_spawn(&pid, executable, &actions, nil, argv, envp)
        guard status == 0 else {
            throw RutisError(name: "Error", message: "RutisTest: cannot start \(executable): \(String(cString: strerror(status)))")
        }
        self.pid = pid
        Thread.detachNewThread {
            var code: Int32 = 0
            while waitpid(pid, &code, 0) == -1, errno == EINTR {}
            ended((code >> 8) & 0xFF)
        }
    }
}

/// A loopback listener for the one connection the host dials.
final class LoopbackListener {
    let fd: Int32
    let port: UInt16

    init() throws {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw failure("socket") }
        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        address.sin_port = 0
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let bound = withUnsafeMutablePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { pointer in
                bind(fd, pointer, length) == 0 && listen(fd, 1) == 0 && getsockname(fd, pointer, &length) == 0
            }
        }
        guard bound else {
            let error = failure("listen on 127.0.0.1")
            close(fd)
            throw error
        }
        self.fd = fd
        port = UInt16(bigEndian: address.sin_port)
    }

    deinit { close(fd) }

    /// The connection that presents `token` first, within `seconds`.
    func accept(token: String, seconds: Double) throws -> Int32 {
        var ready = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        guard poll(&ready, 1, Int32(seconds * 1000)) == 1 else {
            throw RutisError(name: "Error", message: "RutisTest: rutis-host test did not connect within \(Int(seconds)) s")
        }
        let connection = Darwin.accept(fd, nil, nil)
        guard connection >= 0 else { throw failure("accept") }
        _ = fcntl(connection, F_SETFD, FD_CLOEXEC)
        var on: Int32 = 1
        setsockopt(connection, Int32(IPPROTO_TCP), TCP_NODELAY, &on, socklen_t(MemoryLayout<Int32>.size))
        // The token and a newline, one byte at a time: what follows is the
        // session's.
        var line: [UInt8] = []
        var byte: UInt8 = 0
        while line.count <= token.utf8.count, read(connection, &byte, 1) == 1, byte != 0x0A { line.append(byte) }
        guard line == Array(token.utf8) else {
            close(connection)
            throw RutisError(name: "Error", message: "RutisTest: the connection on 127.0.0.1 did not present the token")
        }
        return connection
    }
}
#endif
