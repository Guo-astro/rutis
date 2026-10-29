import Foundation

// What an app tells rutis about the services its own plugins provide
// (docs/design-remote-session-2026-10-28.md §2.6, decision 2): the frames
// rutis nodes use too (crates/rutis-bridge/src/services.rs), with the
// providing plugin's name, which rutis registers as `<id>:<plugin>`.
//
//   services.announce { name, service: { method: fn, … }, shape, version, plugin }
//   services.withdraw { name, version }
//
// `version` comes from one sequence per session, increasing. Announced only
// once rutis offers the `services` family (`link.offers`), again to every
// new offer of it; this side offers `services` too, and rutis announces its
// services to it the same way (without `plugin`).

/// The argument of `services.announce`: `service` provided by `plugin` as
/// `name`, each method of `shape` (`sync` or `async`) a function reference.
func servicesAnnounce(plugin: String, name: String, service: RutisExportable, shape: [String: String], version: Int) -> Value {
    let table = service.rutisMethods()
    let methods: [(String, Value)] = shape.keys.sorted().compactMap { method in
        guard let body = table[method] else { return nil }
        let callee = Refusal.Callee(service: name, method: method)
        return (method, .function(SessionFunction { args in
            try await body(Rutis._Generated.Arguments(args, callee: callee)).value
        }))
    }
    return .record([
        ("name", .data(.string(name))),
        ("service", .record(methods)),
        ("shape", .data(.object(shape.sorted { $0.key < $1.key }.map { ($0.key, .string($0.value)) }))),
        ("version", .data(.int(version))),
        ("plugin", .data(.string(plugin))),
    ])
}

/// The argument of `services.withdraw`: `name` is no longer provided.
func servicesWithdraw(name: String, version: Int) -> Value {
    .record([("name", .data(.string(name))), ("version", .data(.int(version)))])
}

/// The argument of this side's `link.offers`: it takes `services`
/// (rutis's announcements), offered once.
func linkOffers() -> Value {
    .data(.object([("families", .array([.string("services")])), ("version", .int(1)),
                   ("since", .object([("services", .int(1))]))]))
}
