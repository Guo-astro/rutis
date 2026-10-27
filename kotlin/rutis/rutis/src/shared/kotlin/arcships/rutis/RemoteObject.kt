package arcships.rutis

import arcships.rutis.internal.Native
import arcships.rutis.internal.peer.Imported
import arcships.rutis.internal.peer.Peer

/**
 * An object with methods of another process (#687), received where any value
 * was expected (`Any?`): call its methods by name with native values; a
 * result arrives as a native value. It has no method table: a method it lacks
 * is its process's to answer (`no-method`). Received where a `@RutisClient`
 * interface was expected, the same object arrives as that interface's proxy.
 */
class RemoteObject internal constructor(internal val peer: Peer, internal val record: Imported) {
    /** Calls the method `method`, suspending until it returns. */
    suspend fun call(method: String, vararg args: Any?): Any? =
        Native.toNative(peer.callMethodSuspend(this, method, args.map { Native.fromNative(it) }, sync = false))

    /** Calls the method `method`, blocking this thread until it returns. */
    fun callBlocking(method: String, vararg args: Any?): Any? =
        Native.toNative(peer.callMethod(this, method, args.map { Native.fromNative(it) }, sync = true))

    /** Gives the reference back now, instead of when this object is collected. */
    fun release() = peer.release(record)

    override fun toString(): String = "RemoteObject(${record.id})"
}
