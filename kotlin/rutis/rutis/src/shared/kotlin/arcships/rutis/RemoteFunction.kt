package arcships.rutis

import arcships.rutis.internal.Native
import arcships.rutis.internal.peer.Imported
import arcships.rutis.internal.peer.Peer

/**
 * A function of another process, received where any value was expected
 * (`Any?`): call it with native values; its result arrives as a native value.
 * A call waits for the result, synchronously on the wire (the far end may
 * call back into this process meanwhile).
 */
class RemoteFunction internal constructor(internal val peer: Peer, internal val record: Imported) {
    /** Calls the function, suspending until it returns. */
    suspend operator fun invoke(vararg args: Any?): Any? =
        Native.toNative(peer.callFunctionSuspend(this, args.map { Native.fromNative(it) }))

    /** Calls the function, blocking this thread until it returns. */
    fun call(vararg args: Any?): Any? =
        Native.toNative(peer.callFunction(this, args.map { Native.fromNative(it) }))

    /** Gives the reference back now, instead of when this object is collected. */
    fun release() = peer.release(record)

    override fun toString(): String = "RemoteFunction(${record.id})"
}
