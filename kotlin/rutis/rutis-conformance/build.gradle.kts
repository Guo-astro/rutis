// The programs the Rust conformance tests start (design §九), on the jvm
// target, in one jar with every dependency, build/libs/rutis-conformance-all.jar:
// the runtime serving the test plugins through Rutis.serve (`java -jar <jar>
// <channel> <project>`) and the session fixture (`java -cp <jar>
// arcships.rutis.conformance.SessionKt <channel>`). Its tests check the
// testing tool against the expectations the real runtimes meet.
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
    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher:1.11.4")
    kspTest(project(":rutis-ksp"))
}

// The testing tool against the expectations the real runtimes are checked
// against (crates/rutis-bridge/src/runtime/testing/*.expected.json).
tasks.test {
    useJUnitPlatform()
    systemProperty("rutis.repo", rootProject.projectDir.resolve("../..").canonicalPath)
}

val fatJar by tasks.registering(Jar::class) {
    archiveClassifier.set("all")
    archiveVersion.set("")
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
    manifest { attributes["Main-Class"] = "arcships.rutis.conformance.MainKt" }
    from(sourceSets.main.get().output)
    dependsOn(configurations.runtimeClasspath)
    from({ configurations.runtimeClasspath.get().filter { it.name.endsWith(".jar") }.map { zipTree(it) } })
    exclude("META-INF/*.SF", "META-INF/*.DSA", "META-INF/*.RSA", "META-INF/versions/9/module-info.class")
}

tasks.named("assemble") { dependsOn(fatJar) }
