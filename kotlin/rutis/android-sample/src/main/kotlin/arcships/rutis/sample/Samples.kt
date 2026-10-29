package arcships.rutis.sample

import android.content.Context
import arcships.rutis.Plugin
import arcships.rutis.RutisApp
import arcships.rutis.sample.plugins.ContactsConfig
import arcships.rutis.sample.plugins.DeviceConfig
import arcships.rutis.sample.plugins.NotifyConfig
import arcships.rutis.sample.plugins.contacts
import arcships.rutis.sample.plugins.device
import arcships.rutis.sample.plugins.notify

/** The plugins this application brings to rutis, and the [RutisApp] that brings them. */
object Samples {
    /** Installed once connected, in this order. */
    fun plugins(context: Context): List<Plugin> = listOf(contacts(context), notify(context), device(context))

    /**
     * Their configurations: `notify`'s channel name; with `suffix`, every
     * service is provided as `<name>-<suffix>` (`contacts-tablet`), so that a
     * second device's services do not clash with the first's on rutis.
     */
    fun configs(suffix: String? = null): Map<String, Any?> {
        val named = { name: String -> if (suffix == null) name else "$name-$suffix" }
        return mapOf(
            "contacts" to ContactsConfig(service = named("contacts")),
            "notify" to NotifyConfig(channel = "rutis sample", service = named("notify")),
            "device" to DeviceConfig(service = named("device")),
        )
    }

    /**
     * The application's connection to rutis: dialing the access point
     * `connect` as `id`, presenting `token`; `trust` is the PEM of a CA
     * trusted besides the system's (a self-signed access point).
     */
    fun app(context: Context, connect: String, id: String, token: String, trust: String? = null, suffix: String? = null): RutisApp =
        RutisApp(plugins = plugins(context), configs = configs(suffix), connect = connect, id = id, token = token, trust = trust)
}
