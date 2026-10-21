// The KSP processor (compile time only, never in an application): method
// tables for @RutisService classes, proxies for @RutisClient interfaces.
plugins {
    kotlin("jvm")
    id("com.vanniktech.maven.publish")
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation("com.google.devtools.ksp:symbol-processing-api:2.2.21-2.0.4")
}

// Maven Central, as rutis/build.gradle.kts says.
mavenPublishing {
    publishToMavenCentral(automaticRelease = true)
    if (providers.gradleProperty("signingInMemoryKey").isPresent) signAllPublications()
}
