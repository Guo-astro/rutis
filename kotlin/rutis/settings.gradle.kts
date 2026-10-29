// The Kotlin SDK of rutis (design: docs/design-android-runtime-2026-10-12.md):
// `rutis` (the library, Kotlin Multiplatform: jvm and, where an Android SDK
// is installed, android), `rutis-ksp` (the KSP processor generating method
// tables and proxies), `rutis-testing` (the testing tool, jvm: plugins
// under test as rows of `rutis-host test`), `rutis-conformance` (the programs the Rust
// conformance tests start) and, where an Android SDK is installed,
// `android-sample` (an application hosting the sample plugins, and the
// device tests on an emulator).
// Maven Central and Google first: every plugin used is there (the portal
// only as a fallback), so a build depends on as few hosts as possible.
pluginManagement {
    repositories {
        mavenCentral()
        google()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "rutis-kotlin"

include(":rutis", ":rutis-ksp", ":rutis-testing", ":rutis-conformance")

// The Android SDK: ANDROID_HOME, ANDROID_SDK_ROOT, or sdk.dir in
// local.properties (as rutis/build.gradle.kts finds it).
val androidSdk: Boolean = run {
    val local = file("local.properties")
    val fromFile = if (local.exists()) {
        java.util.Properties().apply { local.inputStream().use { load(it) } }.getProperty("sdk.dir")
    } else {
        null
    }
    listOfNotNull(fromFile, System.getenv("ANDROID_HOME"), System.getenv("ANDROID_SDK_ROOT"))
        .any { file(it).resolve("platforms").isDirectory }
}

if (androidSdk) {
    include(":android-sample")
}
