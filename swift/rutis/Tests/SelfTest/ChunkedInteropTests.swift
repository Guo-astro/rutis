// guarantee: bridge/large-and-deep-values
import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif
@testable import Rutis

/// `node` on PATH, when there is one.
private func nodePath() -> String? {
    for directory in (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":") {
        let candidate = String(directory) + "/node"
        if FileManager.default.isExecutableFile(atPath: candidate) { return candidate }
    }
    return nil
}

/// The major version of `node`, when it runs.
private func nodeMajor(_ node: String) -> Int? {
    let process = Process()
    let output = Pipe()
    process.executableURL = URL(fileURLWithPath: node)
    process.arguments = ["--version"]
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    do { try process.run() } catch { return nil }
    process.waitUntilExit()
    guard process.terminationStatus == 0 else { return nil }
    let text = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    guard text.hasPrefix("v") else { return nil }
    return Int(text.dropFirst().split(separator: ".").first ?? "")
}

private func unixError(_ what: String) -> ChannelError {
    ChannelError("\(what): \(String(cString: strerror(errno)))")
}

/// A Unix socket listener at `path`, ready for one connection.
private func listenUnix(_ path: String) throws -> Int32 {
    #if canImport(Darwin)
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    #else
    let fd = socket(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0)
    #endif
    guard fd >= 0 else { throw unixError("socket") }
    unlink(path)
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8)
    guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
        close(fd)
        throw ChannelError("socket path too long: \(path)")
    }
    withUnsafeMutableBytes(of: &address.sun_path) { raw in
        raw.copyBytes(from: bytes)
        raw[bytes.count] = 0
    }
    #if canImport(Darwin)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    #endif
    let bound = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard bound == 0 else {
        let error = unixError("bind \(path)")
        close(fd)
        throw error
    }
    guard listen(fd, 1) == 0 else {
        let error = unixError("listen \(path)")
        close(fd)
        throw error
    }
    return fd
}

/// The one connection `fd` takes, within `seconds`.
private func acceptUnix(_ fd: Int32, seconds: Int32 = 20) throws -> Int32 {
    var ready = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
    guard poll(&ready, 1, seconds * 1000) > 0 else { throw ChannelError("the far end did not connect") }
    let client = accept(fd, nil, nil)
    guard client >= 0 else { throw unixError("accept") }
    return client
}

struct ChunkedInteropTests {
    /// A real call to the Node runtime (#1363): a parameter and a result over
    /// the 16 MiB channel limit cross whole, and `tally` shows each call ran
    /// once. Skipped where there is no Node 22 or newer to run.
    func testASwiftCallToNodeCarriesValuesOverTheLimitInChunks() async throws {
        guard let node = nodePath(), let major = nodeMajor(node), major >= 22 else { return }
        let repo = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // SelfTest
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // rutis
            .deletingLastPathComponent() // swift
            .deletingLastPathComponent() // the repository
        let fixture = repo.appendingPathComponent("node/rutis-runtime/test/fixtures/conformance-session.mjs")
        guard FileManager.default.fileExists(atPath: fixture.path) else { return }
        // The runtime's channel modules import `ws`, so Node must find it.
        let modules = repo.appendingPathComponent("node/rutis-runtime/node_modules/ws/package.json")
        guard FileManager.default.fileExists(atPath: modules.path) else { return }
        let socket = "/tmp/rutis-chunk-\(getpid())-\(UUID().uuidString.prefix(8)).sock"
        let listener = try listenUnix(socket)
        defer {
            close(listener)
            unlink(socket)
        }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: node)
        process.arguments = [fixture.path, socket]
        process.standardOutput = FileHandle.nullDevice
        try process.run()
        defer {
            if process.isRunning { process.terminate() }
            process.waitUntilExit()
        }
        let channel = LineChannel(fd: try acceptUnix(listener))
        let peer = try Peer(channel: channel, options: PeerOptions(endpoint: Endpoint(local: "main", expected: "node")))
        try peer.start()
        await peer.ready()
        defer {
            peer.close()
            channel.close()
        }
        XCTAssertTrue(peer.supports("chunked"), "the Node runtime declares chunked")
        let size = maxMessage + 4096
        let big = String(repeating: "x", count: size)
        let first = try await peer.call(target: "conformance", method: "tally", args: [.data(.string(big))], sync: true)
        XCTAssertEqual(first.plainJSON?["calls"]?.int64Value, 1, "the Node runtime received the value whole, once")
        XCTAssertEqual(first.plainJSON?["size"]?.int64Value, Int64(size))
        let second = try await peer.call(target: "conformance", method: "tally", args: [.data(.string(big))], sync: true)
        XCTAssertEqual(second.plainJSON?["calls"]?.int64Value, 2, "the second call ran once, whole")
        let back = try await peer.call(target: "conformance", method: "big", args: [.data(.int(size))], sync: true)
        guard case let .data(.string(result)) = back else { return XCTFail("a result, not \(back)") }
        XCTAssertEqual(result.utf8.count, size, "the Node runtime's result came back whole")
        let after = try await peer.call(target: "conformance", method: "echo", args: [.data(.string("after"))], sync: true)
        XCTAssertEqual(after.plainJSON, .string("after"))
        XCTAssertNil(peer.error)
    }
}
