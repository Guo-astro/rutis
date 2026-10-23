//! Swift (macOS): `runtimes.swift`, compiled binaries built with the Swift
//! SDK, one runtime each (`swift-<binary>`), started when a row uses it (or
//! with the host); rows `swift:<plugin>` and `swift-<binary>:<plugin>`.
//! The same settings and machinery as Go's ([`super::binary`]); a project
//! is a SwiftPM package (Package.swift) that `dev` builds with `swift
//! build`.

use std::path::{Path, PathBuf};

use rutis::BoxFuture;
use serde_json::Value;

use super::binary::{self, DevBuild};
use super::go::{dev_rows, rebase};
use super::{Language, Mount};
use crate::config::{HostConfig, Restart, Runtimes};
use crate::host::Error;

pub struct Swift;

impl Language for Swift {
    fn key(&self) -> &'static str {
        "swift"
    }

    fn configured(&self, runtimes: &Runtimes) -> bool {
        runtimes.swift.is_some()
    }

    fn settings(&self, runtimes: &Runtimes) -> Option<String> {
        runtimes.swift.as_ref().map(|swift| format!("{swift:?}"))
    }

    /// Each Swift runtime on its own, as Go's.
    fn restart(&self, runtimes: &Runtimes) -> Restart {
        runtimes
            .swift
            .as_ref()
            .map_or(Restart::No, |swift| swift.restart)
    }

    /// The dev file's `restart`; the project's other settings stay.
    fn dev_file(&self, own: &mut Runtimes, dev: &Runtimes) {
        if let (Some(own), Some(dev)) = (&mut own.swift, &dev.swift) {
            own.restart = dev.restart;
        }
    }

    /// As Go's: started as rows use them, from the loader's resolver.
    fn stops_when_removed(&self) -> bool {
        false
    }

    fn rebase(&self, runtimes: &mut Runtimes, at: &dyn Fn(&mut PathBuf)) {
        if let Some(swift) = &mut runtimes.swift {
            rebase(swift, at);
        }
    }

    fn merge(&self, runtimes: &mut Runtimes, other: &mut Runtimes) {
        if runtimes.swift.is_none() {
            runtimes.swift = other.swift.take();
        }
    }

    /// `swift:<plugin>` and `swift-<binary>:<plugin>` are the Swift
    /// runtimes' rows.
    fn claims(&self, name: &str) -> Option<String> {
        binary::claims("swift", "Swift", name)
    }

    /// A Swift runtime on another machine, or in an app, that dials in or
    /// listens (`runtimes.remote[].language`).
    fn remote_names(&self) -> &'static [&'static str] {
        &["swift"]
    }

    fn start(&self, config: &HostConfig, mount: &mut Mount<'_>) -> Result<(), Error> {
        match &config.runtimes.swift {
            Some(swift) => binary::start::<rutis_loader::Swift>(self, swift, config, mount),
            None => Ok(()),
        }
    }

    fn new_names(&self) -> &'static [&'static str] {
        &["swift"]
    }

    fn template(&self) -> &'static [(&'static str, &'static str)] {
        TEMPLATE
    }

    fn next_steps(&self) -> &'static str {
        "swift test && rutis-host dev"
    }

    fn project_file(&self) -> &'static str {
        "Package.swift"
    }

    fn is_project(&self, dir: &Path) -> bool {
        dir.join("Package.swift").is_file()
    }

    /// Why its binaries cannot run here: their SDK is macOS's.
    fn available(&self) -> Result<(), String> {
        if cfg!(target_os = "macos") {
            Ok(())
        } else {
            Err("runtimes.swift needs macOS: Swift plugin binaries run on macOS only".into())
        }
    }

    /// Build it (unless `built`, the first build the caller made without
    /// blocking), then one row per plugin of its binary, named with its
    /// runtime (`swift-<product>:<plugin>`).
    fn dev(&self, dir: &Path, built: Option<&Path>) -> Result<(Vec<Value>, Runtimes), String> {
        let swift =
            SwiftDev::of(dir).ok_or_else(|| format!("{}: no Package.swift", dir.display()))?;
        let binary = match built {
            Some(binary) => binary.to_owned(),
            None => binary::building(&swift.product, || swift.build(1))?,
        };
        let (rows, settings) = dev_rows(&binary, &swift.runtime, &swift.dir, &swift.product)?;
        Ok((
            rows,
            Runtimes {
                swift: Some(settings),
                ..Runtimes::default()
            },
        ))
    }

    fn dev_build(&self, dir: &Path) -> Option<Box<dyn DevBuild>> {
        SwiftDev::of(dir).map(|swift| Box::new(swift) as Box<dyn DevBuild>)
    }
}

