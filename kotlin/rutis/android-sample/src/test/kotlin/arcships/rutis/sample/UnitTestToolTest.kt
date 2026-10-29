package arcships.rutis.sample

import arcships.rutis.Rutis
import arcships.rutis.RutisService
import arcships.rutis.testing.RutisTest
import kotlinx.coroutines.runBlocking
import kotlin.test.Test
import kotlin.test.assertEquals

@RutisService
class Greeter {
    fun hello(name: String): String = "Hello, $name!"
}

val greeter = Rutis.define<Rutis.NoConfig>(name = "greeter") { ctx, _ ->
    ctx.provide("greeter", Greeter())
}

/**
 * The testing tool in an application's unit tests: on the JVM, with the
 * Android variant of rutis on the classpath (kotlin-app §7).
 */
class UnitTestToolTest {
    @Test
    fun loadsAndCallsAPlugin() = runBlocking {
        RutisTest.load(greeter).use { loaded ->
            assertEquals("Hello, Ada!", loaded.service("greeter").call("hello", "Ada"))
        }
    }
}
