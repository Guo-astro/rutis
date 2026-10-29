// The testing tool (io.github.arcships:rutis-testing, jvm): a plugin's tests
// run it in its real runtime, in the test process, as a row of
// `rutis-host test` (docs/design-platform-cleanup-2026-10-28.md §7.4, §7.5).
// It uses the SDK's public API only.
plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
    id("com.vanniktech.maven.publish")
    id("org.jetbrains.kotlinx.binary-compatibility-validator")
}

kotlin {
    jvmToolchain(17)
    compilerOptions {
        optIn.addAll(
            "kotlin.time.ExperimentalTime",
            "kotlin.uuid.ExperimentalUuidApi",
            "kotlinx.serialization.ExperimentalSerializationApi",
        )
    }
}

dependencies {
    api(project(":rutis"))
    // The proxy of rutis-host/test.
    ksp(project(":rutis-ksp"))
    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher:1.11.4")
    kspTest(project(":rutis-ksp"))
}

// rutis-host test: RUTIS_HOST, else the repository's debug build (`cargo
// build -p rutis-host`), else rutis-host on PATH.
fun rutisHost(): String? = System.getenv("RUTIS_HOST")?.takeIf { it.isNotEmpty() }
    ?: rootProject.projectDir.resolve("../../target/debug/rutis-host").takeIf { it.isFile }?.canonicalPath

tasks.test {
    useJUnitPlatform()
    rutisHost()?.let { environment("RUTIS_HOST", it) }
}

// Maven Central (release.yml `maven`), with the SDK.
mavenPublishing {
    publishToMavenCentral(automaticRelease = true)
    if (providers.gradleProperty("signingInMemoryKey").isPresent) signAllPublications()
}
