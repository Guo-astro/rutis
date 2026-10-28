//! Plugin references: where a plugin comes from, as `add` takes it
//! (docs/design-plugin-install-2026-10-27.md §四).

use std::path::{Path, PathBuf};

use super::Error;

/// Where a package comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// `npm:<package>[@<version>]`.
    Npm {
        package: String,
        version: Option<String>,
    },
    /// `pypi:<package>[@<version>]`.
    Pypi {
        package: String,
        version: Option<String>,
    },
    /// A Go module path (its first part has a `.`), `[@<version>]`.
    GoModule {
        path: String,
        version: Option<String>,
    },
    /// A local directory, package file or binary: the path made absolute.
    Local(PathBuf),
}

/// A reference read: its source, and the reference without its version
/// (what the record keeps, `from`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reference {
    pub from: String,
    pub source: Source,
}

impl Reference {
    /// Read `text`, local paths relative to `base`.
    pub fn parse(text: &str, base: &Path) -> Result<Self, Error> {
        if is_local(text) {
            let path = Path::new(text);
            let path = match path.is_absolute() {
                true => path.to_owned(),
                false => base.join(path),
            };
            return Ok(Self {
                from: text.to_owned(),
                source: Source::Local(path),
            });
        }
        if let Some(rest) = text.strip_prefix("npm:") {
            let (package, version) = split_version(text, rest)?;
            return Ok(Self {
                from: format!("npm:{package}"),
                source: Source::Npm { package, version },
            });
        }
        if let Some(rest) = text.strip_prefix("pypi:") {
            let (package, version) = split_version(text, rest)?;
            return Ok(Self {
                from: format!("pypi:{package}"),
                source: Source::Pypi { package, version },
            });
        }
        // Go's own rule: a module path's first element has a dot.
        let first = text.split('/').next().unwrap_or_default();
        if first.contains('.') && !first.contains(':') && !first.starts_with('@') {
            let (path, version) = split_version(text, text)?;
            return Ok(Self {
                from: path.clone(),
                source: Source::GoModule { path, version },
            });
        }
        Err(Error::Other(format!(
            "{text}: say where it comes from: npm:{text}, pypi:{text}, a Go module path or a local path"
        )))
    }
}

/// Whether `text` is a local path: `./`, `../`, `/` (and on Windows `.\`,
/// `..\`, `\` and a drive letter), as a row's plugin file is.
pub(crate) fn is_local(text: &str) -> bool {
    let unix = ["./", "../", "/"]
        .iter()
        .any(|start| text.starts_with(start));
    let windows = cfg!(windows)
        && ([".\\", "..\\", "\\"]
            .iter()
            .any(|start| text.starts_with(start))
            || text.as_bytes().get(1) == Some(&b':')
                && text.as_bytes()[0].is_ascii_alphabetic()
                && matches!(text.as_bytes().get(2), Some(b'\\' | b'/')));
    unix || windows
}

/// `name[@version]`: the version follows the last `@` that does not start
/// the name (`@ana/wordcount@1.2.0`).
fn split_version(text: &str, rest: &str) -> Result<(String, Option<String>), Error> {
    let (name, version) = match rest.rfind('@').filter(|at| *at > 0) {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    if name.is_empty() || version == Some("") {
        return Err(Error::Other(format!(
            "{text}: name a package, and a version after @ if any"
        )));
    }
    Ok((name.to_owned(), version.map(str::to_owned)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Reference {
        Reference::parse(text, Path::new("/app")).unwrap()
    }

    #[test]
    fn npm_references() {
        assert_eq!(
            parse("npm:@ana/wordcount"),
            Reference {
                from: "npm:@ana/wordcount".into(),
                source: Source::Npm {
                    package: "@ana/wordcount".into(),
                    version: None
                }
            }
        );
        assert_eq!(
            parse("npm:@ana/wordcount@1.2.0"),
            Reference {
                from: "npm:@ana/wordcount".into(),
                source: Source::Npm {
                    package: "@ana/wordcount".into(),
                    version: Some("1.2.0".into())
                }
            }
        );
        assert_eq!(
            parse("npm:wordcount@next").source,
            Source::Npm {
                package: "wordcount".into(),
                version: Some("next".into())
            }
        );
    }

    #[test]
    fn pypi_references() {
        assert_eq!(
            parse("pypi:bo-llm@0.3.0"),
            Reference {
                from: "pypi:bo-llm".into(),
                source: Source::Pypi {
                    package: "bo-llm".into(),
                    version: Some("0.3.0".into())
                }
            }
        );
        assert_eq!(parse("pypi:bo-llm").from, "pypi:bo-llm");
    }

    #[test]
    fn go_module_references() {
        assert_eq!(
            parse("github.com/bo/netkit/cmd/netkit@v0.4.1"),
            Reference {
                from: "github.com/bo/netkit/cmd/netkit".into(),
                source: Source::GoModule {
                    path: "github.com/bo/netkit/cmd/netkit".into(),
                    version: Some("v0.4.1".into())
                }
            }
        );
        assert_eq!(
            parse("github.com/bo/netkit/cmd/netkit").source,
            Source::GoModule {
                path: "github.com/bo/netkit/cmd/netkit".into(),
                version: None
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_references() {
        for (text, path) in [
            ("./plugins/notes", "/app/./plugins/notes"),
            ("../wordcount-1.2.0.tgz", "/app/../wordcount-1.2.0.tgz"),
            ("/opt/netkit", "/opt/netkit"),
            ("./a@b/c", "/app/./a@b/c"),
        ] {
            assert_eq!(
                parse(text),
                Reference {
                    from: text.into(),
                    source: Source::Local(PathBuf::from(path))
                }
            );
        }
    }

    #[test]
    fn a_name_without_a_source_is_refused() {
        for text in ["wordcount", "go:dns", "py:llm", "@ana/wordcount"] {
            assert_eq!(
                Reference::parse(text, Path::new("/app"))
                    .unwrap_err()
                    .to_string(),
                format!(
                    "{text}: say where it comes from: npm:{text}, pypi:{text}, a Go module path or a local path"
                )
            );
        }
        assert!(Reference::parse("npm:", Path::new("/app")).is_err());
        assert!(Reference::parse("npm:x@", Path::new("/app")).is_err());
    }
}
