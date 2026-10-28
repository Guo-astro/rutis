import RutisTest
import Testing

@testable import {{Type}}Plugin

/// The plugin runs as it would in a host, without one (guide, step 6).
@Test func greeterGreets() async throws {
    let loaded = try await RutisTest.load(plugin, config: ["greeting": "Hi"])
    let greeting = try await loaded.service("greeter").call("hello", "Ada", as: String.self)
    await loaded.close()
    #expect(greeting == "Hi, Ada!")
}
