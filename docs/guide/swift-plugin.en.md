# Write a Swift Plugin

[中文](swift-plugin.md)

From creating a project to publishing it. Needs macOS 14 or later, Xcode 26 or later with the Swift 6.2 toolchain (`swift test` needs Xcode's test frameworks), and Node.js 22 or later (for `npx` and the fake service of step 7). Swift is the language for what macOS offers its own programs: calendars and reminders (EventKit), contacts, accessibility and Apple events automation, screen capture, Core ML.

## 1. Start a project

<!-- example: swift-en run -->
```bash
npx @arcships/rutis-host new greeter --lang swift
cd greeter
swift test
```

A Swift plugin is compiled into a binary, and a binary is one runtime process in the host. The project is a SwiftPM package:

| File | What it is |
| --- | --- |
| `Sources/GreeterPlugin/Plugin.swift` | The plugin (library `GreeterPlugin`, exporting `plugin`) |
| `Sources/greeter/main.swift` | The binary: `Rutis.serve(plugin)` |
| `Tests/GreeterPluginTests/PluginTests.swift` | Tests, no host needed |
| `rutis.dev.json` | The rows of the local host (step 7) |
| `Info.plist` | Linked into the binary, for macOS's access prompts (see [System access](#system-access)) |
| `Package.swift` | Depends on `https://github.com/arcships/rutis` (products `Rutis` and `RutisTest`) |
| `.github/workflows/release.yml` | On a `v*` tag, tests and publishes the binary (step 8) |

The first build compiles the SDK's macros (swift-syntax, at compile time only), which takes a minute or two; Xcode asks once whether to trust them.

## 2. Provide a service

The generated `Sources/GreeterPlugin/Plugin.swift` is a plugin that provides the service `greeter`:

<!-- example: swift-en file=greeter/Sources/GreeterPlugin/Plugin.swift -->
```swift
// The greeter rutis plugin. The Swift plugin guide goes on from here:
// https://github.com/arcships/rutis/blob/main/docs/guide/swift-plugin.en.md
import Rutis

/// The plugin's configuration (guide, step 4): `@RutisConfig` derives its
/// JSON Schema; a `var` with a literal initial value is its default, filled
/// in when the row leaves it out.
@RutisConfig
public struct Config: Decodable {
    /// how to greet
    public var greeting = "Hello"
}

/// The service the plugin provides (guide, step 2): `@RutisService` makes
/// its methods the service's.
@RutisService
public final class Greeter: @unchecked Sendable {
    let greeting: String

    init(greeting: String) { self.greeting = greeting }

    public func hello(_ name: String) -> String { "\(greeting), \(name)!" }
}

public let plugin = Rutis.define(Plugin(
    name: "greeter",
    // The services it uses (guide, step 3): it starts once they are all there.
    inject: [],
    // What it provides, so that `rutis-host check` knows before it runs.
    provides: ["greeter": .of(Greeter.self)],
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("greeter", Greeter(greeting: config.greeting))
    }
))
```

- `Rutis.define(Plugin(...))` defines the plugin; `name` is its name in the binary. When a host loads the plugin and starts it, it calls `apply`.
- `@RutisService` makes each public method of the class a method of the service (`hello`), and `ctx.provide("greeter", …)` provides the service: a plugin in any language can call it by the name `greeter`, and it is withdrawn when the plugin unloads.
- `provides` says which services the plugin provides, so the host knows without running it.

## 3. Use a service

Now `greeter` first has the service `translate` translate its greeting. Another plugin provides `translate`, in any language:

<!-- example: swift-en file=greeter/Sources/GreeterPlugin/Plugin.swift -->
```swift
import Rutis

@RutisConfig
public struct Config: Decodable {
    /// how to greet
    public var greeting = "Hello"
}

@RutisClient
public protocol Translate {
    func text(_ text: String) async throws -> String
}

@RutisService
public final class Greeter: @unchecked Sendable {
    let translate: any Translate
    let greeting: String

    init(translate: any Translate, greeting: String) {
        self.translate = translate
        self.greeting = greeting
    }

    public func hello(_ name: String) async throws -> String {
        "\(try await translate.text(greeting)), \(name)!"
    }
}

public let plugin = Rutis.define(Plugin(
    name: "greeter",
    inject: ["translate"],
    provides: ["greeter": .of(Greeter.self)],
    config: Config.self,
    apply: { ctx, config in
        let translate = try ctx.use("translate", TranslateProxy.self)
        ctx.provide("greeter", Greeter(translate: translate, greeting: config.greeting))
    }
))
```

- `inject` lists the services the plugin uses. The plugin starts once they are all there; when one is withdrawn, the plugin stops, and it starts again when the service is back.
- `@RutisClient` makes `TranslateProxy` from the protocol; `ctx.use("translate", TranslateProxy.self)` returns an `any Translate`, and calling its methods calls the service. The methods of a service you use are `async throws`.

The template's own test now fails (it gives no `translate`); step 6 replaces it.

## 4. Configuration

`Config` is the plugin's configuration: `@RutisConfig` derives its JSON Schema, and the literal of `var greeting = "Hello"` is its default. Before `apply`, rutis fills in the defaults and validates the configuration, so `config.greeting` in `apply` always has a value; a row whose configuration does not pass fails with `config: …`, and `apply` is not called.

Where the configuration comes from: in a test, `RutisTest.load(plugin, config: ["greeting": "Hi"])` passes it (step 6); when it runs, it is the `config` of the plugin's row in the host (`rutis.dev.json` in step 7). Without `greeting`, it is `Hello`.

## 5. Emit and listen to events

After each greeting, `hello` emits the event `greeter/greeted`; the same plugin listens to it to count the greetings, which the new method `count` returns:

<!-- example: swift-en file=greeter/Sources/GreeterPlugin/Plugin.swift -->
```swift
import Rutis

@RutisConfig
public struct Config: Decodable {
    /// how to greet
    public var greeting = "Hello"
}

@RutisClient
public protocol Translate {
    func text(_ text: String) async throws -> String
}

actor Greeted {
    var count = 0
    func add() { count += 1 }
}

@RutisService
public final class Greeter: @unchecked Sendable {
    let ctx: Rutis.Ctx
    let translate: any Translate
    let greeting: String
    let greeted: Greeted

    init(ctx: Rutis.Ctx, translate: any Translate, greeting: String, greeted: Greeted) {
        self.ctx = ctx
        self.translate = translate
        self.greeting = greeting
        self.greeted = greeted
    }

    public func hello(_ name: String) async throws -> String {
        let text = "\(try await translate.text(greeting)), \(name)!"
        try ctx.emit("greeter/greeted", name)
        return text
    }

    public func count() async -> Int { await greeted.count }
}

public let plugin = Rutis.define(Plugin(
    name: "greeter",
    inject: ["translate"],
    provides: ["greeter": .of(Greeter.self)],
    config: Config.self,
    apply: { ctx, config in
        let greeted = Greeted()
        ctx.on("greeter/greeted") { (_: String) in await greeted.add() }
        let translate = try ctx.use("translate", TranslateProxy.self)
        ctx.provide("greeter", Greeter(ctx: ctx, translate: translate, greeting: config.greeting, greeted: greeted))
    }
))
```

- `ctx.emit(name, arguments…)` emits the event and returns at once, without waiting for the listeners.
- `ctx.on(name) { … }` listens to the event; the closure's parameters are the event's arguments, decoded by their declared types. The listener is withdrawn when the plugin unloads.
- A listener gets the event whatever process or language it is in: other plugins can listen to `greeter/greeted` too.

The other ways to emit (wait for all listeners, take the first result, chain them) are in [Plugin API: events](plugin-api.en.md#events).

## 6. Test

`RutisTest` runs the plugin as a row of a real host: `rutis-host test`, which it starts itself (`rutis-host` on `PATH`, or its path in `RUTIS_HOST`; `npm install --global @arcships/rutis-host`). The test gives the plugin fake services, calls the services it provides and listens to its events from a test plugin in the same runtime. Replace `Tests/GreeterPluginTests/PluginTests.swift` with:

<!-- example: swift-en file=greeter/Tests/GreeterPluginTests/PluginTests.swift -->
```swift
import Rutis
import RutisTest
import Testing

@testable import GreeterPlugin

@RutisService
final class FakeTranslate: @unchecked Sendable {
    func text(_ text: String) -> String { ["Hello": "Bonjour", "Hi": "Salut"][text] ?? text }
}

@Test func greeterGreetsInTranslation() async throws {
    let loaded = try await RutisTest.load(plugin, config: ["greeting": "Hi"], services: ["translate": FakeTranslate()])
    let (greeted, received) = AsyncStream.makeStream(of: String.self)
    let off = await loaded.on("greeter/greeted") { (name: String) in _ = received.yield(name) }
    let greeter = try await loaded.service("greeter")
    #expect(try await greeter.call("hello", "Ada", as: String.self) == "Salut, Ada!")
    var names = greeted.makeAsyncIterator()
    #expect(await names.next() == "Ada")
    #expect(try await greeter.call("count", as: Int.self) == 1)
    await off()
    await loaded.close()
}
```

<!-- example: swift-en run -->
```bash
swift test
```

- `RutisTest.load(plugin, config:, services:)` loads the plugin: `config` is the row's configuration, `services` the fake services (instances of `@RutisService` types); give every service the plugin injects.
- `loaded.service("greeter").call("hello", "Ada", as: String.self)` calls the service as another plugin would.
- `loaded.on(name) { … }` listens to an event. The host dispatches it, so the test waits for it to arrive (here with an `AsyncStream`) rather than checking when the call returns.
- `loaded.close()` unloads the plugin; the host goes on for the next test.

The host checks the row's config, gates the row on the services it injects and dispatches its events, as when it runs. The rest of the test tool is in [Plugin API: the Swift test tool](plugin-api.en.md#the-swift-test-tool).

## 7. Run it locally and try it

`rutis-host dev` runs the plugin in a local host. What it runs are the **rows** in `rutis.dev.json`: each row is one plugin running in the host. The plugin uses `translate`, so a row has to provide it. Swift has no fake service of its own; write one in TypeScript, in `dev/`:

<!-- example: swift-en file=greeter/dev/fake-translate.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

const words: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }

export default definePlugin({
  provides: { translate: { text: 'async' } },
  apply(ctx) {
    ctx.provide('translate', { text: async (text: string) => words[text] ?? text })
  },
})
```

<!-- example: swift-en run -->
```bash
npm install --prefix dev @arcships/rutis
```

Make `rutis.dev.json` two rows:

<!-- example: swift-en file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "runtimes": { "node": { "project": "dev" } },
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "./dev/fake-translate.ts" }
  ]
}
```

- The row with the plugin's id (`greeter`) is the plugin itself: it has no `name`, and `config` is its configuration.
- The other row, `translate`, runs `dev/fake-translate.ts`, which provides `translate`; `runtimes.node` has the host run it with the packages installed in `dev/`.

<!-- example: swift-en run until="greeter: running" -->
```bash
npx @arcships/rutis-host dev
```

`dev` builds the binary with `swift build`, and starts the two rows. `rutis-host dev: services:` gives the services there are and the row providing each; it is printed once every row is running, waiting or failed, and again whenever the services change, before or after the rows' state lines:

```text
greeter: building
greeter: built in 3.2s
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
greeter: starting
translate: running
rutis-host dev: services: greeter (greeter), loader (host), sessions (host), translate (translate)
greeter: running
```

While `dev` runs, in another terminal in the project directory, see the host's state and call the service once:

```console
$ npx @arcships/rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  sessions: host
  translate: translate
runtimes:
  node: running (pid 66737)
  swift-greeter: running (pid 66740)
calls:
  none
$ npx @arcships/rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ npx @arcships/rutis-host call greeter.count
1
```

Each argument is a JSON value (a string in quotes); an error goes to standard error with exit status 1 (see [rutis-host: calling a service](rutis-host.en.md#calling-a-service)).

In `Plugin.swift`, change `\(name)!` in `hello` to `\(name)!!` and save: `dev` rebuilds, restarts only the Swift runtime, and `greeter` runs the new code (while it restarts, `greeter` is briefly missing from the services):

```text
greeter: building
greeter: built in 1.1s
swift-greeter: rebuilt and restarted
rutis-host dev: services: loader (host), sessions (host), translate (translate)
rutis-host dev: services: greeter (greeter), loader (host), sessions (host), translate (translate)
```

```console
$ npx @arcships/rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!!"
```

When the build fails, it prints the compiler's errors and the last build keeps running. Changing `dev/fake-translate.ts` reloads only the row `translate` (`translate: reloaded (dev/fake-translate.ts changed)`), without a rebuild. The rows' status lines may come in a different order each time.

Ctrl-C ends `dev`. The rest of `dev`'s rules are in [rutis-host: developing a plugin with dev](rutis-host.en.md#developing-a-plugin-with-dev). To run on another machine, see the remote section of [rutis-host.en.md](rutis-host.en.md).

## 8. Publish

A Swift plugin is published as a binary, not to a package registry. Push a tag, say `v0.1.0`: the template's workflow, on a GitHub macOS runner, runs the tests and `rutis-host check`, builds a universal binary for arm64 and x86_64, and publishes it with `SHA256SUMS` to the repository's GitHub Release. With the secrets it names (a Developer ID certificate and notarization credentials), the binary is signed and notarized; without them it is ad-hoc signed, and a host that downloads it must trust it first.

Dependency range: `.package(url: "https://github.com/arcships/rutis", from: "<version>")` in `Package.swift` accepts the SDK from that version up to the next major version.

How a host installs your plugin: [rutis-host: how a host installs plugins](rutis-host.en.md#how-a-host-installs-plugins).

## Debugging

`dev --debug` does not support Swift projects. Attach LLDB to the running runtime: its process id is under `runtimes` in `rutis-host status` (`swift-greeter: running (pid 66740)`).

```bash
lldb -p 66740
```

```
(lldb) breakpoint set -n GreeterPlugin.Greeter.hello
(lldb) continue
```

It stops when a call reaches `hello`; `bt` shows the stack, `frame variable` the variables, and `detach` disconnects and lets the runtime go on.

- While stopped at a breakpoint, the whole runtime is stopped: calls to the other plugins of the binary wait too.
- Each rebuild is a new process: attach again and set the breakpoints again.
- The first time you debug on macOS, the system asks to allow developer tools.
- What `print` writes appears in `dev`'s output.
- Tests need no host: open `Package.swift` in Xcode and debug a test there.
- When a row stays `waiting` or fails, `rutis-host status` says what it waits for, or the first line of the failure.

## System access

macOS asks the user before a program reads calendars, contacts and the like, and remembers the answer for that program. For a Swift plugin, that program is **the plugin binary**: the runtime makes itself responsible for its own requests when it starts (whoever started it: `rutis-host`, a terminal, a process manager), so the prompt names the binary, and what the user grants applies to it alone, not to the host or the terminal. Plugins in one binary share its access; plugins that need different access go in different binaries.

- The prompt shows the binary's name (`CFBundleName` in `Info.plist`) and the usage description of what is asked for: add the key (`NSCalendarsFullAccessUsageDescription`, `NSRemindersFullAccessUsageDescription`, `NSContactsUsageDescription`, `NSAppleEventsUsageDescription`, ...) to `Info.plist`. The template's `Package.swift` links it into the binary.
- A denial is an ordinary failure: check the answer and throw, and the caller, in any language, gets the error:

```swift
import EventKit
import Foundation
import Rutis

struct CalendarAccessDenied: Error, LocalizedError {
    let code = "denied"
    var errorDescription: String? { "calendar access is not granted to this plugin" }
}

@RutisService
public final class Calendar: @unchecked Sendable {
    let store = EKEventStore()

    public func titles(_ days: Int) async throws -> [String] {
        guard try await store.requestFullAccessToEvents() else { throw CalendarAccessDenied() }
        let start = Date()
        let end = start.addingTimeInterval(Double(days) * 86_400)
        let events = store.events(matching: store.predicateForEvents(withStart: start, end: end, calendars: nil))
        return events.map { $0.title ?? "" }
    }
}
```

- Local builds are ad-hoc signed (`swift build` does it): macOS knows the binary by its signature, so every rebuild is a new program to it and may ask again. `tccutil reset Calendar <CFBundleIdentifier>` forgets an answer. A binary signed with a Developer ID keeps its access across versions.

To provide plugins from an iOS, iPadOS or macOS app, see [Hosting plugins in an app](swift-app.en.md).

## Advanced: plugin API

Plugin definitions as values, objects with methods, how values cross, errors, sync methods, cancellation and more: [Plugin API](plugin-api.en.md).
