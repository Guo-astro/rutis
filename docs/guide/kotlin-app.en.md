# Hosting plugins in an Android application (Kotlin)

[中文](kotlin-app.md)

An Android application provides its own capabilities and those of its device (contacts, notifications, what the device is…) as rutis plugins: the plugins are written in Kotlin and compiled into the application; once it connects, rutis's configuration decides which plugins load, with what configuration, and when they unload. Design: [Android app hosting](../design-android-runtime-2026-10-12.en.md) and [app integration](../design-app-integration-2026-10-14.en.md).

`kotlin/rutis/android-sample` is a complete sample application (three plugins, `contacts`, `notify` and `device`, and a screen to start and stop): the code below comes from it.

## 1. Dependencies

Android 10 (API 29) or later, JDK 17. The application's `build.gradle.kts`:

```kotlin
plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}

dependencies {
    implementation("io.github.arcships:rutis:0.17.0")
    ksp("io.github.arcships:rutis-ksp:0.17.0")
}
```

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
- `provides` is not needed: the services a plugin provides reach rutis by themselves, and rutis-side rows in the same instance use them as they are.
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

  Each copy runs in the application's process; when the application dials in, it mounts only in that connection's instance (or deeper). It belongs to this generation of the row that handed it over: when that row unloads, or the application stops or disconnects, its copies end first. Handed to a test double of `RutisTest`, the double receives the `Plugin` itself: `RutisTest.load(it, config)` loads it, and handed back by the double, the plugin gets the original one. The parameter or result must be a `Plugin` (or `Plugin?`) itself: plugin definitions inside a collection or a data class, such as `List<Plugin>`, are not supported; make one call per definition. The rules: [the plugin API](plugin-api.en.md#plugin-definitions).

## 3. Connecting to rutis

```kotlin
val rutis = RutisApp(
    plugins = listOf(contacts(context), notify(context), device(context)),
    connect = "wss://rutis.example.com:7443/rutis",
    id = "kim-pixel",
    token = token,
)
rutis.start()
```

| Parameter | Meaning |
| --- | --- |
| `connect` | The address of rutis's access point (recommended: the application is behind NAT, its address changes). A non-loopback address needs `wss://` |
| `listen` | Waits for rutis to dial in instead, such as `wss://0.0.0.0:7443/rutis` (a device with a fixed address); `wss://` needs `certificate` (the PEM certificate chain and private key). One controller at a time; a newer connection takes over |
| `id` | This connection's self-reported id (lower-case letters, digits, `-`), its instance's name in rutis; one per device |
| `token` | The shared token of rutis's side (`RUTIS_TOKEN_<runtime name>`) |
| `trust` | The PEM of CAs trusted besides the system's, for a self-signed access point |
| `peer` | rutis's `id`, when it should be checked |
| `shutdownTimeout` | How long plugins' cleanups may take when they unload; 10 seconds by default |
| `log` | Where the state lines go; `System.err` by default (logcat's `System.err`) |

- `start()` returns at once; once connected, rutis loads the plugins. A lost connection is dialed again (0.5 s doubling up to 30 s; every 30 s after refused credentials; it stops when the protocols are incompatible).
- `stop()` runs every plugin's cleanups in reverse, then disconnects; `start()` may be called again.
- `rutis.state` is a `StateFlow<RutisState>`: `Stopped(reason)`, `Connecting(retrying)` (`retrying` is the last failure), `Connected(address)`. Show it; the SDK decides nothing by it.

On rutis's side ([rutis-host](rutis-host.en.md)): each connection is an instance of an `instanced` group, which lists the plugins to load on the device and the rows using them:

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "remote": [{ "name": "phone", "language": "kotlin", "listen": "public", "instances": "phone" }] },
  "rows": [
    { "id": "phone", "group": true, "instanced": true, "config": [
      { "id": "contacts", "name": "phone:contacts" },
      { "id": "device", "name": "phone:device" },
      { "id": "digest", "name": "py:digest", "inject": ["contacts"] }
    ] }
  ]
}
```

`RUTIS_TOKEN_PHONE=<token> rutis-host run`. Two devices with plugins of the same name are each in their own instance and do not collide; when one device's application stops (`stop`, the network gone, its process killed), only its instance closes.

During development a certificate is not needed: `adb reverse tcp:7443 tcp:7443`, and the application connects to `ws://127.0.0.1:7443/rutis`. Android forbids cleartext traffic by default: allow it for loopback only in the network security config (the sample's `res/xml/network_security_config.xml`).

