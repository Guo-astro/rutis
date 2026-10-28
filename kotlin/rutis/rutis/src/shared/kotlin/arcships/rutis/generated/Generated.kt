package arcships.rutis.generated

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
import arcships.rutis.Plugin
import arcships.rutis.advanced.RemoteFunction
import arcships.rutis.advanced.RemoteObject
import arcships.rutis.RutisException
import arcships.rutis.advanced.RutisFormat
import arcships.rutis.ArgumentRefusal
import arcships.rutis.Refusals
import arcships.rutis.serve

/** The method table of a `@RutisService` class (generated). */
abstract class ServiceTable<T : Any> {
    /** The methods' wire names. */
    abstract val methods: List<String>

    /** Calls `method` on `target` with session values, returning a session value. */
    abstract suspend fun invoke(target: T, method: String, args: List<Any?>): Any?
}

/** What a generated proxy calls a service through. */
interface ServiceHandle {
    /** The service's name, for errors. */
    val name: String

    /** Each method's kind: `sync` or `async`. */
    val kinds: Map<String, String>

    suspend fun callSuspend(method: String, args: List<Any?>): Any?

    fun callBlocking(method: String, args: List<Any?>): Any?
}

/**
 * What a generated proxy also is (generated code implements it, as it uses
 * [ServiceHandle]): the handle it calls through. Handed on, a proxy of an
 * object of another process is that object again (#687).
 */
interface ObjectProxy {
    val rutisHandle: ServiceHandle
}

/** The object of another process the proxy calls, if it does. */
internal val ObjectProxy.rutisObject: RemoteObject? get() = (rutisHandle as? ObjectHandle)?.target

/** The handle of an object of another process: no method table, every method called by name (#687). */
internal class ObjectHandle(val target: RemoteObject) : ServiceHandle {
    override val name: String get() = ""
    override val kinds: Map<String, String> get() = emptyMap()

    override suspend fun callSuspend(method: String, args: List<Any?>): Any? =
        target.peer.callMethodSuspend(target, method, args, sync = false)

    override fun callBlocking(method: String, args: List<Any?>): Any? =
        target.peer.callMethod(target, method, args, sync = true)
}

/** Makes the proxies of a `@RutisClient` interface (generated). */
abstract class ClientFactory<I : Any> {
    /** The methods the interface calls, by wire name. */
    abstract val methods: List<String>

    abstract fun create(handle: ServiceHandle): I
}

/** Finds the generated tables and proxies by name, as Room and Moshi do (R8 keeps the names). */
object Tables {
    private val services = ConcurrentHashMap<Class<*>, Any>()
    private val clients = ConcurrentHashMap<Class<*>, Any>()
    private val none = Any()

    private fun generatedName(type: Class<*>, suffix: String): String {
        val pkg = type.`package`?.name ?: ""
        val simple = type.name.removePrefix(if (pkg.isEmpty()) "" else "$pkg.").replace('$', '_')
        return (if (pkg.isEmpty()) "" else "$pkg.") + simple + suffix
    }

    private fun load(name: String, loader: ClassLoader?): Any? = try {
        val generated = Class.forName(name, true, loader)
        generated.getDeclaredConstructor().newInstance()
    } catch (_: ClassNotFoundException) {
        null
    } catch (_: NoClassDefFoundError) {
        null
    }

    /** The method table of `type` or of the nearest superclass that has one. */
    @Suppress("UNCHECKED_CAST")
    fun serviceTable(type: Class<*>): ServiceTable<Any>? {
        val found = services.getOrPut(type) {
            var current: Class<*>? = type
            var table: Any? = null
            while (current != null && current != Any::class.java && table == null) {
                table = load(generatedName(current, "_RutisService"), current.classLoader)
                current = current.superclass
            }
            table ?: none
        }
        return if (found === none) null else found as ServiceTable<Any>
    }

