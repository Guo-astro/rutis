// The dial conformance plugin (crates/rutis-host/tests/dial_conformance.rs)
// for the Swift runtime: it says on stderr when its row loads and unloads,
// with its config, so the suite sees each session's rows come and go.
import Foundation
import Rutis

private func say(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
}

public let markerPlugin = Rutis.define(Plugin(
    name: "marker",
    config: String.self,
    apply: { ctx, config in
        say("conformance: loaded \(config)")
        ctx.effect { say("conformance: unloaded \(config)") }
    }))
