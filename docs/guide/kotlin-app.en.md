# Hosting plugins in an Android application (Kotlin)

[中文](kotlin-app.md)

An Android application provides its own capabilities and those of its device (contacts, notifications, what the device is…) as rutis plugins: the plugins are written in Kotlin and compiled into the application; once connected to rutis, the application installs them itself, in order, each with the configuration the application gives, and the services they provide appear on rutis as `<id>:<plugin>`. Design: [Android app hosting](../design-android-runtime-2026-10-12.en.md) and [app integration](../design-app-integration-2026-10-14.en.md).

`kotlin/rutis/android-sample` is a complete sample application (three plugins, `contacts`, `notify` and `device`, `notify`'s configuration given in `RutisApp`, and a screen to start and stop): the code below comes from it.

## 1. Dependencies

Android 10 (API 29) or later, JDK 17. The Kotlin SDK is not published to Maven; it is used from source: fetch this repository's tag `v0.19.0` next to the application's project, and copy the application's `local.properties` (where the Android SDK is) into it:

```bash
git clone --depth 1 --branch v0.19.0 https://github.com/arcships/rutis.git ../rutis
cp local.properties ../rutis/kotlin/rutis/
```

`kotlin/rutis` builds its Android variant only when it finds the Android SDK itself (`ANDROID_HOME`, `ANDROID_SDK_ROOT` or `sdk.dir` in its `local.properties`); when it does not, the application gets the desktop JVM variant, without the R8 rules.

The application's `settings.gradle.kts` includes it as a Gradle composite build (the path is relative to the application project's root):

```kotlin
includeBuild("../rutis/kotlin/rutis")
```

The application's `build.gradle.kts`:

```kotlin
plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}

dependencies {
    // No version: Gradle builds them from the source includeBuild brings in.
    implementation("io.github.arcships:rutis")
    ksp("io.github.arcships:rutis-ksp")
    // The testing tool, for the plugins' unit tests on the JVM (§7).
    testImplementation("io.github.arcships:rutis-testing")
    kspTest("io.github.arcships:rutis-ksp")
}
```

To upgrade, check out the new tag in `../rutis` (`git fetch --depth 1 origin tag v<version> && git checkout v<version>`).

The KSP processor generates the method tables and proxies at compile time; nothing is found by method reflection at run time, and the AAR ships its R8 rules: a release build with `isMinifyEnabled` needs no rules for rutis. `AndroidManifest.xml` needs `android.permission.INTERNET`.

## 2. Writing a plugin

A plugin is plain Kotlin: a `@RutisService` class and a `Rutis.define`. The whole API is in [kotlin/README](../../kotlin/README.md) and [the plugin API](plugin-api.en.md).

```kotlin
@Serializable
data class DeviceInfo(val manufacturer: String, val model: String, val release: String, val sdk: Int)

@RutisService
class Device(private val context: Context) {
    fun info(): DeviceInfo = DeviceInfo(Build.MANUFACTURER, Build.MODEL, Build.VERSION.RELEASE, Build.VERSION.SDK_INT)
}

fun device(context: Context) = Rutis.define<Rutis.NoConfig>(name = "device") { ctx, _ ->
    ctx.provide("device", Device(context.applicationContext))
}
```

