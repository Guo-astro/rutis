// The dial conformance plugin (crates/rutis-host/tests/dial_conformance.rs):
// it says on stderr when its row loads and unloads, with its config, so the
// suite sees each session's rows come and go.
package arcships.rutis.conformance.plugins

import arcships.rutis.Rutis

val marker = Rutis.define<String>(name = "marker") { ctx, config ->
    System.err.println("conformance: loaded $config")
    ctx.effect { System.err.println("conformance: unloaded $config") }
}
