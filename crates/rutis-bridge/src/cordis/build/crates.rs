//! The source of the crates an application builds with, as the generator of
//! exported services reads it: which crate a name in the application's
//! `build.rs` is (`cargo metadata`), the module tree of each crate read with
//! syn from its entry file, and what a path written in a module names
//! (its `use` items, globs, re-exports, the extern prelude and the standard
//! prelude). Stable Rust only: no macro expansion, so items a macro writes
//! are not seen.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Crates whose items the generator knows by their path and never reads:
/// the standard library and the crates whose types it maps itself.
const OPAQUE: [&str; 16] = [
    "std",
    "core",
    "alloc",
    "serde",
    "serde_json",
    "rutis",
    "rutis_bridge",
    "futures",
    "futures_util",
    "futures_core",
    "tokio",
    "anyhow",
    "thiserror",
    "chrono",
    "uuid",
    "url",
];

/// What a path names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Res {
    Module(usize),
    /// An item of a module read: `module`'s `index`-th item.
    Item {
        module: usize,
        index: usize,
    },
    /// An item of a crate not read, by its path from that crate's root
    /// (`std::collections::HashMap`; `core` and `alloc` read as `std`).
    External(Vec<String>),
    Primitive(String),
}

#[derive(Clone, Debug)]
struct UsePath {
    leading: bool,
    segments: Vec<String>,
}

#[derive(Clone, Debug)]
enum Name {
    Item(usize),
    Module(usize),
    Use(UsePath),
}

pub(super) struct Module {
    pub(super) package: usize,
    pub(super) parent: Option<usize>,
    pub(super) items: Vec<syn::Item>,
    pub(super) file: PathBuf,
    names: HashMap<String, Vec<Name>>,
    globs: Vec<UsePath>,
}

struct Package {
    /// The crate name of its library (or first binary), as code writes it.
    name: String,
    entry: Option<PathBuf>,
    deps: HashMap<String, String>,
    features: HashSet<String>,
}

pub(super) struct Sources {
    packages: Vec<Package>,
    by_id: HashMap<String, usize>,
    host: usize,
    pub(super) modules: Vec<Module>,
    roots: HashMap<usize, usize>,
    /// Every file read, for `cargo:rerun-if-changed`.
    pub(super) inputs: BTreeSet<PathBuf>,
}

type Result<T> = std::result::Result<T, String>;

