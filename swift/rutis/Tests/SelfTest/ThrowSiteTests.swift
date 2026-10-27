import Foundation
@testable import Rutis

// #677 §4.4: Swift errors carry no stack, so what a service method throws
// crosses with where the method is declared.

struct Overdrawn: Error, LocalizedError {
    var errorDescription: String? { "no money left" }
}

let throwingServiceLine = #line + 3
@RutisService
final class Account {
    func withdraw(_ amount: Int) throws -> Int {
        throw Overdrawn()
    }

    func relay() async throws {
        throw RutisError(name: "Far", message: "from the far end")
    }

    func balance() -> Int { 3 }
}

/// Throws on an error it received from the far end.
let passingOnLine = #line + 4
@RutisService
final class Middle {
    var received: RutisError? = nil
    func passOn() throws {
        throw received!
    }
}

struct ThrowSiteTests {
    /// The root node of the graph `error` arrived with.
    func rootNode(_ graph: JSON?) -> JSON? {
        guard let root = graph?["root"]?["value"]?.int64Value, let nodes = graph?["nodes"]?.arrayValue,
              Int(root) < nodes.count
        else { return nil }
        return nodes[Int(root)]
    }

    /// A received error, as decoded from the far end's throw.
    func received(_ error: Error) -> RutisError {
        decodeError(encodeError(error) { try ValueCodec.encodeAny($0) }) { ValueCodec.native($0) }
    }

    func aReceivedErrorThrownOnByAMethodNamesItInRethrown() async throws {
        let middle = Middle()
        var far = RutisError(name: "Far", message: "from the far end", code: "E_FAR", data: ["n": 1])
        far.cause = RutisError(name: "Under", message: "below")
        middle.received = received(far)
        let stored = middle.received!.graph
        let (host, runtime) = try await pairOfPeers { _, method, args in
            guard let body = middle.rutisMethods()[method] else { throw RutisError(name: "Error", message: "no \(method)") }
            return try await body(Rutis.Arguments(args)).value
        }
        defer {
            host.close()
            runtime.close()
        }
        do {
            _ = try await host.call(target: "t", method: "passOn", args: [], sync: false)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(error.name, "Far")
            XCTAssertEqual(error.code, "E_FAR")
            XCTAssertEqual(error.cause?.name, "Under")
            XCTAssertEqual(String(describing: error.data ?? "nil"), String(describing: middle.received!.data ?? "nil"))
            let root = rootNode(error.graph)
            XCTAssertEqual(root?["rethrown"],
                           .array([.string("    at Middle.passOn (RutisSelfTest/ThrowSiteTests.swift:\(passingOnLine))")]))
            // Everything else as it arrived.
            let before = rootNode(stored)
            XCTAssertEqual(root?["stack"], before?["stack"])
            XCTAssertEqual(root?["code"], before?["code"])
            XCTAssertEqual(root?["data"], before?["data"])
            XCTAssertEqual(error.graph?["nodes"]?.arrayValue?.count, stored?["nodes"]?.arrayValue?.count)
        }
        // The stored graph is not changed.
        XCTAssertEqual(middle.received!.graph, stored)
        XCTAssertNil(rootNode(stored)?["rethrown"])
    }

    func rethrownHoldsAtMostEight() {
        var error = received(RutisError(name: "Far", message: "m"))
        for i in 0 ..< 10 {
            let wire = encodeError(error, frame: "    at T.m\(i) (x.swift:\(i))") { try ValueCodec.encodeAny($0) }
            error = decodeError(wire) { ValueCodec.native($0) }
        }
        let items = rootNode(error.graph)?["rethrown"]?.arrayValue ?? []
        XCTAssertEqual(items.count, 8)
        XCTAssertEqual(items.first, .string("    at T.m0 (x.swift:0)"))
        XCTAssertEqual(items.last, .string("    at T.m7 (x.swift:7)"))
        // A locally thrown error has none.
        let local = encodeError(RutisError(name: "Here", message: "m"), frame: "    at T.m (x.swift:1)") { try ValueCodec.encodeAny($0) }
        XCTAssertNil(rootNode(local["graph"])?["rethrown"])
    }

    /// The stack of the root of the error graph `error` arrived with.
    func stack(_ error: RutisError) -> String? {
        guard let root = error.graph?["root"]?["value"]?.int64Value, let nodes = error.graph?["nodes"]?.arrayValue,
              Int(root) < nodes.count
        else { return nil }
        return nodes[Int(root)]["stack"]?.stringValue
    }

    func aMethodThatThrowsSendsWhereItIsDeclared() async throws {
        let account = Account()
        let (host, runtime) = try await pairOfPeers { _, method, args in
            guard let body = account.rutisMethods()[method] else { throw RutisError(name: "Error", message: "no \(method)") }
            return try await body(Rutis.Arguments(args)).value
        }
        defer {
            host.close()
            runtime.close()
        }
        do {
            _ = try await host.call(target: "t", method: "withdraw", args: [.data(.int(5))], sync: false)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(error.name, "Overdrawn")
            XCTAssertEqual(error.message, "no money left")
            XCTAssertEqual(stack(error), "Overdrawn: no money left\n    at Account.withdraw (RutisSelfTest/ThrowSiteTests.swift:\(throwingServiceLine))")
        }
        // A method that fails before it runs (a bad argument) is not located.
        do {
            _ = try await host.call(target: "t", method: "withdraw", args: [.data(.string("x"))], sync: false)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(stack(error)?.contains("\n"), false, "\(stack(error) ?? "")")
        }
    }

    func aReceivedErrorThrownOnKeepsItsGraph() async throws {
        let account = Account()
        let (host, runtime) = try await pairOfPeers { _, method, args in
            guard let body = account.rutisMethods()[method] else { throw RutisError(name: "Error", message: "no \(method)") }
            return try await body(Rutis.Arguments(args)).value
        }
        defer {
            host.close()
            runtime.close()
        }
        // Received, then thrown on by a method: it crosses as it arrived.
        let far = decodeError(encodeError(RutisError(name: "Far", message: "from the far end")) { try ValueCodec.encodeAny($0) }) { ValueCodec.native($0) }
        XCTAssertEqual(encodeError(far) { try ValueCodec.encodeAny($0) }["graph"], far.graph)
        do {
            _ = try await host.call(target: "t", method: "relay", args: [], sync: false)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(error.name, "Far")
            XCTAssertTrue(stack(error)?.hasPrefix("Far: from the far end\n    at Account.relay (RutisSelfTest/ThrowSiteTests.swift:") == true, "\(stack(error) ?? "")")
        }
    }

    /// Thrown in process, the error is the one the method threw.
    func inProcessTheErrorIsUnchanged() async throws {
        do {
            _ = try await Account().rutisMethods()["withdraw"]!(Rutis.Arguments([.data(.int(1))]))
            XCTFail("expected an error")
        } catch {
            XCTAssertTrue(error is Overdrawn, "\(error)")
        }
    }
}
