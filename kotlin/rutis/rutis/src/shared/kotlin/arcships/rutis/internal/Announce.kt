package arcships.rutis.internal

import arcships.rutis.generated.ServiceTable
import arcships.rutis.internal.peer.Exported
import arcships.rutis.serve

// What an application tells rutis about the services its own plugins
// provide (docs/design-remote-session-2026-10-28.md §2.6, decision 2): the
// frames rutis nodes use too (crates/rutis-bridge/src/services.rs), with
// the providing plugin's name, which rutis registers as `<id>:<plugin>`.
//
//   services.announce { name, service: { method: fn, … }, shape, version, plugin }
//   services.withdraw { name, version }
//
// `version` comes from one sequence per session, increasing. Announced only
// once rutis offers the `services` family (`link.offers`), again to every
// new offer of it; this side offers `services` too, and rutis announces its
// services to it the same way.

/**
 * The argument of `services.announce`: `service` provided by `plugin` as
 * `name`, each method of `shape` (`sync` or `async`) a function reference.
 */
internal fun servicesAnnounce(
    plugin: String,
    name: String,
    service: Any,
    table: ServiceTable<Any>,
    shape: Map<String, String>,
    version: Long,
): Map<String, Any?> {
    val methods = LinkedHashMap<String, Any?>()
    for (method in shape.keys.sorted()) {
        if (method !in table.methods) continue
        methods[method] = Exported(Any()) { args -> table.serve(service, method, args, name) }
    }
    return linkedMapOf(
        "name" to name,
        "service" to methods,
        "shape" to shape.toSortedMap(),
        "version" to version,
        "plugin" to plugin,
    )
}

/** The argument of `services.withdraw`: `name` is no longer provided. */
internal fun servicesWithdraw(name: String, version: Long): Map<String, Any?> = linkedMapOf("name" to name, "version" to version)

/** The argument of this side's `link.offers`: it takes `services` (rutis's announcements), offered once. */
internal fun linkOffers(): Map<String, Any?> =
    linkedMapOf("families" to listOf("services"), "version" to 1L, "since" to linkedMapOf("services" to 1L))
