// The programs the Rust conformance tests start (design §九), on the jvm
// target: the session fixture (`arcships.rutis.conformance.SessionKt`), in
// one jar with every dependency, `build/libs/rutis-conformance-all.jar`:
// `java -cp <jar> arcships.rutis.conformance.SessionKt <channel>`.
plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
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
    implementation(project(":rutis"))
    // The session fixture serves the session layer directly: its data is JSON.
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")
    ksp(project(":rutis-ksp"))
}

val fatJar by tasks.registering(Jar::class) {
    archiveClassifier.set("all")
    archiveVersion.set("")
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
    manifest { attributes["Main-Class"] = "arcships.rutis.conformance.SessionKt" }
    from(sourceSets.main.get().output)
    dependsOn(configurations.runtimeClasspath)
    from({ configurations.runtimeClasspath.get().filter { it.name.endsWith(".jar") }.map { zipTree(it) } })
    exclude("META-INF/*.SF", "META-INF/*.DSA", "META-INF/*.RSA", "META-INF/versions/9/module-info.class")
}

tasks.named("assemble") { dependsOn(fatJar) }
