package arcships.rutis.sample.plugins

import android.Manifest
import android.content.Context
import android.provider.ContactsContract.CommonDataKinds.Phone
import android.provider.ContactsContract.Contacts as Table
import arcships.rutis.Rutis
import arcships.rutis.RutisService
import arcships.rutis.service
import kotlinx.serialization.Serializable

@Serializable
data class Contact(val name: String, val phones: List<String>)

/** The `contacts` service: the device's address book, read only. */
@RutisService
class Contacts(private val context: Context) {
    /** How many contacts there are. */
    fun count(): Int {
        context.require(Manifest.permission.READ_CONTACTS)
        return context.contentResolver.query(Table.CONTENT_URI, arrayOf(Table._ID), null, null, null)?.use { it.count } ?: 0
    }

    /** Contacts whose name contains `query`, at most `limit`, with their phone numbers. */
    fun search(query: String, limit: Int): List<Contact> {
        context.require(Manifest.permission.READ_CONTACTS)
        val found = LinkedHashMap<String, MutableList<String>>()
        context.contentResolver.query(
            Phone.CONTENT_URI,
            arrayOf(Phone.DISPLAY_NAME, Phone.NUMBER),
            "${Phone.DISPLAY_NAME} LIKE ?",
            arrayOf("%$query%"),
            "${Phone.DISPLAY_NAME} ASC",
        )?.use { rows ->
            while (rows.moveToNext()) {
                val name = rows.getString(0) ?: continue
                if (name !in found && found.size >= limit) break
                found.getOrPut(name) { ArrayList() }.add(rows.getString(1) ?: "")
            }
        }
        return found.map { (name, phones) -> Contact(name, phones) }
    }
}

/**
 * The plugin `contacts`. It needs READ_CONTACTS, which the application asks
 * the user for: without it the plugin does not load, and rutis says why
 * (design §六, the load-time case).
 */
fun contacts(context: Context) = Rutis.define<Rutis.NoConfig>(
    name = "contacts",
    // Temporary: an undeclared service of a connection is not yet found by
    // the other rows of its instance in rutis-loader; this goes once that
    // is fixed (declarations are optional).
    provides = mapOf("contacts" to service<Contacts>()),
) { ctx, _ ->
    context.require(Manifest.permission.READ_CONTACTS)
    ctx.provide("contacts", Contacts(context.applicationContext))
}
