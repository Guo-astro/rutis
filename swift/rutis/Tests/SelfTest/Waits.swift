import Foundation
import XCTest

/// How long a wait in a test lasts before it fails as hung: it only guards
/// against a hang and is never what a test measures.
let hangGuardSeconds = 10.0

/// Waits until `condition` holds, at most `hangGuardSeconds`; else the test
/// fails, naming `what`.
///
/// A wait a test needs between two observations (docs/ci.md, "Waits and
/// `clock:` markers"): the tests here share this one instead of writing the
/// poll again.
func waitFor(_ what: String, _ condition: () -> Bool) async throws {
    let deadline = Date().addingTimeInterval(hangGuardSeconds)
    while !condition() {
        if Date() > deadline {
            XCTFail("timed out (\(Int(hangGuardSeconds)) s): \(what)")
            return
        }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
}