impl Sources {
    /// The crates of the package being built (`CARGO_MANIFEST_DIR`), as
    /// `cargo metadata` reports them.
    pub(super) fn of_build() -> Result<Self> {
        let manifest = PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR is not set")?,
        )
        .join("Cargo.toml");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = std::process::Command::new(cargo)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--offline",
                "--manifest-path",
            ])
            .arg(&manifest)
            .output()
            .map_err(|error| format!("cannot run cargo metadata: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cargo metadata failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("cargo metadata: {error}"))?;
        let host = metadata["resolve"]["root"]
            .as_str()
            .ok_or("cargo metadata names no root package")?
            .to_owned();
        Self::from_metadata(&metadata, &host)
    }

    pub(super) fn from_metadata(metadata: &serde_json::Value, host: &str) -> Result<Self> {
        let mut packages = Vec::new();
        let mut by_id = HashMap::new();
        let nodes: HashMap<&str, &serde_json::Value> = metadata["resolve"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|node| Some((node["id"].as_str()?, node)))
            .collect();
        for package in metadata["packages"].as_array().into_iter().flatten() {
            let id = package["id"].as_str().unwrap_or_default().to_owned();
            let targets: Vec<&serde_json::Value> = package["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .collect();
            let kind = |target: &&serde_json::Value, kind: &str| {
                target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|k| k == kind))
            };
            let lib = targets
                .iter()
                .find(|target| {
                    kind(target, "lib") || kind(target, "rlib") || kind(target, "proc-macro")
                })
                .or_else(|| targets.iter().find(|target| kind(target, "bin")));
            let name = lib
                .and_then(|target| target["name"].as_str())
                .or(package["name"].as_str())
                .unwrap_or_default()
                .replace('-', "_");
            let entry = lib
                .and_then(|target| target["src_path"].as_str())
                .map(PathBuf::from);
            let node = nodes.get(id.as_str());
            let deps = node
                .and_then(|node| node["deps"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|dep| {
                    Some((
                        dep["name"].as_str()?.to_owned(),
                        dep["pkg"].as_str()?.to_owned(),
                    ))
                })
                .collect();
            let features = node
                .and_then(|node| node["features"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|feature| feature.as_str().map(str::to_owned))
                .collect();
            by_id.insert(id, packages.len());
            packages.push(Package {
                name,
                entry,
                deps,
                features,
            });
        }
        let host = *by_id
            .get(host)
            .ok_or_else(|| format!("cargo metadata does not list the package {host}"))?;
        Ok(Self {
            packages,
            by_id,
            host,
            modules: Vec::new(),
            roots: HashMap::new(),
            inputs: BTreeSet::new(),
        })
    }

    /// The crate root module of the package being built.
    pub(super) fn host_root(&mut self) -> Result<usize> {
        self.root(self.host)
    }

    /// The root module of `package`, reading the crate on first use.
    fn root(&mut self, package: usize) -> Result<usize> {
        if let Some(root) = self.roots.get(&package) {
            return Ok(*root);
        }
        let entry = self.packages[package].entry.clone().ok_or_else(|| {
            format!(
                "crate {} has no library or binary to read",
                self.packages[package].name
            )
        })?;
        let dir = entry.parent().map(Path::to_owned).unwrap_or_default();
        let root = self.load(package, None, &entry, dir)?;
        self.roots.insert(package, root);
        Ok(root)
    }

    fn load(
        &mut self,
        package: usize,
        parent: Option<usize>,
        file: &Path,
        dir: PathBuf,
    ) -> Result<usize> {
        let text = std::fs::read_to_string(file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        self.inputs.insert(file.to_owned());
        let parsed = syn::parse_file(&text).map_err(|error| {
            let start = error.span().start();
            format!(
                "{}:{}:{}: {error}",
                file.display(),
                start.line,
                start.column + 1
            )
        })?;
        self.add(package, parent, parsed.items, file.to_owned(), dir)
    }

    /// Record a module and, recursively, the modules it declares.
    fn add(
        &mut self,
        package: usize,
        parent: Option<usize>,
        items: Vec<syn::Item>,
        file: PathBuf,
        dir: PathBuf,
    ) -> Result<usize> {
        let id = self.modules.len();
        self.modules.push(Module {
            package,
            parent,
            items: Vec::new(),
            file: file.clone(),
            names: HashMap::new(),
            globs: Vec::new(),
        });
        let mut names: HashMap<String, Vec<Name>> = HashMap::new();
        let mut globs = Vec::new();
        let items: Vec<syn::Item> = items
            .into_iter()
            .filter(|item| self.enabled(package, attrs(item)))
            .collect();
        for (index, item) in items.iter().enumerate() {
            let ident = match item {
                Item::Struct(item) => &item.ident,
                Item::Enum(item) => &item.ident,
                Item::Union(item) => &item.ident,
                Item::Type(item) => &item.ident,
                Item::Trait(item) => &item.ident,
                Item::Use(item) => {
                    flatten(
                        Vec::new(),
                        item.leading_colon.is_some(),
                        &item.tree,
                        &mut names,
                        &mut globs,
                    );
                    continue;
                }
                Item::Mod(item) => {
                    let name = item.ident.to_string();
                    let child_dir = dir.join(&name);
                    let child = match &item.content {
                        Some((_, content)) => Some(self.add(
                            package,
                            Some(id),
                            content.clone(),
                            file.clone(),
                            child_dir,
                        )?),
                        None => {
                            let path = path_attribute(&item.attrs);
                            let candidates = match &path {
                                Some(path) => {
                                    vec![file.parent().unwrap_or(Path::new("")).join(path)]
                                }
                                None => vec![
                                    dir.join(format!("{name}.rs")),
                                    dir.join(&name).join("mod.rs"),
                                ],
                            };
                            match candidates.into_iter().find(|candidate| candidate.exists()) {
                                Some(found) => {
                                    let is_mod_rs =
                                        found.file_name().is_some_and(|f| f == "mod.rs");
                                    let child_dir = if is_mod_rs || path.is_some() {
                                        found.parent().map(Path::to_owned).unwrap_or_default()
                                    } else {
                                        child_dir
                                    };
                                    Some(self.load(package, Some(id), &found, child_dir)?)
                                }
                                // A module of another platform, say.
                                None => None,
                            }
                        }
                    };
                    if let Some(child) = child {
                        names.entry(name).or_default().push(Name::Module(child));
                    }
                    continue;
                }
                _ => continue,
            };
            names
                .entry(ident.to_string())
                .or_default()
                .push(Name::Item(index));
        }
        let module = &mut self.modules[id];
        module.items = items;
        module.names = names;
        module.globs = globs;
        Ok(id)
    }

    /// Whether the `#[cfg]`s of an item hold for this build: features as
    /// Cargo resolved them, the target's `cfg`s as Cargo tells build
    /// scripts (`CARGO_CFG_*`); `test` and `doc` do not hold.
    pub(super) fn enabled(&self, package: usize, attrs: &[syn::Attribute]) -> bool {
        attrs
            .iter()
            .filter(|attr| attr.path().is_ident("cfg"))
            .all(|attr| match attr.parse_args::<syn::Meta>() {
                Ok(meta) => self.holds(package, &meta),
                Err(_) => true,
            })
    }

    fn holds(&self, package: usize, meta: &syn::Meta) -> bool {
        match meta {
            syn::Meta::Path(path) => {
                let name = path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                match name.as_str() {
                    "test" | "doc" | "doctest" | "miri" => false,
                    name => {
                        std::env::var_os(format!("CARGO_CFG_{}", name.to_uppercase())).is_some()
                    }
                }
            }
            syn::Meta::NameValue(pair) => {
                let key = pair
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(value),
                    ..
                }) = &pair.value
                else {
                    return false;
                };
                let value = value.value();
                if key == "feature" {
                    return self.packages[package].features.contains(&value);
                }
                std::env::var(format!("CARGO_CFG_{}", key.to_uppercase()))
                    .is_ok_and(|values| values.split(',').any(|v| v == value))
            }
            syn::Meta::List(list) => {
                let name = list
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                let inner = list
                    .parse_args_with(
                        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                    )
                    .map(|metas| metas.into_iter().collect::<Vec<_>>())
                    .unwrap_or_default();
                match name.as_str() {
                    "not" => !inner.first().is_some_and(|meta| self.holds(package, meta)),
                    "all" => inner.iter().all(|meta| self.holds(package, meta)),
                    "any" => inner.iter().any(|meta| self.holds(package, meta)),
                    _ => false,
                }
            }
        }
    }

    /// What `path` names when written in `module`.
    pub(super) fn resolve(&mut self, module: usize, path: &syn::Path) -> Result<Option<Res>> {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.resolve_segments(module, path.leading_colon.is_some(), &segments, 0)
    }

    fn resolve_segments(
        &mut self,
        module: usize,
        leading: bool,
        segments: &[String],
        depth: usize,
    ) -> Result<Option<Res>> {
        if depth > 48 {
            return Ok(None);
        }
        let Some((first, rest)) = segments.split_first() else {
            return Ok(Some(Res::Module(module)));
        };
        let mut current = if leading {
            self.extern_crate(module, first)?
        } else {
            match first.as_str() {
                "crate" => Some(Res::Module(self.root(self.modules[module].package)?)),
                "self" => Some(Res::Module(module)),
                "super" => self.modules[module].parent.map(Res::Module),
                "Self" => None,
                _ => self.lookup(module, first, true, depth + 1)?,
            }
        };
        for segment in rest {
            current = match current {
                Some(Res::Module(inner)) => match segment.as_str() {
                    "super" => self.modules[inner].parent.map(Res::Module),
                    "self" => Some(Res::Module(inner)),
                    _ => self.lookup(inner, segment, false, depth + 1)?,
                },
                Some(Res::External(mut path)) => {
                    path.push(segment.clone());
                    Some(Res::External(path))
                }
                _ => None,
            };
        }
        Ok(current)
    }

    /// What `name` is in `module`: its own items, its imports, its globs,
    /// and (`scope`: the first segment of a path) the extern and standard
    /// preludes.
    fn lookup(
        &mut self,
        module: usize,
        name: &str,
        scope: bool,
        depth: usize,
    ) -> Result<Option<Res>> {
        if depth > 48 {
            return Ok(None);
        }
        let entries = self.modules[module]
            .names
            .get(name)
            .cloned()
            .unwrap_or_default();
        for entry in &entries {
            match entry {
                Name::Item(index) => {
                    return Ok(Some(Res::Item {
                        module,
                        index: *index,
                    }))
                }
                Name::Module(child) => return Ok(Some(Res::Module(*child))),
                Name::Use(_) => {}
            }
        }
        for entry in entries {
            if let Name::Use(path) = entry {
                // `use serde_json;` names the extern crate, not itself.
                let found = if !path.leading && path.segments.len() == 1 && path.segments[0] == name
                {
                    self.extern_crate(module, name)?
                } else {
                    self.resolve_segments(module, path.leading, &path.segments, depth + 1)?
                };
                if found.is_some() {
                    return Ok(found);
                }
            }
        }
        if scope {
            if let Some(found) = self.extern_crate(module, name)? {
                return Ok(Some(found));
            }
        }
        let globs = self.modules[module].globs.clone();
        for glob in globs {
            match self.resolve_segments(module, glob.leading, &glob.segments, depth + 1)? {
                Some(Res::Module(inner)) if inner != module => {
                    if let Some(found) = self.lookup(inner, name, false, depth + 1)? {
                        return Ok(Some(found));
                    }
                }
                // An enum's variants, or a crate not read: nothing to type.
                _ => {}
            }
        }
        if scope {
            return Ok(prelude(name));
        }
        Ok(None)
    }

    /// The extern crate `name` as `module`'s crate sees it.
    fn extern_crate(&mut self, module: usize, name: &str) -> Result<Option<Res>> {
        if matches!(name, "std" | "core" | "alloc") {
            return Ok(Some(Res::External(vec!["std".into()])));
        }
        let package = self.modules[module].package;
        let Some(dependency) = self.packages[package].deps.get(name).cloned() else {
            return Ok(None);
        };
        let Some(&index) = self.by_id.get(&dependency) else {
            return Ok(None);
        };
        let crate_name = self.packages[index].name.clone();
        if OPAQUE.contains(&crate_name.as_str()) || self.packages[index].entry.is_none() {
            return Ok(Some(Res::External(vec![crate_name])));
        }
        Ok(Some(Res::Module(self.root(index)?)))
    }

    /// Every module of the crate `module` is in.
    pub(super) fn crate_modules(&self, module: usize) -> Vec<usize> {
        let package = self.modules[module].package;
        (0..self.modules.len())
            .filter(|id| self.modules[*id].package == package)
            .collect()
    }

    pub(super) fn item(&self, module: usize, index: usize) -> &syn::Item {
        &self.modules[module].items[index]
    }

    pub(super) fn package_of(&self, module: usize) -> usize {
        self.modules[module].package
    }

    /// Where an item is, for messages: `file:line:column`.
    pub(super) fn location(&self, module: usize, node: &impl syn::spanned::Spanned) -> String {
        let start = node.span().start();
        format!(
            "{}:{}:{}",
            self.modules[module].file.display(),
            start.line,
            start.column + 1
        )
    }
}

