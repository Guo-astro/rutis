import java.util.Properties

// The SDK: Kotlin Multiplatform with two targets sharing one source tree
// (src/shared, pure Kotlin on the JDK APIs ART has; src/jvmMain adds the
// jvm-only local channels and the test entry Rutis.serve): jvm (JDK 17; the desktop test entry Rutis.serve and the
// conformance tests) and android (API 29+). The android target is configured
// only where an Android SDK is installed (ANDROID_HOME, ANDROID_SDK_ROOT, or
// sdk.dir in local.properties), so that the jvm target builds and tests
// without one.
val androidSdk: Boolean = run {
    val local = rootProject.file("local.properties")
    val fromFile = if (local.exists()) {
        Properties().apply { local.inputStream().use { load(it) } }.getProperty("sdk.dir")
    } else {
        null
    }
    listOfNotNull(fromFile, System.getenv("ANDROID_HOME"), System.getenv("ANDROID_SDK_ROOT"))
        .any { file(it).resolve("platforms").isDirectory }
}

plugins {
    kotlin("multiplatform")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
    id("com.android.library") apply false
}

if (androidSdk) {
    apply(plugin = "com.android.library")
}

kotlin {
    jvmToolchain(17)
    jvm {
        testRuns["test"].executionTask.configure {
            useJUnitPlatform()
            // Tests that start sessions in this process: a hung one fails
            // instead of holding the build.
            systemProperty("junit.jupiter.execution.timeout.default", "120s")
        }
    }
    if (androidSdk) {
        androidTarget {
            publishLibraryVariants("release")
        }
    }
    compilerOptions {
        freeCompilerArgs.add("-Xexpect-actual-classes")
        optIn.addAll(
            "kotlin.time.ExperimentalTime",
            "kotlin.uuid.ExperimentalUuidApi",
            "kotlinx.serialization.ExperimentalSerializationApi",
        )
    }
    sourceSets {
        val jvmMain by getting {
            kotlin.srcDir("src/shared/kotlin")
            dependencies {
                api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
                api("org.jetbrains.kotlinx:kotlinx-serialization-core:1.9.0")
                implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")
                // The WebSocket client of `dial:` (design K8).
                implementation("com.squareup.okhttp3:okhttp:5.3.2")
            }
        }
        val jvmTest by getting {
            dependencies {
                implementation(kotlin("test"))
                implementation("org.junit.jupiter:junit-jupiter:5.11.4")
                runtimeOnly("org.junit.platform:junit-platform-launcher:1.11.4")
            }
        }
        if (androidSdk) {
            val androidMain by getting {
                // The sources shared with the jvm target.
                kotlin.srcDir("src/shared/kotlin")
                dependencies {
                    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
                    api("org.jetbrains.kotlinx:kotlinx-serialization-core:1.9.0")
                    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.9.0")
                // The WebSocket client of `dial:` (design K8).
                implementation("com.squareup.okhttp3:okhttp:5.3.2")
                }
            }
        }
    }
}

// The jvm tests define plugins: their method tables and proxies come from
// the processor, as in an application.
dependencies {
    add("kspJvmTest", project(":rutis-ksp"))
}

if (androidSdk) {
    extensions.configure<com.android.build.gradle.LibraryExtension>("android") {
        namespace = "arcships.rutis"
        compileSdk = 34
        defaultConfig {
            minSdk = 29
            // Keeps the generated tables, proxies and what errors cross with
            // in applications that shrink (R8).
            consumerProguardFiles("consumer-rules.pro")
        }
        compileOptions {
            sourceCompatibility = JavaVersion.VERSION_17
            targetCompatibility = JavaVersion.VERSION_17
        }
    }
}