    /** The proxy factory of the `@RutisClient` interface `type`. */
    @Suppress("UNCHECKED_CAST")
    fun <I : Any> clientFactory(type: Class<I>): ClientFactory<I>? {
        val found = clients.getOrPut(type) { load(generatedName(type, "_RutisClient"), type.classLoader) ?: none }
        return if (found === none) null else found as ClientFactory<I>
    }
}

/** A service object exported by reference: the far end calls its method table. */
internal class ServiceObject(override val original: Any, private val table: ServiceTable<Any>) : ExportedObject {
    override suspend fun call(method: String, args: List<Any?>): Any? =
        table.serve(original, method, args, original.javaClass.simpleName.ifEmpty { "the object" }, isObject = true)

    override suspend fun get(property: String): Any? = throw RutisException("Error", "no property $property: a service's methods cross, not its fields")
}

/** What generated code calls. Not for plugin code. */
object Generated {
    /** Argument `i` of a call, decoded. */
    fun <T> arg(args: List<Any?>, i: Int, deserializer: DeserializationStrategy<T>, method: String): T {
        val value = if (i < args.size) args[i] else Undefined
        return try {
            RutisFormat.decode(deserializer, value)
        } catch (e: RutisException) {
            throw e
        } catch (e: Exception) {
            val expected = Refusals.expected(deserializer.descriptor)
            if (value === Undefined) throw ArgumentRefusal(method, i, expected, null)
            // An object is taken only by a @RutisClient interface or Any? (#687).
            val reason = if (value is RemoteObject) "an object is received only as a @RutisClient interface or Any?" else Refusals.reason(value, e)
            throw ArgumentRefusal(method, i, expected, Refusals.shown(value) + (reason?.let { " ($it)" } ?: ""))
        }
    }

    /** Argument `i` of a call as a native value (a parameter of type `Any?`). */
    fun native(args: List<Any?>, i: Int): Any? = Native.toNative(if (i < args.size) args[i] else Undefined)

    /** A result, encoded. */
    fun <T> result(serializer: SerializationStrategy<T>, value: T): Any? = RutisFormat.encode(serializer, value)

    /** A result of type `Any?`, encoded by what it is. */
    fun nativeResult(value: Any?): Any? = Native.fromNative(value)

    /** A value as a session value, by its serializer. */
    fun <T> encode(serializer: SerializationStrategy<T>, value: T): Any? = RutisFormat.encode(serializer, value)

    /** A session value as a `T`. */
    fun <T> decode(deserializer: DeserializationStrategy<T>, value: Any?): T = RutisFormat.decode(deserializer, value)

    /** A session value as a native value. */
    fun toNative(value: Any?): Any? = Native.toNative(value)

    /**
     * Argument `i`, a function: this process's own function when it came home
     * (`F`), else null and [remote] gives the far end's.
     */
    @Suppress("UNCHECKED_CAST")
    fun <F> local(args: List<Any?>, i: Int): F? = ((if (i < args.size) args[i] else null) as? Exported)?.original as? F

    /** Argument `i`, a function of the far end. */
    fun remote(args: List<Any?>, i: Int, method: String): RemoteFunction {
        val value = if (i < args.size) args[i] else Undefined
        return value as? RemoteFunction
            ?: throw ArgumentRefusal(method, i, "a function", if (value === Undefined) null else Refusals.shown(value))
    }

    /** A function result of the far end. */
    fun remoteResult(value: Any?): RemoteFunction =
        value as? RemoteFunction ?: throw ValueException("expected a function, got ${describe(value)}")

    /** Calls a function of the far end with session values, blocking. */
    fun callBlocking(function: RemoteFunction, args: List<Any?>): Any? = function.peer.callFunction(function, args)

    /** Calls a function of the far end with session values, suspending. */
    suspend fun callSuspend(function: RemoteFunction, args: List<Any?>): Any? = function.peer.callFunctionSuspend(function, args)

    /** A Kotlin function exported: the far end calls `body` with session values. */
    fun export(function: Any, body: suspend (List<Any?>) -> Any?): Any = Exported(function, body)