use syn::Item;

fn attrs(item: &Item) -> &[syn::Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn path_attribute(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| match &attr.meta {
        syn::Meta::NameValue(pair) if pair.path.is_ident("path") => match &pair.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(value),
                ..
            }) => Some(value.value()),
            _ => None,
        },
        _ => None,
    })
}

/// A `use` tree as the names it brings in and the globs it opens.
fn flatten(
    mut prefix: Vec<String>,
    leading: bool,
    tree: &syn::UseTree,
    names: &mut HashMap<String, Vec<Name>>,
    globs: &mut Vec<UsePath>,
) {
    let (ident, alias) = match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            return flatten(prefix, leading, &path.tree, names, globs);
        }
        syn::UseTree::Glob(_) => {
            return globs.push(UsePath {
                leading,
                segments: prefix,
            })
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                flatten(prefix.clone(), leading, tree, names, globs);
            }
            return;
        }
        syn::UseTree::Name(item) => (item.ident.to_string(), None),
        syn::UseTree::Rename(item) => (item.ident.to_string(), Some(item.rename.to_string())),
    };
    if ident != "self" {
        prefix.push(ident.clone());
    }
    let name = alias.unwrap_or_else(|| prefix.last().cloned().unwrap_or(ident));
    if name != "_" {
        names.entry(name).or_default().push(Name::Use(UsePath {
            leading,
            segments: prefix,
        }));
    }
}

/// The standard prelude's types, and the primitive types.
fn prelude(name: &str) -> Option<Res> {
    const PRIMITIVES: [&str; 17] = [
        "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32",
        "i64", "i128", "isize", "f32", "f64",
    ];
    if PRIMITIVES.contains(&name) {
        return Some(Res::Primitive(name.to_owned()));
    }
    let path: &[&str] = match name {
        "String" => &["std", "string", "String"],
        "Vec" => &["std", "vec", "Vec"],
        "Option" => &["std", "option", "Option"],
        "Result" => &["std", "result", "Result"],
        "Box" => &["std", "boxed", "Box"],
        _ => return None,
    };
    Some(Res::External(
        path.iter().map(|s| (*s).to_owned()).collect(),
    ))
}