/// A Swift plugin project as `rutis-host dev` builds it: a SwiftPM package
/// and its executable product, the binary a host runs. Each build is
/// copied to `.rutis/swift/<product>-<n>`, a new file: a binary rewritten
/// in place while it runs would no longer match its code signature.
#[derive(Debug, Clone)]
pub struct SwiftDev {
    pub dir: PathBuf,
    /// The executable product: the target `Sources/<name>/main.swift`
    /// is, `<name>` the package's name or else the first such target.
    pub product: String,
    pub runtime: String,
}

impl SwiftDev {
    /// The Swift project in `dir`, if it is one (a Package.swift).
    pub fn of(dir: &Path) -> Option<Self> {
        let manifest = std::fs::read_to_string(dir.join("Package.swift")).ok()?;
        let name = package_name(&manifest);
        let sources = dir.join("Sources");
        let product = name
            .filter(|name| sources.join(name).join("main.swift").is_file())
            .or_else(|| {
                let mut targets: Vec<String> = std::fs::read_dir(&sources)
                    .ok()?
                    .flatten()
                    .filter(|entry| entry.path().join("main.swift").is_file())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect();
                targets.sort();
                targets.into_iter().next()
            })
            .unwrap_or_else(|| {
                dir.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "plugin".into())
            });
        let runtime = rutis_bridge::runtime::swift_runtime_name(Path::new(&product));
        Some(Self {
            dir: dir.to_owned(),
            product,
            runtime,
        })
    }

    /// Build #`n`: its binary, or the compiler's errors.
    pub fn build(&self, n: u32) -> Result<PathBuf, String> {
        built(self.build_command().output())?;
        let bin = bin_path(self.bin_path_command().output())?;
        self.copied(&bin, n)
    }

    /// [`SwiftDev::build`] with these commands, without blocking: `swift
    /// build`, then `swift build --show-bin-path`, each killed when
    /// dropped.
    async fn rebuilt(
        &self,
        build: std::process::Command,
        show_bin_path: std::process::Command,
        n: u32,
    ) -> Result<PathBuf, String> {
        built(binary::run(build).await)?;
        let bin = bin_path(binary::run(show_bin_path).await)?;
        self.copied(&bin, n)
    }

    /// `swift build` of the product, in the debug configuration: what the
    /// development loop rebuilds should build fast.
    fn build_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new("swift");
        command
            .args(["build", "--product", &self.product])
            .current_dir(&self.dir);
        command
    }

    /// What says where `swift build` puts the products.
    fn bin_path_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new("swift");
        command
            .args(["build", "--show-bin-path"])
            .current_dir(&self.dir);
        command
    }

    /// Build #`n` at its own path, copied from the products in `bin`.
    fn copied(&self, bin: &Path, n: u32) -> Result<PathBuf, String> {
        let product = bin.join(&self.product);
        let binary = self.binary(n);
        if let Some(builds) = binary.parent() {
            std::fs::create_dir_all(builds).map_err(|error| error.to_string())?;
        }
        let _ = std::fs::remove_file(&binary);
        std::fs::copy(&product, &binary)
            .map_err(|error| format!("{}: {error}", product.display()))?;
        Ok(binary)
    }
}

/// Whether `swift build` (its output) succeeded: if not, the compiler's
/// errors.
fn built(output: std::io::Result<std::process::Output>) -> Result<(), String> {
    let output = output
        .map_err(|error| format!("cannot run swift (is a Swift toolchain on PATH?): {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    // The compiler's errors are on stdout, the build system's on stderr.
    let mut errors = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        errors = format!("{errors}\n{}", stderr.trim()).trim().to_owned();
    }
    Err(errors)
}

/// Where `swift build` puts the products, from what `--show-bin-path`
/// printed.
fn bin_path(output: std::io::Result<std::process::Output>) -> Result<PathBuf, String> {
    let output = output.map_err(|error| format!("cannot run swift: {error}"))?;
    match output.status.success() {
        true => Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        )),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

impl DevBuild for SwiftDev {
    fn name(&self) -> &str {
        &self.product
    }

    fn runtime(&self) -> &str {
        &self.runtime
    }

    /// [`SwiftDev::build`], without blocking (`swift build`, then where
    /// its products are); dropped, it kills the command running.
    fn rebuild(&self, n: u32) -> BoxFuture<'_, Result<PathBuf, String>> {
        Box::pin(self.rebuilt(self.build_command(), self.bin_path_command(), n))
    }

    fn binary(&self, n: u32) -> PathBuf {
        self.dir
            .join(".rutis")
            .join("swift")
            .join(format!("{}-{n}", self.product))
    }
}

