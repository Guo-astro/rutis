//! `rutis-host new <name> --lang node|bun|python|go`: a plugin project from a
//! template, with a working plugin, its test, a dev configuration and a
//! publish workflow.

use std::path::Path;

/// The release train this program belongs to: the projects depend on the
/// SDK of the same version. Every train release tags the Go SDK
/// (`go/rutis/v<version>`, release.yml), so a released rutis-host makes Go
/// projects that build; one built from an unreleased commit names a
/// version that has no tag yet (point the project's go.mod at a checkout
/// with `replace` meanwhile).
const VERSION: &str = env!("CARGO_PKG_VERSION");

const NODE: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!("../templates/node/package.json"),
    ),
    (
        "tsconfig.json",
        include_str!("../templates/node/tsconfig.json"),
    ),
    (
        "src/index.ts",
        include_str!("../templates/node/src/index.ts"),
    ),
    (
        "test/index.test.ts",
        include_str!("../templates/node/test/index.test.ts"),
    ),
    (
        "rutis.dev.json",
        include_str!("../templates/node/rutis.dev.json"),
    ),
    ("README.md", include_str!("../templates/node/README.md")),
    (".gitignore", include_str!("../templates/node/gitignore")),
    (
        ".github/workflows/publish.yml",
        include_str!("../templates/node/.github/workflows/publish.yml"),
    ),
];

const BUN: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!("../templates/bun/package.json"),
    ),
    (
        "tsconfig.json",
        include_str!("../templates/bun/tsconfig.json"),
    ),
    (
        "src/index.ts",
        include_str!("../templates/bun/src/index.ts"),
    ),
    (
        "test/index.test.ts",
        include_str!("../templates/bun/test/index.test.ts"),
    ),
    (
        "rutis.dev.json",
        include_str!("../templates/bun/rutis.dev.json"),
    ),
    ("README.md", include_str!("../templates/bun/README.md")),
    (".gitignore", include_str!("../templates/bun/gitignore")),
    (
        ".github/workflows/publish.yml",
        include_str!("../templates/bun/.github/workflows/publish.yml"),
    ),
];

const PYTHON: &[(&str, &str)] = &[
    (
        "pyproject.toml",
        include_str!("../templates/python/pyproject.toml"),
    ),
    (
        "src/__module__/__init__.py",
        include_str!("../templates/python/src/__module__/__init__.py"),
    ),
    (
        "tests/test_plugin.py",
        include_str!("../templates/python/tests/test_plugin.py"),
    ),
    (
        "rutis.dev.json",
        include_str!("../templates/python/rutis.dev.json"),
    ),
    ("README.md", include_str!("../templates/python/README.md")),
    (".gitignore", include_str!("../templates/python/gitignore")),
    (
        ".github/workflows/publish.yml",
        include_str!("../templates/python/.github/workflows/publish.yml"),
    ),
];

const GO: &[(&str, &str)] = &[
    ("go.mod", include_str!("../templates/go/go.mod")),
    ("plugin.go", include_str!("../templates/go/plugin.go")),
    (
        "plugin_test.go",
        include_str!("../templates/go/plugin_test.go"),
    ),
    (
        "cmd/__name__/main.go",
        include_str!("../templates/go/cmd/__name__/main.go"),
    ),
    (
        "rutis.dev.json",
        include_str!("../templates/go/rutis.dev.json"),
    ),
    ("README.md", include_str!("../templates/go/README.md")),
    (".gitignore", include_str!("../templates/go/gitignore")),
    (
        ".github/workflows/release.yml",
        include_str!("../templates/go/.github/workflows/release.yml"),
    ),
];

/// Create the project `name` in `parent`/`name`.
pub fn create(parent: &Path, name: &str, lang: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name.starts_with(|c: char| c.is_ascii_lowercase());
    if !valid {
        return Err(format!(
            "{name:?}: a plugin name is lower case letters, digits and -, starting with a letter"
        ));
    }
    let files = match lang {
        "node" | "ts" | "typescript" | "js" => NODE,
        "bun" => BUN,
        "python" | "py" => PYTHON,
        "go" | "golang" => GO,
        other => {
            return Err(format!(
                "{other:?}: the language is node, bun, python or go"
            ))
        }
    };
    let dir = parent.join(name);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let module = name.replace('-', "_");
    // A Go package name has no `-` or `_`.
    let package = name.replace('-', "");
    let next = next_minor(VERSION);
    for (path, text) in files {
        let path = path
            .replace("__module__", &module)
            .replace("__name__", name);
        let text = text
            .replace("{{package}}", &package)
            .replace("{{name}}", name)
            .replace("{{id}}", name)
            .replace("{{module}}", &module)
            .replace("{{version}}", VERSION)
            .replace("{{next}}", &next);
        let target = dir.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        std::fs::write(&target, text).map_err(|error| format!("{}: {error}", target.display()))?;
    }
    Ok(())
}

/// The first version past this one's compatible range: `0.2.x` → `0.3`,
/// `1.4.0` → `2`.
fn next_minor(version: &str) -> String {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    match major {
        0 => format!("0.{}", minor + 1),
        major => format!("{}", major + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_are_created_with_their_names_filled_in() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), "weather-plugin", "python").unwrap();
        let pyproject =
            std::fs::read_to_string(dir.path().join("weather-plugin/pyproject.toml")).unwrap();
        assert!(
            pyproject.contains("weather-plugin = \"weather_plugin\""),
            "{pyproject}"
        );
        assert!(dir
            .path()
            .join("weather-plugin/src/weather_plugin/__init__.py")
            .exists());
        create(dir.path(), "greeter", "node").unwrap();
        let package = std::fs::read_to_string(dir.path().join("greeter/package.json")).unwrap();
        assert!(package.contains(&format!("\"@arcships/rutis\": \"^{VERSION}\"")));
        create(dir.path(), "bun-greeter", "bun").unwrap();
        let package = std::fs::read_to_string(dir.path().join("bun-greeter/package.json")).unwrap();
        assert!(package.contains(&format!("\"@arcships/rutis-bun\": \"^{VERSION}\"")));
        assert!(dir.path().join("bun-greeter/test/index.test.ts").exists());
        assert!(
            create(dir.path(), "greeter", "node").is_err(),
            "an existing directory is kept"
        );
        assert!(create(dir.path(), "Bad_Name", "node").is_err());
        create(dir.path(), "net-probe", "go").unwrap();
        let main =
            std::fs::read_to_string(dir.path().join("net-probe/cmd/net-probe/main.go")).unwrap();
        assert!(
            main.contains("netprobe \"example.com/net-probe\""),
            "{main}"
        );
        let module = std::fs::read_to_string(dir.path().join("net-probe/go.mod")).unwrap();
        assert!(module.contains(&format!("github.com/arcships/rutis/go/rutis v{VERSION}")));
        let plugin = std::fs::read_to_string(dir.path().join("net-probe/plugin.go")).unwrap();
        assert!(
            plugin.starts_with("// Package netprobe") && plugin.contains("Name: \"net-probe\"")
        );
        // Each project's dev configuration names its format (risk J3,
        // guarantee: host/rutis-json-format).
        for project in ["weather-plugin", "greeter", "bun-greeter", "net-probe"] {
            let path = dir.path().join(project).join("rutis.dev.json");
            let text = std::fs::read_to_string(&path).unwrap();
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["format"], crate::config::FORMAT, "{project}");
            crate::config::HostConfig::read(&path).unwrap();
        }
        assert_eq!(next_minor("0.2.0"), "0.3");
        assert_eq!(next_minor("1.4.2"), "2");
    }
}
