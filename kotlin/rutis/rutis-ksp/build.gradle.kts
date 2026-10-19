// The KSP processor (compile time only, never in an application): method
// tables for @RutisService classes, proxies for @RutisClient interfaces.
plugins {
    kotlin("jvm")
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation("com.google.devtools.ksp:symbol-processing-api:2.2.21-2.0.4")
}
