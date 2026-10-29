//! guarantee: host/plugin-install
//! Each kind of reference (§四): the source and language it reads as, and
//! the package manager commands it runs, with recording programs on `PATH`
//! in place of npm, bun, uv and go.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;

/// A directory with recording programs, a log of what they were asked,
/// and the places to install into.
struct Bench {
    dir: tempfile::TempDir,
    _one_at_a_time: std::sync::MutexGuard<'static, ()>,
}

/// One bench at a time: a program just written, run while another test's
/// process is starting (and holds it open for writing), is refused or
/// killed.
static BENCHES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Where [`dispatcher`] is: removed when the tests end.
static DISPATCHER_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

extern "C" fn remove_dispatcher() {
    if let Some(dir) = DISPATCHER_DIR.get() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The one program every recording program links to, written once and run
/// once before any bench uses it; its directory is removed when the tests
/// end.
fn dispatcher() -> &'static Path {
    static DISPATCHER: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DISPATCHER.get_or_init(|| {
        let dir = DISPATCHER_DIR.get_or_init(|| {
            std::env::temp_dir().join(format!("rutis-host-benches-{}", std::process::id()))
        });
        std::fs::create_dir_all(dir).unwrap();
        // SAFETY: registers a function that only removes a directory.
        unsafe { libc::atexit(remove_dispatcher) };
        let path = dir.join("dispatch");
        std::fs::write(
            &path,
            "#!/bin/sh\n\
             name=$(basename \"$0\")\n\
             [ \"$name\" = dispatch ] && exit 0\n\
             bin=$(dirname \"$0\")\n\
             echo \"$name [$(basename \"$PWD\")]${GOBIN:+ GOBIN=$GOBIN} $*\" >> \"$bin/../log\"\n\
             . \"$bin/$name.sh\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Until it runs: only this once can it be busy.
        while std::process::Command::new(&path)
            .current_dir(dir)
            .status()
            .is_err()
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        path
    })
}

impl Bench {
    fn new() -> Self {
        let bench = Self {
            _one_at_a_time: BENCHES
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(bench.bin()).unwrap();
        std::fs::create_dir_all(bench.app()).unwrap();
        bench
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    fn app(&self) -> PathBuf {
        self.dir.path().join("app")
    }

    fn log(&self) -> PathBuf {
        self.dir.path().join("log")
    }

    /// A program `name` that logs `<name> [<cwd>] <args>` and then runs
    /// `body` (a shell script): a link to [`dispatcher`], which sources
    /// `<name>.sh` beside it. A file just written is not run: on Linux it
    /// can be refused (ETXTBSY) while another test's process is starting.
    fn program(&self, name: &str, body: &str) {
        std::fs::write(self.bin().join(format!("{name}.sh")), body).unwrap();
        let link = self.bin().join(name);
        if !link.exists() {
            std::os::unix::fs::symlink(dispatcher(), link).unwrap();
        }
    }

    /// What the programs were asked, one line each; paths under the bench
    /// as `$B`.
    fn asked(&self) -> Vec<String> {
        let root = self.dir.path().display().to_string();
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(|line| line.replace(&root, "$B"))
            .collect()
    }

    fn installer(&self) -> Installer {
        Installer::new(Places {
            base: self.app(),
            node: self.app(),
            node_runtime: None,
            bun: self.app(),
            bun_runtime: None,
            python: self.app().join(".venv"),
            go: Ok(self.app().join("plugins/go")),
            swift: Ok(self.app().join("plugins/swift")),
        })
        .with_path(format!("{}:/usr/bin:/bin", self.bin().display()))
    }

    /// The language and the package installed (steps 1–3).
    fn install(&self, installer: &Installer, text: &str) -> Result<(Lang, Package), Error> {
        let reference = Reference::parse(text, &self.app())?;
        let (lang, _) = installer.core.language(&reference)?;
        let package = installer.core.install(&reference, lang, false)?;
        Ok((lang, package))
    }

    /// `$B` for the bench's directory in `text`.
    fn here(&self, text: &str) -> String {
        text.replace("$B", &self.dir.path().display().to_string())
    }
}

fn names(package: &Package) -> Vec<(&str, &str, &str)> {
    package
        .plugins
        .iter()
        .map(|plugin| {
            (
                plugin.name.as_str(),
                plugin.from.as_str(),
                plugin.language.as_str(),
            )
        })
        .collect()
}

#[test]
fn an_npm_package_without_bun_is_node() {
    let bench = Bench::new();
    bench.program(
        "npm",
        r#"case "$1" in view) echo '{"name":"@ana/wordcount","dependencies":{"@arcships/rutis":"^0.19.0"}}';; esac"#,
    );
    let (lang, package) = bench
        .install(&bench.installer(), "npm:@ana/wordcount@1.2.0")
        .unwrap();
    assert_eq!(lang, Lang::Node);
    assert_eq!(
        names(&package),
        [("@ana/wordcount", "npm:@ana/wordcount", "node")]
    );
    assert_eq!(
        bench.asked(),
        [
            "npm [rutis-host] view @ana/wordcount@1.2.0 --json".to_owned(),
            "npm [app] init -y".into(),
            format!("npm [app] install @ana/wordcount@1.2.0 @arcships/rutis-runtime@{VERSION}"),
        ]
    );
    assert_eq!(
        package.uninstall,
        "npm uninstall --prefix $B/app @ana/wordcount"
            .replace("$B", &bench.dir.path().display().to_string())
    );
}

/// The runtime there and a package.json: only the package is installed.
#[test]
fn a_node_project_with_its_runtime_gets_the_package_only() {
    let bench = Bench::new();
    bench.program("npm", r#"case "$1" in view) echo '{}';; esac"#);
    std::fs::write(bench.app().join("package.json"), "{}").unwrap();
    let runtime = bench.app().join("node_modules/@arcships/rutis-runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(runtime.join("package.json"), "{}").unwrap();
    bench.install(&bench.installer(), "npm:wordcount").unwrap();
    assert_eq!(
        bench.asked(),
        [
            "npm [rutis-host] view wordcount --json",
            "npm [app] install wordcount"
        ]
    );
}

/// The runtime the configuration names (`runtimes.node.runtime`,
/// `runtimes.bun.runtime`) and none in the project: only the package is
/// installed.
#[test]
fn a_configured_runtime_is_not_installed_again() {
    let bench = Bench::new();
    let runtime = bench.dir.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(runtime.join("package.json"), "{}").unwrap();
    std::fs::write(bench.app().join("package.json"), "{}").unwrap();
    let installer = Installer::new(Places {
        node_runtime: Some(runtime.clone()),
        bun_runtime: Some(runtime),
        ..bench.installer().core.places.clone()
    })
    .with_path(format!("{}:/usr/bin:/bin", bench.bin().display()));
    bench.program("npm", r#"case "$1" in view) echo '{}';; esac"#);
    bench.install(&installer, "npm:wordcount").unwrap();
    bench.program(
        "npm",
        r#"case "$1" in view) echo '{"devDependencies":{"@arcships/rutis-bun":"^0.19.0"}}';; esac"#,
    );
    bench.program("bun", "");
    bench.install(&installer, "npm:clock").unwrap();
    assert_eq!(
        bench.asked(),
        [
            "npm [rutis-host] view wordcount --json",
            "npm [app] install wordcount",
            "npm [rutis-host] view clock --json",
            "bun [app] add clock",
        ]
    );
}

#[test]
fn an_npm_package_depending_on_rutis_bun_is_bun() {
    let bench = Bench::new();
    bench.program(
        "npm",
        r#"case "$1" in view) echo '{"devDependencies":{"@arcships/rutis-bun":"^0.19.0"}}';; esac"#,
    );
    bench.program("bun", "");
    let (lang, package) = bench.install(&bench.installer(), "npm:@ana/clock").unwrap();
    assert_eq!(lang, Lang::Bun);
    assert_eq!(names(&package), [("@ana/clock", "npm:@ana/clock", "bun")]);
    assert_eq!(package.plugins[0].row_name(), "bun:@ana/clock");
    assert_eq!(
        bench.asked(),
        [
            "npm [rutis-host] view @ana/clock --json".to_owned(),
            format!("bun [app] add -d @arcships/rutis-bun@{VERSION}"),
            "bun [app] add @ana/clock".into(),
        ]
    );
}

/// The environment that `uv venv` makes: an interpreter that has no rutis
/// package and answers the distribution query.
const UV: &str = r#"case "$1" in
venv) mkdir -p "$2/bin"; cat > "$2/bin/python" <<'PY'
#!/bin/sh
case "$2" in
"import rutis") echo "python import rutis" >> "$(dirname "$0")/../../../log"; exit 1;;
*) echo "python query $3 $4" >> "$(dirname "$0")/../../../log"; echo '{"name": "bo-llm", "version": "0.3.0", "plugins": ["llm"]}';;
esac
PY
chmod +x "$2/bin/python";;
esac"#;

/// From no environment: it is created, the rutis package of this version
/// added, then the package; its entry points are its plugins.
#[test]
fn a_pypi_package_is_python_in_a_new_environment() {
    let bench = Bench::new();
    bench.program("uv", UV);
    let (lang, package) = bench
        .install(&bench.installer(), "pypi:bo-llm@0.3.0")
        .unwrap();
    assert_eq!(lang, Lang::Python);
    assert_eq!(names(&package), [("llm", "pypi:bo-llm", "py")]);
    assert_eq!(package.plugins[0].version.as_deref(), Some("0.3.0"));
    assert_eq!(package.plugins[0].row_name(), "py:llm");
    let python = "$B/app/.venv/bin/python";
    assert_eq!(
        bench.asked(),
        [
            "uv [rutis-host] --version".to_owned(),
            "uv [rutis-host] venv $B/app/.venv".into(),
            "python import rutis".into(),
            format!("uv [rutis-host] pip install --python {python} rutis=={VERSION}"),
            format!("uv [rutis-host] pip install --python {python} bo-llm==0.3.0"),
            "python query distribution bo-llm".into(),
        ]
    );
    assert_eq!(
        package.uninstall,
        format!("uv pip uninstall --python {python} bo-llm")
            .replace("$B", &bench.dir.path().display().to_string())
    );
}

/// Neither uv nor Python: what to install.
#[test]
fn without_uv_or_python_the_environment_cannot_be_made() {
    let bench = Bench::new();
    let installer = bench.installer().with_path(bench.bin());
    let error = bench.install(&installer, "pypi:bo-llm").err().unwrap();
    assert_eq!(
        error.to_string(),
        format!(
            "cannot create the Python environment {}: install uv, or Python 3.12 or later",
            bench.app().join(".venv").display()
        )
    );
}

/// A package without an entry point of the group stays, and the error
/// says how to uninstall it.
#[test]
fn a_python_package_without_entry_points_is_not_a_plugin() {
    let bench = Bench::new();
    bench.program(
        "uv",
        &UV.replace(r#"["llm"]"#, "[]").replace("bo-llm", "bo-utils"),
    );
    let error = bench
        .install(&bench.installer(), "pypi:bo-utils")
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        format!(
            "bo-utils has no rutis.plugins entry point: it is not a plugin; it stays installed: \
             uv pip uninstall --python {}/app/.venv/bin/python bo-utils",
            bench.dir.path().display()
        )
    );
}

#[test]
fn a_go_module_path_is_go() {
    let bench = Bench::new();
    bench.program("go", "");
    let error = bench
        .install(&bench.installer(), "github.com/bo/netkit/cmd/netkit@v0.4.1")
        .err()
        .unwrap();
    assert_eq!(
        bench.asked(),
        ["go [rutis-host] GOBIN=$B/app/plugins/go install github.com/bo/netkit/cmd/netkit@v0.4.1"]
    );
    // The recording go wrote no binary: not a plugin, and how to remove it.
    let binary = bench.app().join("plugins/go/netkit");
    assert_eq!(
        error.to_string(),
        format!(
            "{} was not built with the rutis Go SDK: it is not a plugin; it stays installed: rm {}",
            binary.display(),
            binary.display()
        )
    );
    // No version: the latest; a major version suffix is not the command.
    bench
        .install(&bench.installer(), "example.com/x/v2")
        .err()
        .unwrap();
    assert_eq!(
        bench.asked()[1],
        "go [rutis-host] GOBIN=$B/app/plugins/go install example.com/x/v2@latest"
    );
    assert_eq!(go_command_name("example.com/x/v2"), "x");
}

/// A binary that says it was built with the Go SDK and prints a manifest.
fn go_binary(path: &Path) {
    std::fs::write(
        path,
        "#!/bin/sh\n# rutis-go-runtime:1\necho '{\"sdk\":\"0.19.0\",\"pluginApi\":3,\"plugins\":{\"dns\":{\"version\":\"0.4.1\",\"provides\":{\"dns\":{\"lookup\":\"async\"}}}}}'\n",
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A local binary with the Go SDK's marker is copied in; its manifest
/// names its plugins.
#[tokio::test]
async fn a_local_go_binary_is_copied_in() {
    let bench = Bench::new();
    go_binary(&bench.app().join("netkit"));
    let installer = bench.installer();
    let (lang, package) = bench.install(&installer, "./netkit").unwrap();
    assert_eq!(lang, Lang::Go);
    assert_eq!(names(&package), [("dns", "./netkit", "go")]);
    assert_eq!(package.plugins[0].version.as_deref(), Some("0.4.1"));
    assert!(bench.app().join("plugins/go/netkit").exists());
    assert!(bench.asked().is_empty());
    // Removed by deleting it.
    let removed = installer.remove(&package.plugins).await.unwrap();
    assert_eq!(removed.as_deref(), Some("netkit"));
    assert!(!bench.app().join("plugins/go/netkit").exists());
}

#[test]
fn a_go_project_directory_is_installed_with_go_install() {
    let bench = Bench::new();
    let project = bench.app().join("netkit");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("go.mod"), "module example.com/netkit\n").unwrap();
    // `go list` answers the binary the recording `go install` would write.
    bench.program(
        "go",
        r#"case "$1" in list) echo "$GOBIN/netkit";; install) cp "$(dirname "$0")/netkit-built" "$GOBIN/netkit";; esac"#,
    );
    go_binary(&bench.bin().join("netkit-built"));
    let (lang, package) = bench.install(&bench.installer(), "./netkit").unwrap();
    assert_eq!(lang, Lang::Go);
    assert_eq!(names(&package), [("dns", "./netkit", "go")]);
    assert_eq!(
        bench.asked(),
        [
            "go [netkit] GOBIN=$B/app/plugins/go install ./...",
            "go [netkit] GOBIN=$B/app/plugins/go list -f {{if eq .Name \"main\"}}{{.Target}}{{end}} ./...",
        ]
    );
}

#[test]
fn local_directories_by_their_project_files() {
    let bench = Bench::new();
    let installer = bench.installer();
    let language = |dir: &str, file: &str, text: &str| {
        let path = bench.app().join(dir);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join(file), text).unwrap();
        let reference = Reference::parse(&format!("./{dir}"), &bench.app()).unwrap();
        installer.core.language(&reference)
    };
    assert_eq!(
        language("notes", "package.json", r#"{"name":"@ana/notes"}"#).unwrap(),
        (Lang::Node, Some("@ana/notes".into()))
    );
    assert_eq!(
        language(
            "clock",
            "package.json",
            r#"{"name":"clock","devDependencies":{"@arcships/rutis-bun":"0.19.0"}}"#
        )
        .unwrap(),
        (Lang::Bun, Some("clock".into()))
    );
    assert_eq!(
        language("llm", "pyproject.toml", "[project]\nname = \"bo-llm\"\n").unwrap(),
        (Lang::Python, None)
    );
    assert_eq!(
        python_distribution(&bench.app().join("llm")).as_deref(),
        Some("bo-llm")
    );
    assert_eq!(
        language("net", "go.mod", "module example.com/net\n").unwrap(),
        (Lang::Go, None)
    );
    assert_eq!(
        language("other", "README.md", "").unwrap_err().to_string(),
        "./other has no package.json, pyproject.toml or go.mod"
    );
}

#[test]
fn local_package_files() {
    let bench = Bench::new();
    let installer = bench.installer();
    // An npm pack: its package.json says the name and language.
    let packed = bench.dir.path().join("pack/package");
    std::fs::create_dir_all(&packed).unwrap();
    std::fs::write(
        packed.join("package.json"),
        r#"{"name":"@ana/wordcount","version":"1.2.0"}"#,
    )
    .unwrap();
    let status = std::process::Command::new("tar")
        .args(["-czf"])
        .arg(bench.app().join("wordcount-1.2.0.tgz"))
        .arg("-C")
        .arg(bench.dir.path().join("pack"))
        .arg("package")
        .status()
        .unwrap();
    assert!(status.success());
    let reference = Reference::parse("./wordcount-1.2.0.tgz", &bench.app()).unwrap();
    assert_eq!(
        installer.core.language(&reference).unwrap(),
        (Lang::Node, Some("@ana/wordcount".into()))
    );
    // A wheel is Python, its distribution in its file name.
    std::fs::write(bench.app().join("bo_llm-0.3.0-py3-none-any.whl"), "").unwrap();
    let reference = Reference::parse("./bo_llm-0.3.0-py3-none-any.whl", &bench.app()).unwrap();
    assert_eq!(
        installer.core.language(&reference).unwrap(),
        (Lang::Python, None)
    );
    assert_eq!(
        python_distribution(&bench.app().join("bo_llm-0.3.0-py3-none-any.whl")).as_deref(),
        Some("bo_llm")
    );
    // Anything else is refused.
    std::fs::write(bench.app().join("notes.txt"), "").unwrap();
    let reference = Reference::parse("./notes.txt", &bench.app()).unwrap();
    assert!(installer.core.language(&reference).is_err());
}

/// npm cannot be run: what to install.
#[test]
fn without_npm() {
    let bench = Bench::new();
    let error = bench
        .install(&bench.installer(), "npm:@ana/wordcount")
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "cannot run npm (No such file or directory): install Node.js 22 or later"
    );
}

/// A package manager failing: its command and exit status.
#[test]
fn a_failing_package_manager() {
    let bench = Bench::new();
    bench.program(
        "npm",
        r#"case "$1" in view) echo '{}';; install) exit 1;; esac"#,
    );
    std::fs::write(bench.app().join("package.json"), "{}").unwrap();
    let error = bench
        .install(&bench.installer(), "npm:@ana/wordcount")
        .err()
        .unwrap();
    assert!(matches!(error, Error::Failed(_)));
    assert_eq!(
        error.to_string(),
        format!(
            "npm install @ana/wordcount @arcships/rutis-runtime@{VERSION} failed (exit status 1)"
        )
    );
}

/// An npm name another package's record has is refused before anything is
/// installed.
#[tokio::test]
async fn a_name_taken_by_another_package() {
    let bench = Bench::new();
    bench.program("npm", r#"case "$1" in view) echo '{}';; esac"#);
    let installer = bench.installer().with_installed([Installed {
        name: "wordcount".into(),
        from: "./plugins/wordcount".into(),
        language: "node".into(),
        version: None,
    }]);
    let error = installer.add("npm:wordcount").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "wordcount is already installed from ./plugins/wordcount: remove it first"
    );
    // The record settles it: npm is not asked.
    assert!(bench.asked().is_empty());

    // A Python plugin of that name has another row name: no conflict.
    let installer = bench.installer().with_installed([Installed {
        name: "wordcount".into(),
        from: "pypi:wordcount".into(),
        language: "py".into(),
        version: None,
    }]);
    let added = installer.add("npm:wordcount").await.unwrap();
    assert_eq!(added.plugins[0].row_name(), "wordcount");
    assert_eq!(bench.asked()[0], "npm [rutis-host] view wordcount --json");
}

#[tokio::test]
async fn removing_a_node_package() {
    let bench = Bench::new();
    bench.program("npm", "");
    let plugin = Installed {
        name: "@ana/wordcount".into(),
        from: "npm:@ana/wordcount".into(),
        language: "node".into(),
        version: None,
    };
    let plugin = [plugin];
    // Not there: nothing to uninstall.
    assert_eq!(bench.installer().remove(&plugin).await.unwrap(), None);
    std::fs::create_dir_all(bench.app().join("node_modules/@ana/wordcount")).unwrap();
    assert_eq!(
        bench.installer().remove(&plugin).await.unwrap().as_deref(),
        Some("@ana/wordcount")
    );
    assert_eq!(bench.asked(), ["npm [app] uninstall @ana/wordcount"]);
}

/// The row name, a record's key, says the name and the language.
#[test]
fn row_names() {
    let plugin = Installed {
        name: "dns".into(),
        from: "github.com/bo/netkit/cmd/netkit".into(),
        language: "go".into(),
        version: None,
    };
    assert_eq!(plugin.row_name(), "go:dns");
    assert_eq!(
        Installed::of_row("go:dns", "github.com/bo/netkit/cmd/netkit"),
        Some(plugin)
    );
    let node = Installed::of_row("@ana/wordcount", "npm:@ana/wordcount").unwrap();
    assert_eq!(
        (node.name.as_str(), node.language.as_str()),
        ("@ana/wordcount", "node")
    );
    assert_eq!(node.row_name(), "@ana/wordcount");
    for row in ["deno:x", "node:x", "py:", "./x"] {
        assert_eq!(Installed::of_row(row, "x"), None, "{row}");
    }
}

/// A binary with the Swift SDK's marker is Swift: copied into the Swift
/// directory, its manifest naming its plugins.
#[tokio::test]
async fn a_local_swift_binary_is_copied_in() {
    let bench = Bench::new();
    let binary = bench.app().join("clock");
    std::fs::write(
        &binary,
        "#!/bin/sh\n# rutis-swift-runtime:1\necho '{\"sdk\":\"0.19.0\",\"pluginApi\":3,\"plugins\":{\"clock\":{\"version\":\"1.0.0\"}}}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let installer = bench.installer();
    let (lang, package) = bench.install(&installer, "./clock").unwrap();
    assert_eq!(lang, Lang::Swift);
    assert_eq!(names(&package), [("clock", "./clock", "swift")]);
    assert_eq!(package.plugins[0].row_name(), "swift:clock");
    assert_eq!(package.plugins[0].version.as_deref(), Some("1.0.0"));
    assert!(bench.app().join("plugins/swift/clock").exists());
    assert!(bench.asked().is_empty());
}

/// A package of two binaries is removed whole: each binary serving one of
/// its plugins is deleted, not only the first plugin's.
#[tokio::test]
async fn removing_a_go_package_deletes_all_its_binaries() {
    let bench = Bench::new();
    let dir = bench.app().join("plugins/go");
    std::fs::create_dir_all(&dir).unwrap();
    go_binary(&dir.join("dns"));
    let http = dir.join("http");
    go_binary(&http);
    let text = std::fs::read_to_string(&http)
        .unwrap()
        .replace("dns", "http");
    std::fs::write(&http, text).unwrap();
    go_binary(&dir.join("other"));
    let text = std::fs::read_to_string(dir.join("other"))
        .unwrap()
        .replace("dns", "other");
    std::fs::write(dir.join("other"), text).unwrap();
    let plugins: Vec<Installed> = ["dns", "http"]
        .into_iter()
        .map(|name| Installed::of_row(&format!("go:{name}"), "./netkit").unwrap())
        .collect();
    let removed = bench.installer().remove(&plugins).await.unwrap();
    assert_eq!(removed.as_deref(), Some("dns, http"));
    assert!(!dir.join("dns").exists());
    assert!(!http.exists());
    // Another package's binary stays.
    assert!(dir.join("other").exists());
}

/// Local directories, npm packs and wheels: the command installing each.
#[test]
fn local_packages_are_installed_by_their_package_managers() {
    let bench = Bench::new();
    bench.program("npm", "");
    bench.program("bun", "");
    bench.program(
        "uv",
        &UV.replace("bo-llm", "bo_llm")
            .replace("\"import rutis\") echo", "\"import rutis\") exit 0; echo"),
    );
    let notes = bench.app().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join("package.json"), r#"{"name":"@ana/notes"}"#).unwrap();
    let clock = bench.app().join("clock");
    std::fs::create_dir_all(&clock).unwrap();
    std::fs::write(
        clock.join("package.json"),
        r#"{"name":"clock","devDependencies":{"@arcships/rutis-bun":"0.19.0"}}"#,
    )
    .unwrap();
    let packed = bench.dir.path().join("pack/package");
    std::fs::create_dir_all(&packed).unwrap();
    std::fs::write(packed.join("package.json"), r#"{"name":"@ana/wordcount"}"#).unwrap();
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(bench.app().join("wordcount-1.2.0.tgz"))
        .arg("-C")
        .arg(bench.dir.path().join("pack"))
        .arg("package")
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(bench.app().join("bo_llm-0.3.0-py3-none-any.whl"), "").unwrap();
    // The runtimes are there: only the packages are installed.
    std::fs::write(bench.app().join("package.json"), "{}").unwrap();
    for runtime in ["rutis-runtime", "rutis-bun"] {
        let dir = bench.app().join("node_modules/@arcships").join(runtime);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), "{}").unwrap();
    }
    let installer = bench.installer();
    let installed: Vec<Vec<(String, String, String)>> = [
        "./notes",
        "./clock",
        "./wordcount-1.2.0.tgz",
        "./bo_llm-0.3.0-py3-none-any.whl",
    ]
    .into_iter()
    .map(|text| {
        let (_, package) = bench.install(&installer, text).unwrap();
        names(&package)
            .into_iter()
            .map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned()))
            .collect()
    })
    .collect();
    let owned = |items: &[(&str, &str, &str)]| -> Vec<(String, String, String)> {
        items
            .iter()
            .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
            .collect()
    };
    assert_eq!(
        installed,
        [
            owned(&[("@ana/notes", "./notes", "node")]),
            owned(&[("clock", "./clock", "bun")]),
            owned(&[("@ana/wordcount", "./wordcount-1.2.0.tgz", "node")]),
            owned(&[("llm", "./bo_llm-0.3.0-py3-none-any.whl", "py")]),
        ]
    );
    let python = "$B/app/.venv/bin/python";
    assert_eq!(
        bench.asked(),
        [
            "npm [app] install $B/app/./notes".to_owned(),
            "bun [app] add $B/app/./clock".into(),
            "npm [app] install $B/app/./wordcount-1.2.0.tgz".into(),
            "uv [rutis-host] --version".into(),
            "uv [rutis-host] venv $B/app/.venv".into(),
            format!("uv [rutis-host] pip install --python {python} $B/app/./bo_llm-0.3.0-py3-none-any.whl"),
            "python query distribution bo_llm".into(),
        ]
    );
}

/// What `npm view` prints is its manifest: anything else is an error, not
/// a Node package.
#[test]
fn npm_view_printing_no_manifest() {
    let bench = Bench::new();
    bench.program("npm", r#"case "$1" in view) echo 'not json';; esac"#);
    let error = bench
        .install(&bench.installer(), "npm:@ana/wordcount")
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "npm view @ana/wordcount --json: expected ident at line 1 column 2"
    );
}

/// A plugin the new version adds whose row name another package's record
/// has is refused, the new version staying installed; a local path has
/// no versions to upgrade to.
#[tokio::test]
async fn upgrading_into_a_name_taken_by_another_package() {
    let bench = Bench::new();
    bench.program(
        "uv",
        &UV.replace(r#"["llm"]"#, r#"["chat", "llm"]"#)
            .replace("\"import rutis\") echo", "\"import rutis\") exit 0; echo"),
    );
    let llm = Installed::of_row("py:llm", "pypi:bo-llm").unwrap();
    let chat = Installed::of_row("py:chat", "pypi:other-chat").unwrap();
    let installer = bench
        .installer()
        .with_installed([llm.clone(), chat.clone()]);
    let error = installer.upgrade(&llm, None).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        bench.here(
            "py:chat is already installed from pypi:other-chat: remove it first; \
             it stays installed: uv pip uninstall --python $B/app/.venv/bin/python bo-llm"
        )
    );
    // Its own plugins are no conflict.
    let installer = bench.installer().with_installed([llm.clone()]);
    let upgraded = installer.upgrade(&llm, None).await.unwrap();
    let plugins: Vec<&str> = upgraded.plugins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(plugins, ["chat", "llm"]);

    let local = Installed::of_row("go:dns", "./netkit").unwrap();
    let error = bench
        .installer()
        .upgrade(&local, Some("1.0"))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "./netkit is a local path, which has no versions: upgrade it without @1.0"
    );
}
