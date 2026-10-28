import Foundation
@testable import Rutis

/// A local channel needs `--id`, and greets as that endpoint.
struct LocalEndpointTests {
    func aLocalChannelWithAnIDGreetsAsThatEndpoint() async throws {
        // serveArgs runs the binary again in its place (Responsibility), so
        // the test takes its two halves: the arguments, then the session.
        let (channel, endpoint, project) = try Rutis.parse(["fd:0", "--id", "swiftrun", "--peer", "host", NSTemporaryDirectory()])
        XCTAssertEqual(channel, "fd:0")
        let (runtimeSide, hostSide) = try Channels.pair()
        let plugin = Rutis.define(Plugin(name: "any") { _ in })
        let served = Task { try await Rutis.serveSession(channel: runtimeSide, endpoint: endpoint, project: project, [plugin]) }
        var options = PeerOptions(endpoint: Endpoint(local: "host", expected: "swiftrun"))
        options.dispatch = { _, method, _ in throw RutisError(name: "Error", message: "not served: \(method)") }
        let host = try Peer(channel: hostSide, options: options)
        try host.start()
        await host.ready()
        XCTAssertEqual(host.farGreeting?.endpoint, "swiftrun")
        XCTAssertTrue(host.supports("runtime"))
        let mounted = try await host.call(target: "", method: "mount", args: [.data(.object([("announce", .bool(true)), ("runtime", .string("swiftrun"))]))], sync: false)
        XCTAssertTrue(mounted.plainJSON?["features"]?.arrayValue?.contains(.string("rows.v2")) == true, "mount answered \(mounted)")
        // The host goes: the runtime's session ends and serving returns.
        host.close()
        hostSide.close()
        try await served.value
    }

    func aLocalChannelWithoutAnIDIsRefused() throws {
        let plugin = Rutis.define(Plugin(name: "any") { _ in })
        do {
            try Rutis.Advanced.serveArgs(["unix:/nowhere", NSTemporaryDirectory()], [plugin])
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(error.message, "a local channel needs --id <endpoint>: unix:/nowhere; a host older than rutis 0.19 started this runtime: upgrade the host")
        }
    }

    func anInvalidIDIsRefusedBeforeTheChannelOpens() throws {
        let plugin = Rutis.define(Plugin(name: "any") { _ in })
        do {
            try Rutis.Advanced.serveArgs(["unix:/nowhere", "--id", "Swift Run", NSTemporaryDirectory()], [plugin])
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(error.message, "invalid endpoint id Swift Run: use lower-case letters, digits and -")
        }
    }
}