- A plugin that needs a `Context` is a function taking it, as above, given the `applicationContext`.
- `provides` is not needed: the services a plugin provides reach rutis by themselves, and other rows on rutis use them by name.
- A call that waits for rutis (a non-`suspend` method of a `@RutisClient` interface) cannot be made on the main thread: it throws `IllegalStateException`. Call the `suspend` method from a coroutine, or call from another thread. What rutis calls in the application runs on background threads already.
- A plugin is a value too: the result of `Rutis.define(...)` (a `Plugin`) can be handed to a service as an ordinary argument, and the receiver mounts it elsewhere, one copy per mount. In a `@RutisClient` interface or a `@RutisService` class, the parameter or result type is `Plugin`:

  ```kotlin
  @RutisClient
  interface Parts {
      suspend fun register(scope: String, part: Plugin)
  }

  val git = Rutis.define<Rutis.NoConfig>(name = "git", inject = listOf("parts")) { ctx, _ ->
      val pool = openPool()
      val status = Rutis.define<StatusConfig>(name = "git_status", inject = listOf("history")) { ctx, config ->
          ctx.provide("gitStatus", GitStatus(pool, config.depth))
      }
      ctx.use<Parts>("parts").register("session", status)
  }
  ```

  Each copy runs in the application's process. It belongs to this generation of the row that handed it over: when that row unloads, or the application stops or disconnects, its copies end first. Handed to a test double of `RutisTest` (a plugin of the same runtime), the double receives the `Plugin` itself: `RutisTest.load(it, config)` loads it, and handed back by the double, the plugin gets the original one. The parameter or result must be a `Plugin` (or `Plugin?`) itself: plugin definitions inside a collection or a data class, such as `List<Plugin>`, are not supported; make one call per definition. The rules: [the plugin API](plugin-api.en.md#plugin-definitions).

