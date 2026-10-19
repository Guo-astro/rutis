// Runs every test of the registry, one after another; exits 1 when one
// failed. `swift run RutisSelfTest [prefix]` runs the tests whose name
// starts with `prefix`.
import Foundation

let only = CommandLine.arguments.dropFirst().first
var failed = 0
var ran = 0
for (name, body) in registry where only == nil || name.hasPrefix(only!) {
    let mark = Failures.shared.count
    let started = Date()
    do {
        try await body()
    } catch {
        Failures.shared.add("\(name): threw \(error)")
    }
    ran += 1
    let failures = Failures.shared.since(mark)
    let time = String(format: "%.3f", Date().timeIntervalSince(started))
    if failures.isEmpty {
        print("✔ \(name) (\(time)s)")
    } else {
        failed += 1
        print("✘ \(name) (\(time)s)")
        for failure in failures { print("    \(failure)") }
    }
}
print(failed == 0 ? "✔ \(ran) tests passed" : "✘ \(failed) of \(ran) tests failed")
exit(failed == 0 ? 0 : 1)
