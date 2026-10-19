// What the tests check with: a small harness of its own, so that the tests
// run with any Swift toolchain (Command Line Tools have neither XCTest nor,
// in every version, Swift Testing's macros). `swift run RutisSelfTest`.
import Foundation

final class Failures: @unchecked Sendable {
    static let shared = Failures()
    private let lock = NSLock()
    private var lines: [String] = []

    func add(_ line: String) { lock.withLock { lines.append(line) } }

    /// The failures since `mark`.
    func since(_ mark: Int) -> [String] { lock.withLock { Array(lines.dropFirst(mark)) } }
    var count: Int { lock.withLock { lines.count } }
}

struct Required: Error, CustomStringConvertible {
    let description: String
}

/// Records a failure unless `condition` holds.
func expect(_ condition: @autoclosure () throws -> Bool, _ message: @autoclosure () -> String = "",
            file: StaticString = #fileID, line: UInt = #line)
{
    do {
        if try !condition() { Failures.shared.add("\(file):\(line): expectation failed: \(message())") }
    } catch {
        Failures.shared.add("\(file):\(line): \(error) \(message())")
    }
}

/// Records a failure.
func record(_ message: String, file: StaticString = #fileID, line: UInt = #line) {
    Failures.shared.add("\(file):\(line): \(message)")
}

func record(_ error: Error, _ message: String = "", file: StaticString = #fileID, line: UInt = #line) {
    Failures.shared.add("\(file):\(line): \(error) \(message)")
}

/// The value, or a failure that ends the test.
func require<T>(_ value: @autoclosure () throws -> T?, _ message: @autoclosure () -> String = "",
                file: StaticString = #fileID, line: UInt = #line) throws -> T
{
    guard let value = try value() else { throw Required(description: "\(file):\(line): required a value: \(message())") }
    return value
}