- The config: its JSON Schema is derived from the `@Serializable` type (or given as `schema`); the application gives the config in `RutisApp` (§3). The SDK only decodes it: a key left out takes the constructor's default argument (a nullable field without one is null), and no Schema is checked; a required key missing, or a value the type cannot take, fails the plugin's load with the key named as rutis words it: `config: city is required`, `config: days should be an integer, got "x"` ([plugin API: configuration](plugin-api.en.md#configuration)). The rows rutis loads (a plugin in a host's `rutis.json`, a test's rows) are checked against the Schema and filled with its defaults by rutis before the runtime decodes them; a decoder error after that is `<path> is not a value the plugin accepts`.

### Objects with methods

A `@Serializable` class is data and is copied; an instance of a `@RutisService` class goes to another language by reference: the far end gets a proxy whose calls reach the original object here. A class with neither annotation cannot cross: `a Cache cannot cross: annotate it @RutisService to pass it by reference, or @Serializable to copy it`. For an object another process passes, type the parameter with a `@RutisClient` interface, or as `Any?`:

```kotlin
/** How to use an object another process passes. */
@RutisClient
interface Cache {
    suspend fun get(key: String): Long?
    suspend fun set(key: String, value: Long)
}

@RutisService
class Warmer {
    suspend fun warm(cache: Cache): Long? {
        cache.set("a", 1)                    // a call of the far end's object
        return cache.get("a")
    }

    /** When you do not know what arrives, take Any?: an object is a RemoteObject, called by name. */
    suspend fun peek(value: Any?): Any? = (value as? RemoteObject)?.call("get", "a")
}
```

- A parameter typed with a `@RutisClient` interface gets the KSP-generated proxy, the same kind as `ctx.use<Cache>("cache")`: `suspend` methods suspend, plain methods block (and cannot be called on the main thread). So does the result of a `@RutisClient` method.
- A parameter typed `Any?` gets an `arcships.rutis.advanced.RemoteObject`: `suspend fun call(method, vararg args)` suspends, `fun callBlocking(method, vararg args)` blocks, and arguments and results are ordinary Kotlin values; `release()` gives the reference back now instead of when it is collected. Objects inside lists and maps, and in event arguments, arrive through `Any?` as `RemoteObject`; interface types inside collections, such as `List<Cache>`, are not supported.
- A parameter of a data type (a `@Serializable` class, `String` and so on) fails the call with `bad-argument`: `argument 1 should be a Point, got an object (an object is received only as a @RutisClient interface or Any?)`.
- A proxy has no data fields: to get data, have a method return it. Calling a method the object lacks fails that call with `no-method`: `Cache has no method gett (methods: get, set, size)`.
- One object passed several times is one proxy here; a received proxy or `RemoteObject` passed on sends the original object, and passed back to its process it is the original there.
- An object does not follow its row: after the row that passed it unloads, it can still be called. Objects the application hands over live in its process: after the application calls `stop()` or disconnects, calls on them elsewhere fail with `peer-gone`; once the far end's process has exited, calls here fail with `peer-gone` too.
- In the same process you get the object itself when its class implements the interface, else a proxy that calls it here; so does a test double of `RutisTest`, a plugin of the same runtime. The rules: [the plugin API](plugin-api.en.md#objects-with-methods).

## 3. Connecting to rutis

```kotlin
@Serializable
data class NotifyConfig(val channel: String = "rutis")

fun notify(context: Context) = Rutis.define<NotifyConfig>(name = "notify") { ctx, config -> /* … */ }

val rutis = RutisApp(
    plugins = listOf(contacts(context), notify(context), device(context)),
    configs = mapOf("notify" to NotifyConfig(channel = "rutis sample")),
    connect = "wss://rutis.example.com:7443/rutis",
    id = "kim-pixel",
    token = token,
)
rutis.start()
```

| Parameter | Meaning |
| --- | --- |
| `plugins` | The plugins installed once connected, in this order |
| `configs` | Optional: the plugins' configurations by plugin name, each a value of its configuration type; a plugin without one gets its configuration Schema's defaults (kotlinx's default arguments). A value not of that plugin's configuration type, or a name not among `plugins`, makes the `RutisApp` constructor throw `IllegalArgumentException`: `rutis: plugin notify: … is not its configuration: …`, `rutis: configs names nope, which is not one of the plugins: contacts, device, notify`. Configurations are given in `RutisApp` only: `Rutis.serve`'s plugins get theirs from rutis's rows |
| `connect` | The address of rutis's access point (recommended: the application is behind NAT, its address changes). A non-loopback address needs `wss://` |
| `listen` | Waits for rutis to dial in instead, such as `wss://0.0.0.0:7443/rutis` (a device with a fixed address); `wss://` needs `certificate` (the PEM certificate chain and private key). One controller at a time; a newer connection takes over |
| `id` | This connection's self-reported id (lower-case letters, digits, `-`): its plugins are registered on rutis as `<id>:<plugin>` (`kim-pixel:contacts`); one per device |
| `token` | The shared token of rutis's side (`RUTIS_TOKEN_<row id>`) |
| `trust` | The PEM of CAs trusted besides the system's, for a self-signed access point |
| `peer` | rutis's `id`, when it should be checked |
| `shutdownTimeout` | How long plugins' cleanups may take when they unload; 10 seconds by default |
| `log` | Where the state lines go; `System.err` by default (logcat's `System.err`) |

- `start()` returns at once; once connected (rutis greeted), the application installs the plugins in the order of `plugins`, by the rule plugins follow on rutis: a plugin is installed once every service it `inject`s is there (provided by another plugin of the application, or by rutis), unloaded when one goes, and installed again when it is back. rutis's shareable services all come over to the application; every service the application's plugins provide is reported to rutis (with the plugin's name), and those rutis gave are not reported back. A plugin that fails to load affects only itself: `System.err` gets a line `rutis: plugin <name>: <error>`, and the ones after it are still installed; the line is on the device only, and on rutis's side the plugin just does not appear. A lost connection is dialed again (0.5 s doubling up to 30 s; every 30 s after refused credentials; it stops when the protocols are incompatible); every connection is a new one, and the plugins are installed again on reconnect.
- `stop()` withdraws the services, runs every plugin's cleanups in reverse, then disconnects; `start()` may be called again.
- `rutis.state` is a `StateFlow<RutisState>`: `Stopped(reason)`, `Connecting(retrying)` (`retrying` is the last failure), `Connected(address)`. Show it; the SDK decides nothing by it.

On rutis's side ([rutis-host](rutis-host.en.md)): one `rutis-host/listen` row waits for applications; neither the devices nor their plugins are listed in rutis.json:

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

`RUTIS_TOKEN_PHONE=<token> rutis-host run`. Every connection let in is a session, and the plugins the application runs are registered as `<the application's id>:<plugin>` (`kim-pixel:contacts`); the services they provide are globally visible, and other rows `inject` them by name; a newer connection with the same id replaces the older one. The output of `run` starts with the row id: `phone: connected kim-pixel`, `phone: disconnected kim-pixel (connection lost)`. When one device's application stops (`stop`, the network gone, its process killed), only its plugins unload.

When two devices provide same-named services, the kernel rule applies: the later device's plugin is refused, and its `System.err` has the reason rutis gives (`rutis: cannot announce contacts: …`). A plugin in the application that cannot be installed (refused, or failing in its own `apply`) is written only in the device's log in 0.19; the host prints nothing. The 0.18 way, "one `py:digest` in an `instanced` group, one per device", is not supported in 0.19; telling devices apart is the business of the plugins on the devices:

- Put it on the device: write digest as a plugin in the application that `inject`s that application's `contacts` (it is installed once `contacts` is).
- Let the plugin on the host tell the devices apart: the plugins on the devices provide their services under names with the device's name (the sample's plugins all take their service names from their configurations: `Samples.configs(suffix = "tablet")` gives `contacts-tablet`), and a plugin on the host listens to the framework event `rutis/service` (`{ name, scope, available, row }`, [the plugin API: framework events](plugin-api.en.md#framework-events-rutis)): when `contacts-<device>` appears (`available: true`), it calls the service `loader` in the event handler, `loader.create({id: "digest-<device>", name: "py:digest-one", config: {device}, inject: ["contacts-<device>"]})`, and `loader.remove("digest-<device>")` when it is withdrawn (`available: false`); not in `apply`. The full code: [Swift apps §4](swift-app.en.md#once-per-device).

During development a certificate is not needed: `adb reverse tcp:7443 tcp:7443`, and the application connects to `ws://127.0.0.1:7443/rutis`. Android forbids cleartext traffic by default: allow it for loopback only in the network security config (the sample's `res/xml/network_security_config.xml`).

## 4. Permissions

The permission prompt belongs to the application: the application asks the user when it suits it (the sample calls `requestPermissions` when Start is pressed), and plugins only check. rutis never prompts.

| Case | What the plugin does | What rutis's side sees |
| --- | --- | --- |
| Needed to load (`contacts`) | Checks in `apply`; without it, throws, naming the permission | The application fails to install this plugin (`rutis: plugin contacts: …`, the permission named in the error), and rutis has no `<id>:contacts`; the other plugins run |
| Needed by a call (`notify` posting) | Checks in the method; without it, throws with a `code` and `data` | The caller gets the error: `code` `permission-denied`, `data.permission` the permission |
| The user revokes a runtime permission in settings | — | The system kills the application's process → the connection drops → its plugins unload; after the application restarts and reconnects, they are installed again |

The sample's error type (plain Kotlin properties, which cross with it):

```kotlin
class PermissionDenied(val permission: String) :
    Exception("$permission is not granted to this application: grant it in the application (its settings, or when it asks)") {
    val code: String = "permission-denied"
    val data: Map<String, String> = mapOf("permission" to permission)
}
```

When the user grants the permission later, connect again (`stop()`, then `start()`): the application installs the plugins that failed before again. The sample does so in `onRequestPermissionsResult`.

## 5. When to start and stop (the business layer)

The SDK only connects; foreground and background, keeping alive and waking up are the application's business. Common ways:

| Need | Way |
| --- | --- |
| Capabilities only while the user has the application open | Follow the screen: `start()` in `onStart`, `stop()` in `onStop` |
| Always online | Hold the `RutisApp` in a foreground service: `start()` when it starts, `stop()` when it stops; its notification tells the user |
| Now and then | `start()` briefly on a push message (FCM), `stop()` when done |

A foreground service (its type must be declared from Android 14):

```xml
<uses-permission android:name="android.permission.FOREGROUND_SERVICE" />
<uses-permission android:name="android.permission.FOREGROUND_SERVICE_DATA_SYNC" />
<service android:name=".RutisService" android:foregroundServiceType="dataSync" android:exported="false" />
```

- Choose the type by what the application really does. From Android 15, `dataSync` runs about 6 hours a day in total; to stay longer without a more fitting type, use `specialUse` with its purpose stated (`<property android:name="android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE" android:value="…"/>`), which Google Play reviews. Plugins using location, the camera or the microphone need the matching `location`, `camera`, `microphone` types and permissions.
- The service calls `startForeground(id, notification, type)` early in `onStartCommand`.

Battery optimization:

- Doze and app standby defer the application's network access: the connection drops and is dialed again by the backoff. To stay connected, ask the user to exempt the application from battery optimization: open `Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS` and let them choose; asking directly with `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` needs the `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` permission, which Google Play allows few categories of applications.
- Some vendors' systems add background restrictions of their own (auto-start, background activity switches): guide the user to allow the application in the system settings.
- Whatever happens, rutis only sees a service there or not: when the connection drops, the application's plugins unload and their users stop; after reconnecting, they come back.

## 6. Threads and lifecycle details

- `start()` and `stop()` may be called on the main thread; they return at once.
- Each call rutis makes into the application runs on a thread of its own; a `suspend` method runs on it, and the call chain travels with the coroutine: plugin code passes no context.
- Data a plugin keeps goes to the platform's own storage (`SharedPreferences`, Room…); the SDK has no storage.

## 7. Testing

- Unit tests of a plugin, on the JVM, use the testing tool `arcships.rutis.testing.RutisTest` (artifact `io.github.arcships:rutis-testing`, see kotlin/README): the plugin runs in its real runtime in the test process, as a row of `rutis-host test` (on `PATH` or at `RUTIS_HOST`), started once per test process. On what it loads, `loaded.on(name) { args -> ... }` listens at the root scope: the plugin's emissions and the test's own reach it, and it returns the function that stops it.
- On a device there is no `rutis-host` process: as the application does, the test installs its own plugins with `RutisApp`, dialing a rutis on the development machine (a `rutis-host/listen` row, reached through `adb reverse`), and its plugins use one another's services and events through it. The repository's device tests (an emulator, run locally: [CI](../ci.en.md) §12): `./gradlew :rutis:connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.address=<ws address> -Pandroid.testInstrumentationRunnerArguments.token=<token>` (the SDK's main-thread rule; skipped without `address`) and `cargo test -p rutis-host --features android --test android` (the sample's release build, through R8, dialing a rutis-host, stopped and started; two connections each installing their plugins; the permission prompt being the application's, a denial failing `contacts` at load and reaching `notify`'s caller as `permission-denied`, everything back once granted). `tools/android-emulator.sh start` boots an emulator as CI does.

## 8. Acceptance on real devices

The flows above are checked automatically on an emulator. Walk through these by hand on real devices (before a release, or with a major new Android version):

1. A computer runs rutis-host configured as in §3, listening on `wss://` with a real certificate (or a self-signed one and the application's `trust`); two phones with the sample application, on the same Wi-Fi or on mobile data.
2. Give each phone its own id (such as `pixel`, `galaxy`) and the token, press Start: each shows this application's ("rutis sample") prompts for contacts and notifications.
3. Allow on one, deny on the other. The first installs `contacts` (`<id>:contacts` appears on rutis), and the rows using it (such as `py:digest`) get this phone's contacts. On the other, the application's logcat has `rutis: plugin contacts: …` with `android.permission.READ_CONTACTS` in the error, rutis has no `contacts` of it, and a row calling `notify.post` gets `permission-denied`.
4. Grant the permission in the second phone's settings, then press Stop and Start in the application (or restart it): its `contacts` row comes back.
5. Press Stop on one phone: only its plugins unload, the other is untouched; Start brings it back.
6. Turn one phone's network off for a minute and on again: its plugins unload, and it reconnects and installs them again by itself.
7. Revoke a granted permission in settings: the system kills the application's process and its plugins unload; opened and started again, it installs them as §4 says.
8. Repeat steps 2 and 3 with the release build (R8): it behaves as the debug build does.

## 9. Known limitations

- New code is not loaded on the device: new plugins come with a new version of the application.
- Identity is interim: a shared token and a self-reported id, which rutis does not verify.
