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

    /// Build it, then one row per plugin of its binary, named with its
    /// runtime (`swift-<product>:<plugin>`).
    fn dev(&self, dir: &Path) -> Result<(Vec<Value>, Runtimes), String> {
        let swift =
            SwiftDev::of(dir).ok_or_else(|| format!("{}: no Package.swift", dir.display()))?;
        let binary = binary::building(&swift.product, || swift.build(1))?;
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
        let built = self.build_command().output();
        self.copied(built, n)
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

    /// Where `swift build` puts the products.
    fn bin_path(&self) -> Result<PathBuf, String> {
        let output = std::process::Command::new("swift")
            .args(["build", "--show-bin-path"])
            .current_dir(&self.dir)
            .output()
            .map_err(|error| format!("cannot run swift: {error}"))?;
        match output.status.success() {
            true => Ok(PathBuf::from(
                String::from_utf8_lossy(&output.stdout).trim(),
            )),
            false => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
        }
    }

    /// Build #`n` at its own path, once `built` (the build's output) says
    /// it succeeded; else the compiler's errors.
    fn copied(
        &self,
        built: std::io::Result<std::process::Output>,
        n: u32,
    ) -> Result<PathBuf, String> {
        let output = built.map_err(|error| {
            format!("cannot run swift (is a Swift toolchain on PATH?): {error}")
        })?;
        if !output.status.success() {
            // The compiler's errors are on stdout, the build system's on
            // stderr.
            let mut errors = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                errors = format!("{errors}\n{}", stderr.trim()).trim().to_owned();
            }
            return Err(errors);
        }
        let product = self.bin_path()?.join(&self.product);
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

impl DevBuild for SwiftDev {
    fn name(&self) -> &str {
        &self.product
    }

    fn runtime(&self) -> &str {
        &self.runtime
    }

    /// [`SwiftDev::build`], without blocking; dropped, it kills `swift
    /// build`.
    fn rebuild(&self, n: u32) -> BoxFuture<'_, Result<PathBuf, String>> {
        Box::pin(async move {
            let built = binary::run(self.build_command()).await;
            self.copied(built, n)
        })
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
}
