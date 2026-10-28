package arcships.rutis

import arcships.rutis.internal.Native
import arcships.rutis.internal.peer.CallChain
import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.ExportedObject
import arcships.rutis.internal.peer.RemotePlugin
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.ValueException
import arcships.rutis.internal.peer.describe
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerializationStrategy
import java.util.concurrent.ConcurrentHashMap

/**
 * Marks a class whose instances are services: the processor (`rutis-ksp`)
 * generates its method table, `<Class>_RutisService`. Its public member
 * functions, its superclasses' included, are its methods on the wire, by
 * their names; properties are not methods. Overloads are a compile error.
 */
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.BINARY)
annotation class RutisService

/**
 * Marks an interface a plugin uses a service through (`ctx.use<I>(name)`):
 * the processor generates its proxy, `<Interface>_RutisClient`. A `suspend`
 * function suspends until the service answers; a plain function blocks its
 * thread.
 */
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.BINARY)
annotation class RutisClient
