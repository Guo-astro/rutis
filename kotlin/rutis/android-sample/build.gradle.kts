// An Android application hosting rutis plugins (design-android-runtime §五,
// K-P3): the sample capability plugins, compiled into the app, and a
// screen to start and stop it (also driven by intent extras). On an
// emulator, against a rutis-host the Rust test runs
// (crates/rutis-host/tests/android.rs): the release build, shrunk and
// obfuscated by R8 as an application ships, started and stopped by intents
// (the method tables and proxies found at run time as they are there); and
// the device tests (src/androidTest, on the debug build) driving several
// connections.
plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}

android {
    namespace = "arcships.rutis.sample"
    compileSdk = 34
    defaultConfig {
        applicationId = "arcships.rutis.sample"
        minSdk = 29
        targetSdk = 34
        versionCode = 1
        versionName = "1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // A sample: signed with the debug key, so that it installs.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    jvmToolchain(17)
    compilerOptions {
        optIn.addAll("kotlinx.serialization.ExperimentalSerializationApi")
    }
}

dependencies {
    implementation(project(":rutis"))
    ksp(project(":rutis-ksp"))
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test:rules:1.6.1")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation(kotlin("test"))
}