/// The `name:` of the `Package(...)` in a Package.swift.
fn package_name(manifest: &str) -> Option<String> {
    let rest = &manifest[manifest.find("Package(")? + "Package(".len()..];
    let rest = rest.trim_start().strip_prefix("name:")?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}

const TEMPLATE: &[(&str, &str)] = &[
    (
        "Package.swift",
        include_str!("../../templates/swift/Package.swift"),
    ),
    (
        "Info.plist",
        include_str!("../../templates/swift/Info.plist"),
    ),
    (
        "Sources/__Type__Plugin/Plugin.swift",
        include_str!("../../templates/swift/Sources/__Type__Plugin/Plugin.swift"),
    ),
    (
        "Sources/__name__/main.swift",
        include_str!("../../templates/swift/Sources/__name__/main.swift"),
    ),
    (
        "Tests/__Type__PluginTests/PluginTests.swift",
        include_str!("../../templates/swift/Tests/__Type__PluginTests/PluginTests.swift"),
    ),
    (
        "rutis.dev.json",
        include_str!("../../templates/swift/rutis.dev.json"),
    ),
    ("README.md", include_str!("../../templates/swift/README.md")),
    (
        ".gitignore",
        include_str!("../../templates/swift/gitignore"),
    ),
    (
        ".github/workflows/release.yml",
        include_str!("../../templates/swift/.github/workflows/release.yml"),
    ),
];

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_package_is_named_and_its_product_found() {
        assert_eq!(
            package_name("let package = Package(\n    name: \"net-probe\",\n").as_deref(),
            Some("net-probe")
        );
        assert_eq!(package_name("let package = Package(platforms: [])"), None);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Package.swift"),
            "let package = Package(name: \"kits\")",
        )
        .unwrap();
        // No target of the package's name: the first with a main.swift.
        for target in ["Zeta", "Alpha", "Lib"] {
            std::fs::create_dir_all(dir.path().join("Sources").join(target)).unwrap();
        }
        std::fs::write(dir.path().join("Sources/Zeta/main.swift"), "").unwrap();
        std::fs::write(dir.path().join("Sources/Alpha/main.swift"), "").unwrap();
        let swift = SwiftDev::of(dir.path()).unwrap();
        assert_eq!(
            (swift.product.as_str(), swift.runtime.as_str()),
            ("Alpha", "swift-alpha")
        );
        std::fs::create_dir_all(dir.path().join("Sources/kits")).unwrap();
        std::fs::write(dir.path().join("Sources/kits/main.swift"), "").unwrap();
        assert_eq!(SwiftDev::of(dir.path()).unwrap().product, "kits");
        assert!(swift.binary(3).ends_with(".rutis/swift/Alpha-3"));
    }

    fn sh(script: &str) -> std::process::Command {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", script]);
        command
    }

    /// A rebuild asks where the products are without blocking: on a
    /// single thread, a slow `--show-bin-path` still lets other work run
    /// (Ctrl-C), and dropping the rebuild ends it. Once it answers, the
    /// product is copied to the build's own path.
    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn a_rebuild_finds_its_products_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Package.swift"),
            "let package = Package(name: \"probe\")",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("Sources/probe")).unwrap();
        std::fs::write(dir.path().join("Sources/probe/main.swift"), "").unwrap();
        let swift = SwiftDev::of(dir.path()).unwrap();

        let started = std::time::Instant::now();
        let slow = swift.rebuilt(sh("true"), sh("sleep 30"), 1);
        let ended = tokio::time::timeout(Duration::from_millis(300), slow).await;
        assert!(ended.is_err());
        assert!(started.elapsed() < Duration::from_secs(10));

        let bin = dir.path().join("products");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("probe"), "built").unwrap();
        let printed = format!("echo {}", bin.display());
        let binary = swift.rebuilt(sh("true"), sh(&printed), 2).await.unwrap();
        assert_eq!(binary, swift.binary(2));
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "built");

        let failed = swift.rebuilt(sh("echo 'error: nope'; exit 1"), sh("true"), 3);
        assert_eq!(failed.await.unwrap_err(), "error: nope");
    }
}
