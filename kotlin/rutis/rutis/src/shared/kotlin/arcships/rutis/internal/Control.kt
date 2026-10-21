package arcships.rutis.internal

import arcships.rutis.Rutis
import arcships.rutis.Scope
import arcships.rutis.internal.peer.Undefined

/** The control operations rutis drives a runtime with (runtime contract §3, §4). */
internal object Control {
    private fun arg(args: List<Any?>, i: Int): Any? = Native.toNative(if (i < args.size) args[i] else Undefined)

    private fun string(args: List<Any?>, i: Int, what: String): String =
        arg(args, i) as? String ?: throw IllegalArgumentException("$what: expected a string")

    @Suppress("UNCHECKED_CAST")
    private fun record(value: Any?): Map<String, Any?> = (value as? Map<String, Any?>) ?: emptyMap()

    /** The engine this runtime runs on, for `mount`: `kotlin` and the VM. */
    fun engine(): Map<String, String> {
        val vm = System.getProperty("java.vm.name") ?: "JVM"
        val version = System.getProperty("java.version") ?: "unknown"
        return linkedMapOf("name" to "kotlin", "version" to "${KotlinVersion.CURRENT} ($vm $version)")
    }

    suspend fun control(rt: RuntimeState, method: String, args: List<Any?>): Any? = when (method) {
        "mount" -> {
            val mount = record(arg(args, 0))
            rt.mounted(mount["announce"] == true, mount["runtime"] as? String)
            linkedMapOf(
                "services" to linkedMapOf<String, Any?>(),
                "features" to FEATURES,
                "implementation" to linkedMapOf("name" to Rutis.IMPLEMENTATION, "version" to Rutis.VERSION),
                "engine" to engine(),
                "plugins" to LinkedHashMap<String, Any?>().apply { for ((name, plugin) in rt.plugins) put(name, rt.describe(plugin)) },
            )
        }
        // Served by `arrive` on the reading thread; here only when called directly.
        "events.elsewhere", "events.call" -> EventsRuntime.arrive(rt, method, args)!!.invoke()
        "dispose" -> {
            rt.closing = true
            rt.dispose()
            rt.session?.drain(rt.shutdownMillis)
            null
        }
        "rows.load" -> {
            val key = string(args, 0, "rows.load key")
            val entry = string(args, 1, "rows.load entry")
            val config = if (args.size > 2) args[2] else Undefined
            // args[4], the row's own `inject`, names services a Cordis row
            // waits for in Cordis; a leaf runtime's rows wait in rutis, which
            // gates every name its plugin injects.
            val isolate = LinkedHashMap<String, String>()
            for (pair in arg(args, 3) as? List<*> ?: emptyList<Any?>()) {
                val (name, label) = (pair as? List<*>)?.takeIf { it.size == 2 } ?: throw IllegalArgumentException("isolate: expected [name, label] pairs")
                isolate[name as? String ?: throw IllegalArgumentException("isolate: a name is not a string")] =
                    label as? String ?: throw IllegalArgumentException("isolate: a label is not a string")
            }
            val exports = LinkedHashMap<String, Map<String, String>>()
            for ((name, methods) in record(arg(args, 5))) {
                exports[name] = record(methods).mapValues { it.value as? String ?: "async" }
            }
            // The seventh, from rutis 0.13: the row's scope chain and its id.
            val placed = record(arg(args, 6))
            val scope = (placed["scope"] as? List<*> ?: emptyList<Any?>()).map {
                val s = record(it)
                Scope(s["id"] as? String ?: "", s["group"] as? String ?: "")
            }
            rt.load(key, placed["id"] as? String ?: "", entry, config, exports, isolate, scope)
            null
        }
        "rows.update" -> {
            rt.update(string(args, 0, "rows.update key"), if (args.size > 1) args[1] else Undefined)
            null
        }
        "rows.unload" -> {
            rt.unload(string(args, 0, "rows.unload key"))
            null
        }
        "rows.schema" -> rt.describe(rt.definition(string(args, 0, "rows.schema entry")))
        "hosts.provide" -> {
            val name = string(args, 0, "hosts.provide name")
            val methods = record(arg(args, 1)).mapValues { it.value as? String ?: "async" }
            val label = arg(args, 2) as? String
            val id = scopedId(name, label, label != null)
            synchronized(rt.lock) {
                if (rt.hosts.containsKey(id)) throw IllegalStateException("host service ${printable(id)} is already provided")
                rt.hosts[id] = methods
            }
            null
        }
        "hosts.withdraw" -> {
            val id = string(args, 0, "hosts.withdraw id")
            synchronized(rt.lock) { rt.hosts.remove(id) }
            null
        }
        "release" -> {
            rt.released(string(args, 0, "release handle"))
            null
        }
        "get" -> rt.property(string(args, 0, "get handle"), string(args, 1, "get property"))
        else -> throw IllegalArgumentException("unknown control method $method")
    }
}
