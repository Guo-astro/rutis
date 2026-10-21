// An app hosting a plugin, on macOS (crates/rutis-host/tests/swift.rs): what
// an iOS app does with `RutisApp`, driven by lines on stdin as an app's
// business layer would drive it.
//
//   RutisAppFixture dial <ws://…> <id>     dials rutis as <id>
//   RutisAppFixture listen <ws://…> <id>   listens for rutis
//
// The token is RUTIS_TOKEN. stdin: `start`, `stop`, `quit`; `start&` and
// `stop&` call it in a Task of its own and go on, as an app reacting to
// its scene phase does. stdout: every state (`state: connected`), a line
// when the observation saw a change (`observed`), and `done <command>` once
// a command returned (for `start&`, `stop&`: once that call returned). The
// device's cleanup takes 300 ms, so that a stop takes a while, as an app's
// plugins' may.
import Foundation
import Observation
import Rutis

/// The `device` service: which app this is.
@RutisService
final class Device: @unchecked Sendable {
    let id: String
    init(id: String) { self.id = id }
    public func name() -> String { id }
}

func say(_ line: String) {
    FileHandle.standardOutput.write(Data((line + "\n").utf8))
}

/// Runs `body` to its end, blocking this thread.
func blocking(_ body: @escaping @Sendable () async -> Void) {
    let done = DispatchSemaphore(value: 0)
    Task.detached {
        await body()
        done.signal()
    }
    done.wait()
}

let arguments = CommandLine.arguments
guard arguments.count == 4, ["dial", "listen"].contains(arguments[1]) else {
    FileHandle.standardError.write(Data("usage: RutisAppFixture dial|listen <address> <id>\n".utf8))
    exit(2)
}
let (mode, address, id) = (arguments[1], arguments[2], arguments[3])
let token = ProcessInfo.processInfo.environment["RUTIS_TOKEN"] ?? ""

let device = Rutis.define(Plugin(
    name: "device",
    provides: ["device": .of(Device.self, sync: ["name"])],
    apply: { ctx in
        ctx.provide("device", Device(id: id))
        ctx.effect { try? await Task.sleep(nanoseconds: 300_000_000) }
    }))

let app = mode == "dial"
    ? RutisApp(plugins: [device], connect: address, id: id, token: token, peer: "test")
    : RutisApp(plugins: [device], listen: address, id: id, token: token)

Task.detached {
    for await state in app.states { say("state: \(state)") }
}

/// Observation, as SwiftUI observes it: once per change.
@Sendable func observe() {
    withObservationTracking {
        _ = app.state
    } onChange: {
        say("observed")
        DispatchQueue.global().async { observe() }
    }
}
observe()

while let command = readLine() {
    switch command {
    case "start":
        blocking {
            do {
                try await app.start()
            } catch {
                say("error: \(error)")
            }
        }
    case "stop":
        blocking { await app.stop() }
    case "start&":
        Task.detached {
            do {
                try await app.start()
            } catch {
                say("error: \(error)")
            }
            say("done start&")
        }
        // The call made before the next command's.
        usleep(50000)
        continue
    case "stop&":
        Task.detached {
            await app.stop()
            say("done stop&")
        }
        usleep(50000)
        continue
    case "quit":
        blocking { await app.stop() }
        say("done quit")
        exit(0)
    default:
        say("error: unknown command \(command)")
        continue
    }
    say("done \(command)")
}
blocking { await app.stop() }
