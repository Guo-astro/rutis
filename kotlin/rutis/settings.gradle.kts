// The Kotlin SDK of rutis (design: docs/design-android-runtime-2026-10-12.md):
// `rutis` (the library, Kotlin Multiplatform: jvm and, where an Android SDK
// is installed, android), `rutis-ksp` (the KSP processor generating method
// tables and proxies) and `rutis-conformance` (the programs the Rust
// conformance tests start).
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

include(":rutis", ":rutis-ksp", ":rutis-conformance")
