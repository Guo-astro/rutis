//! The Kotlin SDK's programs for the conformance tests (`kotlin` in `RUTIS_TEST_RUNTIMES`):
//! one jar (`kotlin/rutis/rutis-conformance`, Gradle task `fatJar`) holding
//! the session fixture and the runtime that serves the test plugins through
//! `Rutis.serve`, started by `java` (design K-P1: the generic launcher, no
//! Kotlin runtime in rutis-host). `RUTIS_KOTLIN_JAR` names a jar built
//! beforehand (CI builds it once); otherwise it is built here, once per test
//! binary, with the Gradle wrapper.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The conformance jar.
pub fn jar() -> &'static Path {
    static JAR: OnceLock<PathBuf> = OnceLock::new();
    JAR.get_or_init(|| {
        if let Some(jar) = std::env::var_os("RUTIS_KOTLIN_JAR") {
            return PathBuf::from(jar);
        }
        let project = repo().join("kotlin/rutis");
        let wrapper = if cfg!(windows) {
            project.join("gradlew.bat")
        } else {
            project.join("gradlew")
        };
        let status = std::process::Command::new(wrapper)
            .args(["--quiet", ":rutis-conformance:fatJar"])
            .current_dir(&project)
            .status()
            .expect("the Gradle wrapper runs (a JDK 17 on PATH or JAVA_HOME)");
        assert!(status.success(), "gradlew :rutis-conformance:fatJar");
        project.join("rutis-conformance/build/libs/rutis-conformance-all.jar")
    })
}

/// The JVM options of every Kotlin program: they start fast and stay small.
pub const JVM_OPTIONS: [&str; 2] = ["-XX:TieredStopAtLevel=1", "-XX:+UseSerialGC"];

/// The session fixture (`arcships.rutis.conformance.SessionKt`): endpoint
/// `kotlin`, serving `conformance`; the channel is its last argument.
pub fn session() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("java");
    command
        .args(JVM_OPTIONS)
        .arg("-cp")
        .arg(jar())
        .arg("arcships.rutis.conformance.SessionKt");
    command
}

/// The runtime serving the test plugins (`Rutis.serve`), as the generic
/// launcher starts it: `java -jar <jar> <channel> <project>`.
pub fn launcher() -> rutis_bridge::runtime::Launcher {
    let mut launcher = rutis_bridge::runtime::Launcher::new("java");
    for option in JVM_OPTIONS {
        launcher = launcher.arg(option);
    }
    launcher.arg("-jar").arg(jar())
}