## 4. Permissions

The permission prompt belongs to the application: the application asks the user when it suits it (the sample calls `requestPermissions` when Start is pressed), and plugins only check. rutis never prompts.

| Case | What the plugin does | What rutis's side sees |
| --- | --- | --- |
| Needed to load (`contacts`) | Checks in `apply`; without it, throws, naming the permission | The row fails to load, the permission named in the error |
| Needed by a call (`notify` posting) | Checks in the method; without it, throws with a `code` and `data` | The caller gets the error: `code` `permission-denied`, `data.permission` the permission |
| The user revokes a runtime permission in settings | — | The system kills the application's process → the connection drops → the instance closes; after the application restarts and reconnects, its rows load again as configured |

The sample's error type (plain Kotlin properties, which cross with it):

```kotlin
class PermissionDenied(val permission: String) :
    Exception("$permission is not granted to this application: grant it in the application (its settings, or when it asks)") {
    val code: String = "permission-denied"
    val data: Map<String, String> = mapOf("permission" to permission)
}
```

When the user grants the permission later, connect again (`stop()`, then `start()`): rutis loads the plugins that failed before. The sample does so in `onRequestPermissionsResult`.

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
- Whatever happens, rutis only sees a service there or not: when the connection drops, the instance closes and its users stop; after reconnecting, they come back.

## 6. Threads and lifecycle details

- `start()` and `stop()` may be called on the main thread; they return at once.
- Each call rutis makes into the application runs on a thread of its own; a `suspend` method runs on it, and the call chain travels with the coroutine: plugin code passes no context.
- Data a plugin keeps goes to the platform's own storage (`SharedPreferences`, Room…); the SDK has no storage.

## 7. Testing

- Unit tests of a plugin use the testing tool `arcships.rutis.testing.RutisTest` (see kotlin/README), on the JVM or a device.
- The repository's device tests (an emulator, run locally: [CI](../ci.en.md) §12): `./gradlew :rutis:connectedDebugAndroidTest` (the SDK's main-thread rule) and `cargo test -p rutis-host --features android --test android` (the sample's release build, through R8, dialing a rutis-host, stopped and started; two connections each an instance; the permission prompt being the application's, a denial failing `contacts` at load and reaching `notify`'s caller as `permission-denied`, everything back once granted). `tools/android-emulator.sh start` boots an emulator as CI does.

## 8. Acceptance on real devices

The flows above are checked automatically on an emulator. Walk through these by hand on real devices (before a release, or with a major new Android version):

1. A computer runs rutis-host configured as in §3, listening on `wss://` with a real certificate (or a self-signed one and the application's `trust`); two phones with the sample application, on the same Wi-Fi or on mobile data.
2. Give each phone its own id (such as `pixel`, `galaxy`) and the token, press Start: each shows this application's ("rutis sample") prompts for contacts and notifications.
3. Allow on one, deny on the other. On the first, the `contacts` row runs in its instance and the rows using it (such as `py:digest`) get this phone's contacts. On the other, rutis-host reports the row failed with `android.permission.READ_CONTACTS` in the error, and a row calling `notify.post` gets `permission-denied`.
4. Grant the permission in the second phone's settings, then press Stop and Start in the application (or restart it): its `contacts` row comes back.
5. Press Stop on one phone: only its instance closes, the other is untouched; Start brings it back.
6. Turn one phone's network off for a minute and on again: its instance closes, and it reconnects and loads its rows again by itself.
7. Revoke a granted permission in settings: the system kills the application's process and its instance closes; opened and started again, it loads as §4 says.
8. Repeat steps 2 and 3 with the release build (R8): it behaves as the debug build does.

## 9. Known limitations

- New code is not loaded on the device: new plugins come with a new version of the application.
- Identity is interim: a shared token and a self-reported id, which rutis does not verify.
