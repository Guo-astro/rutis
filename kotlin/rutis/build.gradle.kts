plugins {
    kotlin("multiplatform") version "2.2.21" apply false
    kotlin("jvm") version "2.2.21" apply false
    kotlin("plugin.serialization") version "2.2.21" apply false
    id("com.google.devtools.ksp") version "2.2.21-2.0.4" apply false
    id("com.android.library") version "8.11.1" apply false
    id("com.android.application") version "8.11.1" apply false
    kotlin("android") version "2.2.21" apply false
    // The public API of the library, locked by rutis/api/ (#692).
    id("org.jetbrains.kotlinx.binary-compatibility-validator") version "0.18.1" apply false
    // Publishing to Maven Central (the release train, release.yml `maven`).
    id("com.vanniktech.maven.publish") version "0.35.0" apply false
}