    /**
     * Argument `i`, a plugin definition (a parameter of type [Plugin]): one
     * of this process's, back home, or the definition itself handed to a
     * test double; null for none when [nullable].
     */
    fun pluginArg(args: List<Any?>, i: Int, method: String, nullable: Boolean): Plugin? {
        val value = if (i < args.size) args[i] else Undefined
        return when (value) {
            is Plugin, is RemotePlugin -> plugin(value, nullable) { "$method: argument ${i + 1}: $it" }
            null, Undefined, kotlinx.serialization.json.JsonNull ->
                if (nullable) null else throw ArgumentRefusal(method, i, "a plugin definition", if (value === Undefined) null else "null")
            else -> throw ArgumentRefusal(method, i, "a plugin definition", Refusals.shown(value))
        }
    }

    /** Argument `i` of type `Any` (not null): missing or null is refused. */
    fun nativeArg(args: List<Any?>, i: Int, method: String): Any {
        val value = if (i < args.size) args[i] else Undefined
        return Native.toNative(value) ?: throw ArgumentRefusal(method, i, "a value", if (value === Undefined) null else "null")
    }

    /** A result, a plugin definition. */
    fun pluginResult(value: Any?, nullable: Boolean): Plugin? = plugin(value, nullable) { it }

    private fun plugin(value: Any?, nullable: Boolean, where: (String) -> String): Plugin? = when (value) {
        is Plugin -> value
        is RemotePlugin -> value.local ?: throw ValueException(where("a plugin definition of another process can only be handed back to it"))
        null, Undefined, kotlinx.serialization.json.JsonNull -> if (nullable) null else throw ValueException(where("expected a plugin definition, got null"))
        else -> throw ValueException(where("expected a plugin definition, got ${describe(value)}"))
    }

    /**
     * Argument `i` of a `@RutisClient` interface type `type`: an object of
     * another process as that interface's proxy, or this process's own object
     * back home (#687 §四); null for none when [nullable].
     */
    fun <I : Any> clientArg(args: List<Any?>, i: Int, method: String, type: Class<I>, nullable: Boolean): I? {
        val value = if (i < args.size) args[i] else Undefined
        return when (value) {
            null, Undefined, kotlinx.serialization.json.JsonNull ->
                if (nullable) null else throw ArgumentRefusal(method, i, Refusals.article(type.simpleName), if (value === Undefined) null else "null")
            else -> client(value, type)
                ?: throw ArgumentRefusal(method, i, Refusals.article(type.simpleName), Refusals.shown(value) + if (value is RemoteObject) " (an object is received only as a @RutisClient interface or Any?)" else "")
        }
    }

    /** A result of a `@RutisClient` interface type `type`. */
    fun <I : Any> clientResult(value: Any?, type: Class<I>, nullable: Boolean): I? = when (value) {
        null, Undefined, kotlinx.serialization.json.JsonNull -> if (nullable) null else throw ValueException("expected ${Refusals.article(type.simpleName)}, got null")
        else -> client(value, type) ?: throw ValueException("expected ${Refusals.article(type.simpleName)}, got ${describe(value)}")
    }

    private fun <I : Any> client(value: Any, type: Class<I>): I? = when (value) {
        // One object, one proxy of each interface.
        is RemoteObject -> type.cast(
            value.proxies[type] ?: Tables.clientFactory(type)?.create(ObjectHandle(value))?.let { value.proxies.putIfAbsent(type, it) ?: it },
        )
        is ExportedObject -> type.cast(value.original.takeIf { type.isInstance(it) })
        else -> null
    }

    /** A `@RutisClient` interface value as a session value: a proxy of another process's object is that object. */
    fun clientValue(value: Any?): Any? = Native.fromNative(value)

    /** A plugin definition as a session value: it crosses as a `plugin` reference. */
    fun plugin(value: Plugin?): Any? = value

    /** The chain of the code running now, for proxies that block. */
    fun chain(): List<String> = CallChain.current()
}
