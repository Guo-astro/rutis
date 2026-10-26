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

struct ThrowSiteTests {
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
        let wire = encodeError(far, frame: "    at Account.relay (x.swift:1)") { try ValueCodec.encodeAny($0) }
        XCTAssertEqual(wire["graph"], far.graph)
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
