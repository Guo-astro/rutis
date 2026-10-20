package arcships.rutis.sample

import android.content.Context
import arcships.rutis.Plugin
import arcships.rutis.RutisApp
import arcships.rutis.sample.plugins.device

/** The plugins this application brings to rutis, and the [RutisApp] that brings them. */
object Samples {
    fun plugins(context: Context): List<Plugin> = listOf(device(context))

    /**
     * The application's connection to rutis: dialing the access point
     * `connect` as `id`, presenting `token`; `trust` is the PEM of a CA
     * trusted besides the system's (a self-signed access point).
     */
    fun app(context: Context, connect: String, id: String, token: String, trust: String? = null): RutisApp =
        RutisApp(plugins = plugins(context), connect = connect, id = id, token = token, trust = trust)
}
