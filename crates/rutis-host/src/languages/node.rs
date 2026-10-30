//! Node: `runtimes.node`, the `@arcships/rutis-runtime` package, rows named
//! as npm packages or `file:` URLs.

use std::path::{Path, PathBuf};

use rutis_bridge::runtime::Launcher;
use serde_json::{json, Value};

use super::{id_of, package_entry, Language};
use crate::config::{file_url, NodeRuntime, Restart, Runtimes};
use crate::connect::SpawnPlugin;
use crate::host::Error;

pub struct Node;

impl Language for Node {
    fn key(&self) -> &'static str {
        "node"
    }

    fn configured(&self, runtimes: &Runtimes) -> bool {
        runtimes.node.is_some()
    }

    fn settings(&self, runtimes: &Runtimes) -> Option<String> {
        runtimes.node.as_ref().map(|node| format!("{node:?}"))
    }

    fn restart(&self, runtimes: &Runtimes) -> Restart {
        runtimes
            .node
            .as_ref()
            .map_or(Restart::No, |node| node.restart)
    }

    /// The dev file's `restart`; the project's other settings stay.
    fn dev_file(&self, own: &mut Runtimes, dev: &Runtimes) {
        if let (Some(own), Some(dev)) = (&mut own.node, &dev.node) {
            own.restart = dev.restart;
        }
    }

    /// A JavaScript or TypeScript file, named by a path or a `file:` URL
    /// (an npm name may be another resolver's: not guessed).
    fn runs_by_default(&self, name: &str) -> bool {
        let file = ["./", "../", "/", "file:"]
            .iter()
            .any(|start| name.starts_with(start));
        let script = [".js", ".mjs", ".cjs", ".ts", ".mts", ".cts"]
            .iter()
            .any(|end| name.ends_with(end));
        file && script
    }

    fn enable_default(&self, runtimes: &mut Runtimes) {
        runtimes.node = Some(NodeRuntime {
            project: PathBuf::from("."),
            runtime: None,
            restart: Restart::No,
        });
    }

    fn rebase(&self, runtimes: &mut Runtimes, at: &dyn Fn(&mut PathBuf)) {
        if let Some(node) = &mut runtimes.node {
            at(&mut node.project);
            if let Some(runtime) = &mut node.runtime {
                at(runtime);
            }
        }
    }

    fn merge(&self, runtimes: &mut Runtimes, other: &mut Runtimes) {
        if runtimes.node.is_none() {
            runtimes.node = other.node.take();
        }
    }

    fn runtime(&self, runtimes: &Runtimes) -> Option<Result<SpawnPlugin, Error>> {
        runtimes
            .node
            .as_ref()
            .map(|node| runtime(node, runtimes.debug))
    }

    fn debugging(&self) -> Option<Result<&'static str, String>> {
        Some(Ok(
            "the Node runtime's inspector listens on 127.0.0.1:9229 (chrome://inspect, or a debugger attaching to port 9229)",
        ))
    }

    fn new_names(&self) -> &'static [&'static str] {
        &["node", "ts", "typescript", "js"]
    }

    fn template(&self) -> &'static [(&'static str, &'static str)] {
        TEMPLATE
    }

    fn next_steps(&self) -> &'static str {
        "npm install && npm test && npx rutis-host dev"
    }

    fn project_file(&self) -> &'static str {
        "package.json"
    }

    /// A package.json project, unless it runs in Bun.
    fn is_project(&self, dir: &Path) -> bool {
        super::is_package_project(dir) && !super::bun::runs_in_bun(dir)
    }

    /// The package's source as the row.
    fn dev(&self, dir: &Path, _built: Option<&Path>) -> Result<(Vec<Value>, Runtimes), String> {
        let (name, entry) = package_entry(dir)?;
        let row = json!({ "id": id_of(&name), "name": file_url(&entry), "config": {} });
        let runtimes = Runtimes {
            node: Some(NodeRuntime {
                restart: Restart::No,
                project: dir.to_owned(),
                runtime: None,
            }),
            ..Runtimes::default()
        };
        Ok((vec![row], runtimes))
    }
}

/// Where the Node runtime's inspector listens under `dev --debug`: Node's
/// own default port, taken by the runtime's process alone (not by an npm
/// launcher that started this host, as `NODE_OPTIONS=--inspect` would).
pub const INSPECT: &str = "127.0.0.1:9229";

/// The Node runtime package (`@arcships/rutis-runtime`) for the project
/// `project`: `configured` (`runtimes.node.runtime`), else the project's
/// own, else the one `RUTIS_NODE_RUNTIME` names; the first that is there.
pub(crate) fn runtime_package(configured: Option<&Path>, project: &Path) -> Option<PathBuf> {
    [
        configured.map(Path::to_path_buf),
        Some(project.join("node_modules/@arcships/rutis-runtime")),
        std::env::var_os("RUTIS_NODE_RUNTIME").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    .find(|package| package.join("package.json").exists())
}

/// The Node runtime of `node.project`, with the `@arcships/rutis-runtime`
/// package installed there (or given, or next to this program); with
/// `debug`, its inspector on [`INSPECT`].
fn runtime(node: &NodeRuntime, debug: bool) -> Result<SpawnPlugin, Error> {
    let anchor = node.project.join("package.json");
    if !anchor.exists() {
        return Err(format!(
            "the Node runtime needs a package.json in {}: run `npm init -y` there",
            node.project.display()
        ));
    }
    let package = runtime_package(node.runtime.as_deref(), &node.project).ok_or_else(|| {
            format!(
                "the Node runtime is not installed in {}: run `npm install @arcships/rutis-runtime` there",
                node.project.display()
            )
        })?;
    let mut launcher = launcher(&package);
    if debug {
        launcher
            .args
            .insert(0, format!("--inspect={INSPECT}").into());
    }
    Ok(SpawnPlugin::new("node", launcher, anchor))
}

const TEMPLATE: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!("../../templates/node/package.json"),
    ),
    (
        "tsconfig.json",
        include_str!("../../templates/node/tsconfig.json"),
    ),
    (
        "src/index.ts",
        include_str!("../../templates/node/src/index.ts"),
    ),
    (
        "test/index.test.ts",
        include_str!("../../templates/node/test/index.test.ts"),
    ),
    (
        "rutis.dev.json",
        include_str!("../../templates/node/rutis.dev.json"),
    ),
    ("README.md", include_str!("../../templates/node/README.md")),
    (".gitignore", include_str!("../../templates/node/gitignore")),
    (
        ".github/workflows/publish.yml",
        include_str!("../../templates/node/.github/workflows/publish.yml"),
    ),
];

/// The Node runtime of the npm package `package` (`node/rutis-runtime`, or
/// an installed `@arcships/rutis-runtime`): `node --import tsx
/// src/runner.mjs` in the package, on an inherited socket (on Windows, a
/// loopback address).
pub fn launcher(package: &Path) -> Launcher {
    Launcher::new("node")
        .arg("--import")
        .arg("tsx")
        .arg(package.join("src/runner.mjs"))
        .cwd(package)
        .inherit_fd()
}
