//! The TypeScript declarations of the Rust services marked
//! `#[rutis::service]`, generated from their Rust definitions, read from the
//! source of the crates that define them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use quote::ToTokens;
use syn::visit::Visit;
use syn::{
    FnArg, GenericArgument, ImplItem, Item, PathArguments, ReturnType, Type, TypeParamBound,
};

use super::crates::{Res, Sources};

type Result<T> = std::result::Result<T, String>;

/// The TypeScript declarations of the services marked `#[rutis::service]`
/// in a package and in every crate it uses that depends on rutis, written
/// from an application's `build.rs` on every build:
///
/// ```ignore
/// rutis_bridge::build::Exports::new()
///     .typescript("rows/services.d.ts")
///     .generate()
///     .expect("generate the service declarations");
/// ```
///
/// `rutis-host types` writes the same, outside a build script
/// ([`Exports::declarations`]). Each marked type is an interface of the
/// public `&self` methods of its marked blocks, each `async` for the caller
/// (a Promise / awaitable); a method the generator cannot declare is left
/// out with a build warning saying why. The source is found through
/// `cargo metadata`.
///
/// A type whose source you cannot change is wrapped in a type of your own,
/// marked, whose methods call it.
#[derive(Default)]
pub struct Exports {
    declared: Vec<(String, String)>,
    declared_python: Vec<(String, String)>,
    typescript: Option<PathBuf>,
    python: Option<PathBuf>,
    contract: Option<PathBuf>,
}

impl Exports {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare the TypeScript type of the Rust type at `path` (from the
    /// application crate), for a type whose serde form the generator cannot
    /// derive from its definition (a hand-written `Serialize`, a
    /// `serialize_with`): for example
    /// `.declared("events::AgentEvent", "import('./agent').AgentEvent")`.
    pub fn declared(mut self, path: &str, typescript: &str) -> Self {
        self.declared.push((path.to_owned(), typescript.to_owned()));
        self
    }

    /// Declare the Python type of a type declared with
    /// [`Exports::declared`], written as given into the Python module
    /// (which imports `typing` and `rutis`): for example
    /// `.declared_python("notes::Text", "str")`. Without it the type is
    /// `typing.Any` in Python, with a warning.
    pub fn declared_python(mut self, path: &str, python: &str) -> Self {
        self.declared_python
            .push((path.to_owned(), python.to_owned()));
        self
    }

    /// Write the TypeScript declarations to `path` (relative to the
    /// package's `Cargo.toml`), where the rows' TypeScript finds them. It is
    /// written only when it changes.
    pub fn typescript(mut self, path: impl AsRef<Path>) -> Self {
        self.typescript = Some(path.as_ref().to_owned());
        self
    }

    /// Write the Python types to `path` (relative to the package's
    /// `Cargo.toml`): a module for Python 3.10 and later, where each service
    /// is a `typing.Protocol` and each struct a `typing.TypedDict`, for
    /// `notes: Notes = ctx.use("notes")`. It is written only when it
    /// changes.
    pub fn python(mut self, path: impl AsRef<Path>) -> Self {
        self.python = Some(path.as_ref().to_owned());
        self
    }

    /// Write the contract description to `path` (relative to the package's
    /// `Cargo.toml`): a JSON file the testing tools of
    /// `@arcships/rutis` and `rutis` read (their `contract` option), to fail
    /// a test where these services would refuse the call. It describes the
    /// types only, is read by the testing tools of the same major version,
    /// and is written only when it changes.
    pub fn contract(mut self, path: impl AsRef<Path>) -> Self {
        self.contract = Some(path.as_ref().to_owned());
        self
    }

    /// Write the files named with [`Exports::typescript`],
    /// [`Exports::python`] and [`Exports::contract`], and have Cargo run the
    /// build script again
    /// when a source it read changes.
    pub fn generate(self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        if self.typescript.is_none() && self.python.is_none() && self.contract.is_none() {
            return Err(
                "nothing to generate: name a file with .typescript(path), .python(path) or .contract(path)"
                    .into(),
            );
        }
        println!("cargo:rerun-if-changed=build.rs");
        let mut sources = Sources::of_build()?;
        let generated = self.generate_from(&mut sources);
        for input in &sources.inputs {
            println!("cargo:rerun-if-changed={}", input.display());
        }
        let generated = generated?;
        for warning in &generated.warnings {
            println!("cargo:warning={warning}");
        }
        let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
        self.write_files(&root, &generated)?;
        Ok(())
    }

    /// The TypeScript declarations of the services marked
    /// `#[rutis::service]` in the crate in `crate_dir` (its `Cargo.toml`)
    /// and in the crates it uses that depend on rutis. What `rutis-host
    /// types` writes, outside a build script.
    pub fn declarations(
        self,
        crate_dir: impl AsRef<Path>,
    ) -> std::result::Result<String, Box<dyn std::error::Error>> {
        Ok(self.of_crate(crate_dir.as_ref())?.typescript)
    }

    /// What `rutis-host types` does: generate for the crate in
    /// `crate_dir`, write the files named with [`Exports::typescript`],
    /// [`Exports::python`] and [`Exports::contract`] (relative to the
    /// current directory), and
    /// return the TypeScript declarations.
    pub fn write(
        self,
        crate_dir: impl AsRef<Path>,
    ) -> std::result::Result<String, Box<dyn std::error::Error>> {
        let generated = self.of_crate(crate_dir.as_ref())?;
        self.write_files(Path::new("."), &generated)?;
        Ok(generated.typescript)
    }

    /// Generate for the crate in `crate_dir`, its warnings on stderr: what
    /// [`Exports::declarations`] and [`Exports::write`] share.
    fn of_crate(
        &self,
        crate_dir: &Path,
    ) -> std::result::Result<Generated, Box<dyn std::error::Error>> {
        let mut sources = Sources::of_crate(crate_dir)?;
        let generated = self.generate_from(&mut sources)?;
        for warning in &generated.warnings {
            eprintln!("warning: {warning}");
        }
        Ok(generated)
    }

    /// Write each named file under `root`, only when it changes.
    fn write_files(
        &self,
        root: &Path,
        generated: &Generated,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let files = [
            (&self.typescript, &generated.typescript),
            (&self.python, &generated.python),
            (&self.contract, &generated.contract),
        ];
        for (path, text) in files {
            let Some(path) = path else { continue };
            let path = root.join(path);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                if let Some(parent) = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, text)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
            }
        }
        Ok(())
    }

    pub(super) fn generate_from(&self, sources: &mut Sources) -> Result<Generated> {
        let root = sources.host_root()?;
        let mut generator = Generator {
            sources,
            declared: HashMap::new(),
            python: self.python.is_some(),
            untyped: HashSet::new(),
            named: HashMap::new(),
            names: HashSet::new(),
            declarations: Vec::new(),
            thrown: BTreeMap::new(),
            warnings: Vec::new(),
            cyclic: HashMap::new(),
            contract_types: BTreeMap::new(),
            py_declarations: Vec::new(),
            py_names: HashSet::new(),
            py_classes: HashSet::new(),
            py_thrown: BTreeMap::new(),
        };
        for (path, typescript) in &self.declared {
            let parsed: syn::Path =
                syn::parse_str(path).map_err(|error| format!("{path}: {error}"))?;
            let res = generator
                .sources
                .resolve(root, &parsed)?
                .ok_or_else(|| format!("declared type {path} is not found"))?;
            let python = self
                .declared_python
                .iter()
                .rev()
                .find(|(declared, _)| declared == path)
                .map(|(_, python)| python.clone());
            generator.declared.insert(
                res,
                Declared {
                    path: path.clone(),
                    typescript: typescript.clone(),
                    python,
                },
            );
        }
        for (path, _) in &self.declared_python {
            if !self.declared.iter().any(|(declared, _)| declared == path) {
                return Err(format!(
                    "declared_python(\"{path}\", ..) needs its TypeScript too: add .declared(\"{path}\", ..)"
                ));
            }
        }
        let mut services_ts = String::new();
        let mut services_contract = serde_json::Map::new();
        let mut names: HashMap<String, String> = HashMap::new();
        for (module, index, ident, name) in generator.marked_types()? {
            if let Some(first) = names.insert(name.clone(), ident.clone()) {
                return Err(format!(
                    "the service name {name} is given twice: {first} and {ident} are both marked #[rutis::service] with it"
                ));
            }
            let (interface, methods) = generator.service(module, index, &ident)?;
            services_ts.push_str(&format!("  {}: {interface}\n", ts_key(&name)));
            services_contract.insert(name, serde_json::json!({ "methods": methods }));
        }
        let mut typescript = String::from(
            "// Generated from the service declarations; do not edit.\n\n\
             /** What a method's `Err(e)` throws: named after `e`'s type, `code` its variant, `message` its `Display`, `data` its serde value. */\n\
             export interface RustError<Name extends string = string, Code extends string = string, Data = unknown> extends Error {\n  readonly name: Name\n  readonly code: Code\n  readonly data: Data\n}\n",
        );
        for declaration in &generator.declarations {
            typescript.push('\n');
            typescript.push_str(declaration);
        }
        for thrown in generator.thrown.values() {
            typescript.push('\n');
            typescript.push_str(thrown);
        }
        typescript.push_str(&format!(
            "\n/** The exported services by name: `ctx.use<Services['name']>('name')`. */\nexport interface Services {{\n{services_ts}}}\n"
        ));
        let mut body = String::new();
        for declaration in generator
            .py_declarations
            .iter()
            .chain(generator.py_thrown.values())
        {
            body.push_str("\n\n");
            body.push_str(declaration);
        }
        let mut imports = String::new();
        for module in ["datetime", "decimal", "typing", "uuid"] {
            if body.contains(&format!("{module}.")) {
                imports.push_str(&format!("import {module}\n"));
            }
        }
        let python = format!(
            "# Generated from the service declarations; do not edit.\n\n\
             from __future__ import annotations\n\n\
             {imports}\n\
             import rutis\n\n\
             #: What a method's `Err(e)` raises: named after e's type, `code` its variant,\n\
             #: `message` its Display, `data` its serde value.\n\
             RustError = rutis.RemoteError\n{body}"
        );
        let contract = serde_json::json!({
            "version": CONTRACT_VERSION,
            "services": services_contract,
            "types": generator.contract_types,
        });
        let mut contract =
            serde_json::to_string_pretty(&contract).expect("a contract description is JSON");
        contract.push('\n');
        let mut warnings = std::mem::take(&mut generator.sources.warnings);
        warnings.extend(generator.warnings);
        Ok(Generated {
            typescript,
            python,
            contract,
            warnings,
        })
    }
}

#[derive(Debug)]
pub(super) struct Generated {
    pub(super) typescript: String,
    /// The Python module.
    pub(super) python: String,
    /// The contract description, as JSON.
    pub(super) contract: String,
    pub(super) warnings: Vec<String>,
}

/// The format version of the contract description, which the testing
/// tools of the same major version read. It is their internal format, not
/// a published one.
const CONTRACT_VERSION: u32 = 1;

// ── Types ───────────────────────────────────────────────────────

/// A type as it crosses: its TypeScript, and its contract description.
#[derive(Clone, Debug)]
enum Ty {
    Unit,
    Bool,
    /// An integer, by its Rust primitive: one of 64 bits or more crosses as
    /// a bigint beyond ±(2^53−1).
    Int(String),
    Float,
    Str,
    /// A string in TypeScript and in the contract, with its Python type
    /// given: a tagged value that crosses as text (`rutis_bridge::DateTime`).
    Text(String),
    Json,
    Option(Box<Ty>),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    Record(Box<Ty>),
    /// A declared struct or enum, by its name, with its type arguments.
    Named(String, Vec<Ty>),
    /// A generic parameter of a declared type.
    Param(String),
    /// A plugin definition.
    Definition,
    /// Its TypeScript and its Python are given: a type the application
    /// declared, or one whose form the contract does not describe.
    Ts(String, String),
}

impl Ty {
    fn ts(&self) -> String {
        match self {
            Ty::Unit => "null".into(),
            Ty::Bool => "boolean".into(),
            Ty::Int(_) if self.big() => "number | bigint".into(),
            Ty::Int(_) | Ty::Float => "number".into(),
            Ty::Str | Ty::Text(_) => "string".into(),
            Ty::Json => "unknown".into(),
            Ty::Option(inner) => format!("{} | null", inner.ts()),
            Ty::Array(inner) => match inner.as_ref() {
                Ty::Option(_) => format!("({})[]", inner.ts()),
                Ty::Int(_) if inner.big() => format!("({})[]", inner.ts()),
                _ => {
                    let text = inner.ts();
                    if matches!(
                        inner.as_ref(),
                        Ty::Ts(..) | Ty::Definition | Ty::Named(..) | Ty::Param(_)
                    ) && text.contains(['|', '&', '='])
                    {
                        format!("({text})[]")
                    } else {
                        format!("{text}[]")
                    }
                }
            },
            Ty::Tuple(items) => format!(
                "[{}]",
                items.iter().map(Ty::ts).collect::<Vec<_>>().join(", ")
            ),
            Ty::Record(value) => format!("Record<string, {}>", value.ts()),
            Ty::Named(name, args) if args.is_empty() => name.clone(),
            Ty::Named(name, args) => format!(
                "{name}<{}>",
                args.iter().map(Ty::ts).collect::<Vec<_>>().join(", ")
            ),
            Ty::Param(name) => name.clone(),
            Ty::Definition => DEFINITION_TS.into(),
            Ty::Ts(text, _) => text.clone(),
        }
    }

    /// Its Python type annotation (Python 3.10), in a module that imports
    /// `typing` and `rutis`.
    fn py(&self) -> String {
        match self {
            Ty::Unit => "None".into(),
            Ty::Bool => "bool".into(),
            Ty::Int(_) => "int".into(),
            Ty::Float => "float".into(),
            Ty::Str => "str".into(),
            Ty::Text(py) => py.clone(),
            Ty::Json | Ty::Param(_) => "typing.Any".into(),
            Ty::Option(inner) => match inner.as_ref() {
                Ty::Unit | Ty::Option(_) => inner.py(),
                _ => format!("{} | None", inner.py()),
            },
            Ty::Array(inner) => format!("list[{}]", inner.py()),
            Ty::Tuple(items) => format!(
                "tuple[{}]",
                items.iter().map(Ty::py).collect::<Vec<_>>().join(", ")
            ),
            Ty::Record(value) => format!("dict[str, {}]", value.py()),
            // Generic declarations are not generic in Python 3.10's
            // TypedDict: their parameters are Any.
            Ty::Named(name, _) => name.clone(),
            Ty::Definition => "rutis.Plugin".into(),
            Ty::Ts(_, py) => py.clone(),
        }
    }

    /// An integer of 64 bits or more.
    fn big(&self) -> bool {
        matches!(self, Ty::Int(name) if matches!(name.as_str(), "u64" | "i64" | "u128" | "i128" | "usize" | "isize"))
    }

    /// As a method's result: `void` for nothing.
    fn returned(&self) -> String {
        match self {
            Ty::Unit => "void".into(),
            other => other.ts(),
        }
    }

    /// Its contract description (§六 of the design): what the testing
    /// tools check an argument against. A type whose form is not certain
    /// is `any`, which every value passes.
    fn contract(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Ty::Unit => json!({ "kind": "null" }),
            Ty::Bool => json!({ "kind": "bool" }),
            Ty::Int(name) => {
                let (min, max) = int_range(name);
                json!({ "kind": "int", "min": min, "max": max })
            }
            Ty::Float => json!({ "kind": "float" }),
            Ty::Str | Ty::Text(_) => json!({ "kind": "string" }),
            Ty::Option(inner) => json!({ "kind": "option", "of": inner.contract() }),
            Ty::Array(inner) => json!({ "kind": "list", "of": inner.contract() }),
            Ty::Tuple(items) => {
                json!({ "kind": "tuple", "items": items.iter().map(Ty::contract).collect::<Vec<_>>() })
            }
            Ty::Record(value) => json!({ "kind": "map", "values": value.contract() }),
            Ty::Named(name, args) if args.is_empty() => json!({ "kind": "ref", "name": name }),
            Ty::Definition => json!({ "kind": "plugin" }),
            Ty::Json | Ty::Named(..) | Ty::Param(_) | Ty::Ts(..) => json!({ "kind": "any" }),
        }
    }
}

/// The range of a Rust integer primitive, as text (a JSON number cannot
/// hold every 64-bit value exactly). `usize` and `isize` are 64 bits.
fn int_range(name: &str) -> (String, String) {
    let (min, max): (i128, u128) = match name {
        "u8" => (0, u8::MAX.into()),
        "u16" => (0, u16::MAX.into()),
        "u32" => (0, u32::MAX.into()),
        "u64" | "usize" => (0, u64::MAX.into()),
        "u128" => (0, u128::MAX),
        "i8" => (i8::MIN.into(), i8::MAX as u128),
        "i16" => (i16::MIN.into(), i16::MAX as u128),
        "i32" => (i32::MIN.into(), i32::MAX as u128),
        "i128" => (i128::MIN, i128::MAX as u128),
        // i64, isize
        _ => (i64::MIN.into(), i64::MAX as u128),
    };
    (min.to_string(), max.to_string())
}

/// Where a type is written: its module, and what its generic parameters
/// stand for.
#[derive(Clone)]
struct Scope {
    module: usize,
    params: Rc<HashMap<String, Binding>>,
}

#[derive(Clone)]
enum Binding {
    /// A type argument, written in its own scope.
    Type(Box<Type>, Scope),
    /// A generic parameter of a declared TypeScript type.
    Ts(String),
}

impl Scope {
    fn of(module: usize) -> Self {
        Self {
            module,
            params: Rc::new(HashMap::new()),
        }
    }
}

/// A type the application declares (`Exports::declared`), by its path.
struct Declared {
    path: String,
    typescript: String,
    python: Option<String>,
}

/// A method of a service, as each output writes it.
struct Method {
    typescript: String,
    contract: serde_json::Value,
    python: String,
}

/// A parameter that takes a function.
struct Callback {
    args: Vec<Ty>,
    ok: Ty,
}

struct Generator<'a> {
    sources: &'a mut Sources,
    declared: HashMap<Res, Declared>,
    /// Whether the Python module is written: a declared type without a
    /// Python type is then a warning.
    python: bool,
    /// The declared types already warned of having no Python type.
    untyped: HashSet<String>,
    /// Declared items and their TypeScript names.
    named: HashMap<(usize, usize), String>,
    names: HashSet<String>,
    declarations: Vec<String>,
    /// `<Name>Thrown` declarations, by error type.
    thrown: BTreeMap<String, String>,
    warnings: Vec<String>,
    /// Whether a type alias refers to itself, by alias.
    cyclic: HashMap<(usize, usize), bool>,
    /// The contract descriptions of the declared types, by name.
    contract_types: BTreeMap<String, serde_json::Value>,
    /// The Python declarations, in order.
    py_declarations: Vec<String>,
    /// The names of the Python module's helper declarations.
    py_names: HashSet<String>,
    /// The `TypedDict` classes of the Python module.
    py_classes: HashSet<String>,
    /// The Python `<Name>Thrown` declarations, by error type.
    py_thrown: BTreeMap<String, String>,
}

/// `rutis::Error`, `rutis::host::ErrorKind` (or 0.16's `rutis::CordisError`),
/// also through `rutis_sdk::rutis`.
fn rutis_error(path: &[String]) -> bool {
    let path = match path.first().map(String::as_str) {
        Some("rutis_sdk") => &path[1..],
        _ => path,
    };
    path_is(
        path,
        &[
            &["rutis", "Error"],
            &["rutis", "host", "ErrorKind"],
            &["rutis", "CordisError"],
        ],
    )
}

fn path_is(path: &[String], candidates: &[&[&str]]) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.iter().eq(path.iter()))
}

fn joined(path: &[String]) -> String {
    path.join("::")
}

impl Generator<'_> {
    /// The type arguments of `segment`, in `scope`.
    fn type_args(segment: &syn::PathSegment) -> Vec<Type> {
        match &segment.arguments {
            PathArguments::AngleBracketed(args) => args
                .args
                .iter()
                .filter_map(|arg| match arg {
                    GenericArgument::Type(ty) => Some(ty.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The parameter `name` of `scope`, if `ty` is one.
    fn binding(scope: &Scope, ty: &Type) -> Option<Binding> {
        let Type::Path(path) = ty else { return None };
        if path.qself.is_some()
            || path.path.leading_colon.is_some()
            || path.path.segments.len() != 1
        {
            return None;
        }
        let segment = &path.path.segments[0];
        if !segment.arguments.is_empty() {
            return None;
        }
        scope.params.get(&segment.ident.to_string()).cloned()
    }

    /// Fails for a type alias that refers to itself, directly or through
    /// other aliases (`type A = B; type B = A;`), which rustc refuses:
    /// expanding it would not end.
    fn expandable(&mut self, module: usize, index: usize) -> Result<()> {
        if !self.alias_cycle(module, index)? {
            return Ok(());
        }
        let item = self.sources.item(module, index).clone();
        let ident = match &item {
            Item::Type(alias) => alias.ident.to_string(),
            _ => String::new(),
        };
        Err(format!(
            "{}: type alias `{ident}` refers to itself",
            self.sources.location(module, &item)
        ))
    }

    fn alias_cycle(&mut self, module: usize, index: usize) -> Result<bool> {
        let start = (module, index);
        if let Some(cyclic) = self.cyclic.get(&start) {
            return Ok(*cyclic);
        }
        let mut seen = HashSet::new();
        let mut todo = vec![start];
        let mut cyclic = false;
        'walk: while let Some((module, index)) = todo.pop() {
            let Item::Type(alias) = self.sources.item(module, index).clone() else {
                continue;
            };
            let mut paths = PathTypes(Vec::new());
            paths.visit_type(&alias.ty);
            for path in paths.0 {
                let Some(Res::Item { module, index }) = self.sources.resolve(module, &path)? else {
                    continue;
                };
                if !matches!(self.sources.item(module, index), Item::Type(_)) {
                    continue;
                }
                if (module, index) == start {
                    cyclic = true;
                    break 'walk;
                }
                if seen.insert((module, index)) {
                    todo.push((module, index));
                }
            }
        }
        self.cyclic.insert(start, cyclic);
        Ok(cyclic)
    }

    /// What a path type names in `scope`, with its last segment.
    fn resolve_type(&mut self, scope: &Scope, path: &syn::Path) -> Result<Option<Res>> {
        self.sources.resolve(scope.module, path)
    }

    fn ty(&mut self, scope: &Scope, ty: &Type) -> Result<Ty> {
        if let Some(binding) = Self::binding(scope, ty) {
            return match binding {
                Binding::Type(ty, scope) => self.ty(&scope, &ty),
                Binding::Ts(name) => Ok(Ty::Param(name)),
            };
        }
        match ty {
            Type::Tuple(tuple) if tuple.elems.is_empty() => Ok(Ty::Unit),
            Type::Tuple(tuple) => Ok(Ty::Tuple(
                tuple
                    .elems
                    .iter()
                    .map(|ty| self.ty(scope, ty))
                    .collect::<Result<_>>()?,
            )),
            Type::Reference(reference) if reference.mutability.is_none() => {
                self.ty(scope, &reference.elem)
            }
            Type::Slice(slice) => Ok(Ty::Array(Box::new(self.ty(scope, &slice.elem)?))),
            Type::Array(array) => Ok(Ty::Array(Box::new(self.ty(scope, &array.elem)?))),
            Type::Paren(inner) => self.ty(scope, &inner.elem),
            Type::Group(inner) => self.ty(scope, &inner.elem),
            Type::Path(path) if path.qself.is_none() => {
                let segment = path.path.segments.last().expect("a path has a segment");
                let res = self
                    .resolve_type(scope, &path.path)?
                    .ok_or_else(|| format!("`{}` is not found", path.to_token_stream()))?;
                if let Some(declared) = self.declared.get(&res) {
                    let py = match &declared.python {
                        Some(py) => py.clone(),
                        None => {
                            if self.python && self.untyped.insert(declared.path.clone()) {
                                self.warnings.push(format!(
                                    "{} has no Python type: declare it with .declared_python (written as Any)",
                                    declared.path
                                ));
                            }
                            "typing.Any".into()
                        }
                    };
                    return Ok(Ty::Ts(declared.typescript.clone(), py));
                }
                let args = Self::type_args(segment);
                self.named_or_known(scope, &res, &args, ty)
            }
            _ => Err(format!(
                "`{}` cannot cross between languages",
                ty.to_token_stream()
            )),
        }
    }

    fn arg(&mut self, scope: &Scope, args: &[Type], index: usize, ty: &Type) -> Result<Ty> {
        let arg = args
            .get(index)
            .ok_or_else(|| format!("`{}` needs its type arguments", ty.to_token_stream()))?;
        self.ty(scope, arg)
    }

    fn named_or_known(&mut self, scope: &Scope, res: &Res, args: &[Type], ty: &Type) -> Result<Ty> {
        match res {
            Res::Primitive(name) => Ok(match name.as_str() {
                "bool" => Ty::Bool,
                "char" | "str" => Ty::Str,
                "f32" | "f64" => Ty::Float,
                other => Ty::Int(other.to_owned()),
            }),
            Res::External(path) => self.known(scope, path, args, ty),
            Res::Item { module, index } => {
                let (module, index) = (*module, *index);
                match self.sources.item(module, index).clone() {
                    Item::Type(alias) => {
                        self.expandable(module, index)?;
                        let params = bind(&alias.generics, args, scope, module)?;
                        let alias_scope = Scope {
                            module,
                            params: Rc::new(params),
                        };
                        self.ty(&alias_scope, &alias.ty)
                    }
                    Item::Struct(_) | Item::Enum(_) => {
                        let name = self.declare(module, index)?;
                        let args = args
                            .iter()
                            .map(|arg| self.ty(scope, arg))
                            .collect::<Result<Vec<_>>>()?;
                        Ok(Ty::Named(name, args))
                    }
                    _ => Err(format!("`{}` is not a data type", ty.to_token_stream())),
                }
            }
            Res::Module(_) => Err(format!("`{}` is a module", ty.to_token_stream())),
        }
    }

    /// Types of crates the generator does not read, by their path.
    fn known(&mut self, scope: &Scope, path: &[String], args: &[Type], ty: &Type) -> Result<Ty> {
        let last = path.last().map(String::as_str).unwrap_or_default();
        let krate = path.first().map(String::as_str).unwrap_or_default();
        let address = krate == "std" && (last.starts_with("Ip") || last.starts_with("SocketAddr"));
        if address
            || path_is(
                path,
                &[
                    &["std", "string", "String"],
                    &["std", "path", "PathBuf"],
                    &["std", "path", "Path"],
                ],
            )
        {
            return Ok(Ty::Str);
        }
        if path_is(path, &[&["std", "option", "Option"]]) {
            return Ok(Ty::Option(Box::new(self.arg(scope, args, 0, ty)?)));
        }
        if path_is(
            path,
            &[
                &["std", "boxed", "Box"],
                &["std", "sync", "Arc"],
                &["std", "rc", "Rc"],
            ],
        ) {
            return self.arg(scope, args, 0, ty);
        }
        if path_is(path, &[&["std", "borrow", "Cow"]]) {
            return self.arg(scope, args, 0, ty);
        }
        if krate == "std"
            && matches!(
                last,
                "Vec" | "VecDeque" | "LinkedList" | "HashSet" | "BTreeSet" | "BinaryHeap"
            )
        {
            return Ok(Ty::Array(Box::new(self.arg(scope, args, 0, ty)?)));
        }
        if krate == "std" && matches!(last, "HashMap" | "BTreeMap") {
            return Ok(Ty::Record(Box::new(self.arg(scope, args, 1, ty)?)));
        }
        if path_is(path, &[&["std", "time", "Duration"]]) {
            return Ok(Ty::Ts(
                "{ secs: number | bigint; nanos: number }".into(),
                "dict[str, int]".into(),
            ));
        }
        if path_is(path, &[&["std", "time", "SystemTime"]]) {
            return Ok(Ty::Ts(
                "{ secs_since_epoch: number | bigint; nanos_since_epoch: number }".into(),
                "dict[str, int]".into(),
            ));
        }
        if is_rutis_definition(path) {
            return Ok(Ty::Definition);
        }
        if krate == "serde_json" && last == "Value" {
            return Ok(Ty::Json);
        }
        if krate == "serde_json" && last == "Map" {
            return Ok(Ty::Record(Box::new(Ty::Json)));
        }
        if krate == "uuid" && last == "Uuid" || krate == "url" && last == "Url" {
            return Ok(Ty::Str);
        }
        if krate == "chrono"
            && matches!(
                last,
                "DateTime" | "NaiveDate" | "NaiveDateTime" | "NaiveTime"
            )
        {
            return Ok(Ty::Str);
        }
        if krate == "rutis_bridge" {
            // The tagged values (#308), as rutis_bridge::session names them.
            return match last {
                "Instant" => Ok(Ty::Ts("Date".into(), "datetime.datetime".into())),
                "BigInt" => Ok(Ty::Ts("bigint".into(), "int".into())),
                "Bytes" => Ok(Ty::Ts("Uint8Array".into(), "bytes".into())),
                "DateTime" => Ok(Ty::Text("datetime.datetime".into())),
                "Date" => Ok(Ty::Text("datetime.date".into())),
                "TimeOfDay" => Ok(Ty::Text("datetime.time".into())),
                "Decimal" => Ok(Ty::Text("decimal.Decimal".into())),
                "Uuid" => Ok(Ty::Text("uuid.UUID".into())),
                "ValueMap" => {
                    let (key, value) =
                        (self.arg(scope, args, 0, ty)?, self.arg(scope, args, 1, ty)?);
                    Ok(Ty::Ts(
                        format!("Map<{}, {}>", key.ts(), value.ts()),
                        format!("dict[{}, {}]", key.py(), value.py()),
                    ))
                }
                "ValueSet" => {
                    let element = self.arg(scope, args, 0, ty)?;
                    Ok(Ty::Ts(
                        format!("Set<{}>", element.ts()),
                        format!("set[{}]", element.py()),
                    ))
                }
                _ => Err(format!("`{}` cannot cross between languages", joined(path))),
            };
        }
        Err(format!(
            "the TypeScript of `{}` is not known: declare it with Exports::declared",
            joined(path)
        ))
    }

    /// The TypeScript name of a struct or enum, declaring it on first use.
    /// A type that cannot be declared leaves nothing behind, nor do the
    /// types first declared for it.
    fn declare(&mut self, module: usize, index: usize) -> Result<String> {
        if let Some(name) = self.named.get(&(module, index)) {
            return Ok(name.clone());
        }
        let snapshot = (
            self.declarations.len(),
            self.named.clone(),
            self.names.clone(),
            self.contract_types.clone(),
            self.py_declarations.len(),
            self.py_names.clone(),
            self.py_classes.clone(),
        );
        let declared = self.declare_new(module, index);
        if declared.is_err() {
            self.declarations.truncate(snapshot.0);
            self.named = snapshot.1;
            self.names = snapshot.2;
            self.contract_types = snapshot.3;
            self.py_declarations.truncate(snapshot.4);
            self.py_names = snapshot.5;
            self.py_classes = snapshot.6;
        }
        declared
    }

    fn declare_new(&mut self, module: usize, index: usize) -> Result<String> {
        let item = self.sources.item(module, index).clone();
        let (ident, attrs, generics) = match &item {
            Item::Struct(item) => (
                item.ident.to_string(),
                item.attrs.clone(),
                item.generics.clone(),
            ),
            Item::Enum(item) => (
                item.ident.to_string(),
                item.attrs.clone(),
                item.generics.clone(),
            ),
            _ => unreachable!("declared only for structs and enums"),
        };
        let name = self.unique(&ident);
        self.named.insert((module, index), name.clone());
        let location = self.sources.location(module, &item);
        if !derives(&attrs, "Serialize") && !derives(&attrs, "Deserialize") {
            return Err(format!(
                "{location}: {ident} derives neither Serialize nor Deserialize: declare its TypeScript with Exports::declared"
            ));
        }
        let serde = SerdeAttrs::of(&attrs).map_err(|error| format!("{location}: {error}"))?;
        let mut params = HashMap::new();
        let mut ts_params = Vec::new();
        for param in &generics.params {
            match param {
                syn::GenericParam::Type(param) => {
                    let param = param.ident.to_string();
                    params.insert(param.clone(), Binding::Ts(param.clone()));
                    ts_params.push(param);
                }
                syn::GenericParam::Lifetime(_) => {}
                syn::GenericParam::Const(_) => {
                    return Err(format!("{location}: {ident} has a const parameter"))
                }
            }
        }
        let scope = Scope {
            module,
            params: Rc::new(params),
        };
        let header = if ts_params.is_empty() {
            name.clone()
        } else {
            format!("{name}<{}>", ts_params.join(", "))
        };
        let (body, enum_py) = match &item {
            Item::Struct(item) => (
                self.struct_body(&scope, &item.fields, &serde, serde.rename_all.as_deref())
                    .map_err(|error| format!("{location}: {ident}: {error}"))?,
                None,
            ),
            Item::Enum(item) => {
                let (body, py) = self
                    .enum_body(&scope, item, &serde, &name)
                    .map_err(|error| format!("{location}: {ident}: {error}"))?;
                (body, Some(py))
            }
            _ => unreachable!(),
        };
        let py_docs = py_doc_lines(&attrs);
        let py_declaration = match enum_py {
            Some(py) => py_alias(&name, &py, &py_docs),
            None => self.py_struct(&name, &body, &py_docs),
        };
        self.py_declarations.push(py_declaration);
        let contract = match &item {
            _ if !ts_params.is_empty() || !derives(&attrs, "Deserialize") || serde.asymmetric => {
                serde_json::json!({ "kind": "any" })
            }
            Item::Struct(_) => body.contract(serde.deny_unknown_fields),
            Item::Enum(item) => self.enum_contract(&scope, item, &serde)?,
            _ => unreachable!(),
        };
        self.contract_types.insert(name.clone(), contract);
        let docs = jsdoc(&attrs, "");
        let declaration = match body {
            Body::Object {
                fields, flattened, ..
            } if flattened.is_empty() => {
                format!(
                    "{docs}export interface {header} {}\n",
                    object(&fields, false)
                )
            }
            body => {
                let expression = body.inline(false);
                let space = if expression.starts_with('\n') {
                    ""
                } else {
                    " "
                };
                format!("{docs}export type {header} ={space}{expression}\n")
            }
        };
        self.declarations.push(declaration);
        Ok(name)
    }

    fn unique(&mut self, ident: &str) -> String {
        let mut name = ident.to_owned();
        let mut n = 2;
        while !self.names.insert(name.clone()) || RESERVED.contains(&name.as_str()) {
            name = format!("{ident}{n}");
            n += 1;
        }
        name
    }

    fn struct_body(
        &mut self,
        scope: &Scope,
        fields: &syn::Fields,
        serde: &SerdeAttrs,
        rename_all: Option<&str>,
    ) -> Result<Body> {
        match fields {
            syn::Fields::Unit => Ok(Body::Alias(Ty::Unit)),
            syn::Fields::Unnamed(fields) => {
                let fields: Vec<_> = fields
                    .unnamed
                    .iter()
                    .filter(|field| self.sources_enabled(scope, &field.attrs))
                    .collect();
                let kept: Vec<_> = fields
                    .iter()
                    .filter(|field| !SerdeAttrs::of(&field.attrs).is_ok_and(|attrs| attrs.skip))
                    .collect();
                if kept.len() == 1 {
                    return Ok(Body::Alias(self.ty(scope, &kept[0].ty)?));
                }
                let items = kept
                    .iter()
                    .map(|field| self.ty(scope, &field.ty))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Body::Alias(Ty::Tuple(items)))
            }
            syn::Fields::Named(named) => {
                if serde.transparent {
                    let field = named
                        .named
                        .iter()
                        .find(|field| !SerdeAttrs::of(&field.attrs).is_ok_and(|attrs| attrs.skip))
                        .ok_or("a transparent struct needs a field")?;
                    return Ok(Body::Alias(self.ty(scope, &field.ty)?));
                }
                let mut fields = Vec::new();
                let mut flattened = Vec::new();
                let mut asymmetric = false;
                for field in &named.named {
                    if !self.sources_enabled(scope, &field.attrs) {
                        continue;
                    }
                    let attrs = SerdeAttrs::of(&field.attrs)?;
                    if attrs.asymmetric {
                        asymmetric = true;
                    }
                    if attrs.skip {
                        continue;
                    }
                    let ident = field.ident.as_ref().expect("a named field").to_string();
                    let ident = ident.strip_prefix("r#").unwrap_or(&ident).to_owned();
                    let ty = self.ty(scope, &field.ty)?;
                    if attrs.flatten {
                        flattened.push(ty);
                        continue;
                    }
                    let key = attrs
                        .rename
                        .clone()
                        .unwrap_or_else(|| rename_field(&ident, rename_all));
                    let optional = matches!(ty, Ty::Option(_))
                        || attrs.default
                        || attrs.skip_if
                        || serde.default;
                    fields.push(Field {
                        docs: doc_lines(&field.attrs),
                        key,
                        optional,
                        ty,
                    });
                }
                Ok(Body::Object {
                    fields,
                    flattened,
                    loose: asymmetric,
                })
            }
        }
    }

    fn sources_enabled(&self, scope: &Scope, attrs: &[syn::Attribute]) -> bool {
        self.sources
            .enabled(self.sources.package_of(scope.module), attrs)
    }

    /// What each variant is as serde writes it, and the codes they give.
    /// Its TypeScript body, and its Python type expression (helper
    /// `TypedDict`s for its variants are declared first).
    fn enum_body(
        &mut self,
        scope: &Scope,
        item: &syn::ItemEnum,
        serde: &SerdeAttrs,
        enum_name: &str,
    ) -> Result<(Body, String)> {
        let mut alternatives = Vec::new();
        let mut py_alternatives = Vec::new();
        for (variant, name) in self.variants(scope, item, serde)? {
            let attrs = SerdeAttrs::of(&variant.attrs)?;
            let rename_all = attrs
                .rename_all
                .as_deref()
                .or(serde.rename_all_fields.as_deref());
            let literal = format!("{name:?}");
            let body = match &variant.fields {
                syn::Fields::Unit => None,
                fields => Some(self.struct_body(scope, fields, &attrs, rename_all)?),
            };
            py_alternatives.push(self.py_variant(
                serde,
                &format!("{enum_name}{}", variant.ident),
                &name,
                body.as_ref(),
                &py_doc_lines(&variant.attrs),
            ));
            // An internally tagged variant's fields sit beside the tag.
            if let (
                Some(tag),
                None,
                false,
                Some(Body::Object {
                    fields, flattened, ..
                }),
            ) = (&serde.tag, &serde.content, serde.untagged, &body)
            {
                let mut fields = fields.clone();
                fields.insert(
                    0,
                    Field {
                        docs: Vec::new(),
                        key: tag.clone(),
                        optional: false,
                        ty: Ty::Ts(literal.clone(), format!("typing.Literal[{literal}]")),
                    },
                );
                let merged = Body::Object {
                    fields,
                    flattened: flattened.clone(),
                    loose: true,
                };
                alternatives.push(format!(
                    "{}{}",
                    inline_doc(&variant.attrs),
                    merged.inline(true)
                ));
                continue;
            }
            let content = body.map(|body| body.inline(true));
            let alternative = match (&serde.tag, &serde.content, serde.untagged, content) {
                (_, _, true, None) => "null".to_owned(),
                (_, _, true, Some(content)) => content,
                (Some(tag), _, _, None) => format!("{{ {}: {literal} }}", ts_key(tag)),
                (Some(tag), Some(key), _, Some(content)) => {
                    format!(
                        "{{ {}: {literal}; {}: {content} }}",
                        ts_key(tag),
                        ts_key(key)
                    )
                }
                (Some(tag), None, _, Some(content)) => {
                    if matches!(variant.fields, syn::Fields::Unnamed(ref fields) if fields.unnamed.len() > 1)
                    {
                        return Err("an internally tagged enum cannot have tuple variants".into());
                    }
                    format!("{{ {}: {literal} }} & {content}", ts_key(tag))
                }
                (None, _, _, None) => literal,
                (None, _, _, Some(content)) => format!("{{ {}: {content} }}", ts_key(&name)),
            };
            alternatives.push(format!("{}{alternative}", inline_doc(&variant.attrs)));
        }
        let py = py_union(&py_alternatives);
        if alternatives.is_empty() {
            return Ok((Body::Alias(Ty::Ts("never".into(), py.clone())), py));
        }
        let long = alternatives.iter().map(String::len).sum::<usize>() > 80
            || alternatives
                .iter()
                .any(|alternative| alternative.starts_with("/**"));
        let ts = if long {
            alternatives
                .iter()
                .map(|alternative| format!("\n  | {alternative}"))
                .collect()
        } else {
            alternatives.join(" | ")
        };
        Ok((Body::Alias(Ty::Ts(ts, py.clone())), py))
    }

    // ── Python ───────────────────────────────────────────────────

    /// A name for a helper declaration of the Python module, `_`-prefixed:
    /// taken by no declaration (TypeScript's names are Python's too).
    fn py_helper(&mut self, base: &str) -> String {
        let base = format!("_{base}");
        let mut name = base.clone();
        let mut n = 2;
        while self.names.contains(&name) || !self.py_names.insert(name.clone()) {
            name = format!("{base}{n}");
            n += 1;
        }
        name
    }

    /// The Python declaration of a struct `name` with `body`.
    fn py_struct(&mut self, name: &str, body: &Body, docs: &[String]) -> String {
        match body {
            Body::Alias(ty) => py_alias(name, &ty.py(), docs),
            Body::Object {
                fields, flattened, ..
            } => {
                // Flattened structs are base classes; anything else
                // flattened cannot be written as a TypedDict.
                let mut bases = Vec::new();
                for ty in flattened {
                    match ty {
                        Ty::Named(base, _) if self.py_classes.contains(base) => {
                            bases.push(base.clone())
                        }
                        _ => return py_alias(name, "dict[str, typing.Any]", docs),
                    }
                }
                let fields: Vec<PyField> = fields.iter().map(PyField::of).collect();
                self.typed_dict(name, &fields, docs, &bases)
            }
        }
    }

    /// The Python type of a variant `name` (serde's) of an enum, declaring
    /// the helper `TypedDict` it needs (`helper` names it).
    fn py_variant(
        &mut self,
        serde: &SerdeAttrs,
        helper: &str,
        name: &str,
        body: Option<&Body>,
        docs: &[String],
    ) -> String {
        let literal = format!("typing.Literal[{name:?}]");
        let tag_field = |tag: &str| PyField {
            key: tag.to_owned(),
            optional: false,
            py: literal.clone(),
            docs: Vec::new(),
        };
        let fields = match (&serde.tag, &serde.content, serde.untagged, body) {
            (_, _, true, None) => return "None".into(),
            (_, _, true, Some(body)) => return self.py_content(helper, body, docs),
            (None, _, _, None) => return literal,
            (Some(tag), _, _, None) => vec![tag_field(tag)],
            (Some(tag), Some(content), _, Some(body)) => {
                let content_py = self.py_content(&format!("{helper}Content"), body, &[]);
                vec![
                    tag_field(tag),
                    PyField {
                        key: content.clone(),
                        optional: false,
                        py: content_py,
                        docs: Vec::new(),
                    },
                ]
            }
            (
                Some(tag),
                None,
                _,
                Some(Body::Object {
                    fields, flattened, ..
                }),
            ) if flattened.is_empty() => std::iter::once(tag_field(tag))
                .chain(fields.iter().map(PyField::of))
                .collect(),
            // A newtype beside a tag: an intersection Python cannot write.
            (Some(_), None, _, Some(_)) => return "typing.Any".into(),
            (None, _, _, Some(body)) => {
                let content_py = self.py_content(&format!("{helper}Content"), body, &[]);
                vec![PyField {
                    key: name.to_owned(),
                    optional: false,
                    py: content_py,
                    docs: Vec::new(),
                }]
            }
        };
        let helper = self.py_helper(helper);
        let declaration = self.typed_dict(&helper, &fields, docs, &[]);
        self.py_declarations.push(declaration);
        helper
    }

    /// The Python type of a variant's content.
    fn py_content(&mut self, helper: &str, body: &Body, docs: &[String]) -> String {
        match body {
            Body::Alias(ty) => ty.py(),
            Body::Object { flattened, .. } if !flattened.is_empty() => {
                "dict[str, typing.Any]".into()
            }
            Body::Object { fields, .. } => {
                let fields: Vec<PyField> = fields.iter().map(PyField::of).collect();
                let helper = self.py_helper(helper);
                let declaration = self.typed_dict(&helper, &fields, docs, &[]);
                self.py_declarations.push(declaration);
                helper
            }
        }
    }

    /// A `TypedDict` `name` with `fields` (the optional ones in a
    /// `total=False` parent, as Python 3.10 has no `NotRequired`), on
    /// `bases`. Keys that are not identifiers take the functional form.
    fn typed_dict(
        &mut self,
        name: &str,
        fields: &[PyField],
        docs: &[String],
        bases: &[String],
    ) -> String {
        self.py_classes.insert(name.to_owned());
        let required: Vec<&PyField> = fields.iter().filter(|field| !field.optional).collect();
        let optional: Vec<&PyField> = fields.iter().filter(|field| field.optional).collect();
        let mut text = String::new();
        let mut parents = Vec::new();
        if fields.iter().all(|field| py_identifier(&field.key)) {
            if !optional.is_empty() {
                let helper = self.py_helper(&format!("{name}Optional"));
                text.push_str(&py_class(
                    &helper,
                    &["typing.TypedDict".into(), "total=False".into()],
                    &[],
                    &optional,
                ));
                text.push_str(
                    "

",
                );
                parents.push(helper);
            }
            parents.extend(bases.iter().cloned());
            if parents.is_empty() {
                parents.push("typing.TypedDict".into());
            }
            text.push_str(&py_class(name, &parents, docs, &required));
        } else {
            if !required.is_empty() || (optional.is_empty() && bases.is_empty()) {
                let helper = self.py_helper(&format!("{name}Required"));
                text.push_str(&py_functional(&helper, &required, true));
                parents.push(helper);
            }
            if !optional.is_empty() {
                let helper = self.py_helper(&format!("{name}Optional"));
                text.push_str(&py_functional(&helper, &optional, false));
                parents.push(helper);
            }
            parents.extend(bases.iter().cloned());
            text.push_str(
                "

",
            );
            text.push_str(&py_class(name, &parents, docs, &[]));
        }
        text
    }

    /// The variants serde writes, each with its name as serde writes it.
    fn variants<'v>(
        &self,
        scope: &Scope,
        item: &'v syn::ItemEnum,
        serde: &SerdeAttrs,
    ) -> Result<Vec<(&'v syn::Variant, String)>> {
        let mut variants = Vec::new();
        for variant in &item.variants {
            if !self.sources_enabled(scope, &variant.attrs) {
                continue;
            }
            let attrs = SerdeAttrs::of(&variant.attrs)?;
            if attrs.skip {
                continue;
            }
            let ident = variant.ident.to_string();
            let name = attrs
                .rename
                .clone()
                .unwrap_or_else(|| rename_variant(&ident, serde.rename_all.as_deref()));
            variants.push((variant, name));
        }
        Ok(variants)
    }

    /// The contract description of an enum: the names of an enum of unit
    /// variants written as their names; any other enum is `any`.
    fn enum_contract(
        &self,
        scope: &Scope,
        item: &syn::ItemEnum,
        serde: &SerdeAttrs,
    ) -> Result<serde_json::Value> {
        let any = serde_json::json!({ "kind": "any" });
        if serde.tag.is_some() || serde.untagged {
            return Ok(any);
        }
        let mut names = Vec::new();
        for (variant, name) in self.variants(scope, item, serde)? {
            if !matches!(variant.fields, syn::Fields::Unit) {
                return Ok(any);
            }
            names.push(name);
        }
        for variant in &item.variants {
            if SerdeAttrs::of(&variant.attrs)?.asymmetric {
                return Ok(any);
            }
        }
        Ok(serde_json::json!({ "kind": "enum", "names": names }))
    }

    // ── Methods ──────────────────────────────────────────────────

    /// The `#[rutis::service]` name of the type at `module`/`index`, when
    /// one of its blocks is marked: the name a block gives, or the derived
    /// one.
    fn marked(&mut self, module: usize, index: usize) -> Result<Option<String>> {
        let ident = match self.sources.item(module, index) {
            Item::Struct(item) => item.ident.to_string(),
            Item::Enum(item) => item.ident.to_string(),
            _ => return Ok(None),
        };
        let mut marked = false;
        let mut named: Option<String> = None;
        for candidate in self.sources.crate_modules(module) {
            let impls: Vec<syn::ItemImpl> = self.sources.modules[candidate]
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Impl(item) if item.trait_.is_none() => Some(item.clone()),
                    _ => None,
                })
                .collect();
            for implementation in impls {
                let Some(options) = marker(&implementation.attrs)? else {
                    continue;
                };
                let Type::Path(self_ty) = implementation.self_ty.as_ref() else {
                    continue;
                };
                if self.sources.resolve(candidate, &self_ty.path)?
                    != Some(Res::Item { module, index })
                {
                    continue;
                }
                marked = true;
                if let Some(name) = options.name {
                    if let Some(known) = named.as_ref().filter(|known| **known != name) {
                        return Err(format!(
                            "two #[rutis::service] blocks of {ident} name it `{known}` and `{name}`"
                        ));
                    }
                    named = Some(name);
                }
            }
        }
        Ok(marked.then(|| named.unwrap_or_else(|| default_name(&ident))))
    }

    /// Every type marked `#[rutis::service]` in the crates that can hold
    /// one ([`Sources::marked_roots`]), with its ident and service name.
    fn marked_types(&mut self) -> Result<Vec<(usize, usize, String, String)>> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        for root in self.sources.marked_roots()? {
            for candidate in self.sources.crate_modules(root) {
                let impls: Vec<syn::ItemImpl> = self.sources.modules[candidate]
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        Item::Impl(item) if item.trait_.is_none() => Some(item.clone()),
                        _ => None,
                    })
                    .collect();
                for implementation in impls {
                    if marker(&implementation.attrs)?.is_none()
                        || !self
                            .sources
                            .enabled(self.sources.package_of(candidate), &implementation.attrs)
                    {
                        continue;
                    }
                    let Type::Path(self_ty) = implementation.self_ty.as_ref() else {
                        continue;
                    };
                    let Some(Res::Item { module, index }) =
                        self.sources.resolve(candidate, &self_ty.path)?
                    else {
                        continue;
                    };
                    if !seen.insert((module, index)) {
                        continue;
                    }
                    let ident = match self.sources.item(module, index) {
                        Item::Struct(item) => item.ident.to_string(),
                        Item::Enum(item) => item.ident.to_string(),
                        _ => continue,
                    };
                    if let Some(name) = self.marked(module, index)? {
                        found.push((module, index, ident, name));
                    }
                }
            }
        }
        Ok(found)
    }

    /// The TypeScript interface of the marked service type at
    /// `module`/`index`: the methods of its `#[rutis::service]` blocks,
    /// without their `#[service(skip)]` ones. Returns the interface's name
    /// and the contract descriptions of its methods.
    fn service(
        &mut self,
        module: usize,
        index: usize,
        ident: &str,
    ) -> Result<(String, serde_json::Value)> {
        let interface = self.unique(ident);
        let item = self.sources.item(module, index).clone();
        let item_attrs = match &item {
            Item::Struct(item) => item.attrs.clone(),
            Item::Enum(item) => item.attrs.clone(),
            _ => Vec::new(),
        };
        let mut methods_ts = String::new();
        let mut methods_contract = serde_json::Map::new();
        let mut methods_py = String::new();
        let mut seen = HashSet::new();
        for candidate in self.sources.crate_modules(module) {
            let impls: Vec<(usize, syn::ItemImpl)> = self.sources.modules[candidate]
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Impl(item) if item.trait_.is_none() => Some((candidate, item.clone())),
                    _ => None,
                })
                .collect();
            for (impl_module, implementation) in impls {
                let Type::Path(self_ty) = implementation.self_ty.as_ref() else {
                    continue;
                };
                if self_ty.qself.is_some() {
                    continue;
                }
                let found = self.sources.resolve(impl_module, &self_ty.path)?;
                if found != Some(Res::Item { module, index }) {
                    continue;
                }
                if !self
                    .sources
                    .enabled(self.sources.package_of(impl_module), &implementation.attrs)
                {
                    continue;
                }
                if marker(&implementation.attrs)?.is_none() {
                    continue;
                }
                for member in &implementation.items {
                    let ImplItem::Fn(method) = member else {
                        continue;
                    };
                    if marker(&method.attrs)?.is_some_and(|options| options.skip) {
                        continue;
                    }
                    if !matches!(method.vis, syn::Visibility::Public(_))
                        || !self
                            .sources
                            .enabled(self.sources.package_of(impl_module), &method.attrs)
                    {
                        continue;
                    }
                    let location = self.sources.location(impl_module, &method.sig);
                    let rust_name = method.sig.ident.to_string();
                    match method.sig.inputs.first() {
                        Some(FnArg::Receiver(receiver)) if shared_receiver(receiver) => {}
                        // Constructors and associated functions are not
                        // the service's methods.
                        None | Some(FnArg::Typed(_)) => continue,
                        Some(FnArg::Receiver(_)) => {
                            self.warnings.push(format!(
                                "{location}: {ident}::{rust_name} is not exported: a shared service's methods take &self"
                            ));
                            continue;
                        }
                    }
                    let js = camel(&rust_name);
                    if !seen.insert(js.clone()) {
                        return Err(format!(
                            "{location}: {ident}: two methods are {js} in TypeScript"
                        ));
                    }
                    let scope = Scope::of(impl_module);
                    match self.method(&scope, method) {
                        Ok(declared) => {
                            methods_ts.push_str(&declared.typescript);
                            if !py_identifier(&js) {
                                if self.python {
                                    self.warnings.push(format!(
                                        "{location}: {ident}::{rust_name} has no Python declaration: {js} is a Python keyword"
                                    ));
                                }
                            } else {
                                if !methods_py.is_empty() {
                                    methods_py.push('\n');
                                }
                                methods_py.push_str(&declared.python);
                            }
                            methods_contract
                                .insert(js, serde_json::json!({ "params": declared.contract }));
                        }
                        Err(reason) => {
                            // The service still has it: callable, with its
                            // arguments not checked.
                            methods_contract
                                .insert(js.clone(), serde_json::json!({ "params": null }));
                            seen.remove(&js);
                            self.warnings.push(format!(
                                "{location}: {ident}::{rust_name} is not exported: {reason}"
                            ));
                        }
                    }
                }
            }
        }
        if seen.is_empty() {
            self.warnings.push(format!(
                "{}: {ident} has no public &self method to export",
                self.sources.location(module, &item)
            ));
        }
        let docs = jsdoc(&item_attrs, "");
        self.declarations.push(format!(
            "{docs}export interface {interface} {{\n{methods_ts}}}\n"
        ));
        let py_docs = py_docstring(&py_doc_lines(&item_attrs), "    ");
        let separator = if py_docs.is_empty() || methods_py.is_empty() {
            ""
        } else {
            "\n"
        };
        let body = if py_docs.is_empty() && methods_py.is_empty() {
            "    pass\n"
        } else {
            &methods_py
        };
        self.py_declarations.push(format!(
            "class {interface}(typing.Protocol):\n{py_docs}{separator}{body}"
        ));
        Ok((interface, serde_json::Value::Object(methods_contract)))
    }

    /// One method's TypeScript and Python signatures and the contract
    /// descriptions of its parameters, or why it cannot be declared.
    fn method(&mut self, scope: &Scope, method: &syn::ImplItemFn) -> Result<Method> {
        let sig = &method.sig;
        if sig.unsafety.is_some() {
            return Err("it is unsafe".into());
        }
        // Generic parameters: lifetimes, and functions it takes.
        let mut bounds: HashMap<String, Vec<TypeParamBound>> = HashMap::new();
        for param in &sig.generics.params {
            match param {
                syn::GenericParam::Lifetime(_) => {}
                syn::GenericParam::Type(param) => {
                    bounds
                        .entry(param.ident.to_string())
                        .or_default()
                        .extend(param.bounds.iter().cloned());
                }
                syn::GenericParam::Const(_) => return Err("it has a const parameter".into()),
            }
        }
        if let Some(clause) = &sig.generics.where_clause {
            for predicate in &clause.predicates {
                if let syn::WherePredicate::Type(predicate) = predicate {
                    if let Some(name) = single_ident(&predicate.bounded_ty) {
                        if let Some(list) = bounds.get_mut(&name) {
                            list.extend(predicate.bounds.iter().cloned());
                            continue;
                        }
                    }
                    return Err("its where clause binds more than its parameters".into());
                }
            }
        }
        let mut params_ts = Vec::new();
        let mut params_contract = Vec::new();
        let mut params_py = Vec::new();
        let mut count = 0;
        for input in sig.inputs.iter().skip(1) {
            let FnArg::Typed(input) = input else {
                unreachable!()
            };
            let name = match input.pat.as_ref() {
                syn::Pat::Ident(ident) => ident.ident.to_string(),
                _ => format!("arg{count}"),
            };
            let name = name.trim_start_matches('_').to_owned();
            let name = if name.is_empty() {
                format!("arg{count}")
            } else {
                name
            };
            let js = camel(name.strip_prefix("r#").unwrap_or(&name));
            let py_name = py_param(name.strip_prefix("r#").unwrap_or(&name));
            // The calling row's context: not a parameter for the caller.
            if is_ctx(self, scope, &input.ty)? {
                if matches!(input.ty.as_ref(), Type::Reference(reference) if reference.mutability.is_some())
                {
                    return Err("it takes a mutable reference".into());
                }
                continue;
            }
            // A plugin definition.
            if is_definition(self, scope, &input.ty)? {
                if matches!(input.ty.as_ref(), Type::Reference(reference) if reference.mutability.is_some())
                {
                    return Err("it takes a mutable reference".into());
                }
                params_ts.push(format!("{js}: {DEFINITION_TS}"));
                params_contract.push(Ty::Definition.contract());
                params_py.push(format!("{py_name}: {}", Ty::Definition.py()));
                count += 1;
                continue;
            }
            if let Some(callback) = self.callback(scope, &input.ty, &bounds)? {
                let args_ts: Vec<String> = callback
                    .args
                    .iter()
                    .enumerate()
                    .map(|(index, ty)| format!("arg{index}: {}", ty.ts()))
                    .collect();
                let ok = callback.ok.returned();
                params_ts.push(format!(
                    "{js}: ({}) => {ok} | Promise<{ok}>",
                    args_ts.join(", ")
                ));
                let ok_py = callback.ok.py();
                params_py.push(format!(
                    "{py_name}: typing.Callable[[{}], {ok_py} | typing.Awaitable[{ok_py}]]",
                    callback
                        .args
                        .iter()
                        .map(Ty::py)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                params_contract.push(serde_json::json!({
                    "kind": "function",
                    "params": callback.args.iter().map(Ty::contract).collect::<Vec<_>>(),
                    "result": callback.ok.contract(),
                }));
                count += 1;
                continue;
            }
            if let Some(name) = single_ident(&input.ty).filter(|name| bounds.contains_key(name)) {
                return Err(format!("its parameter {name} is generic"));
            }
            let ty = self.ty(scope, &input.ty)?;
            params_ts.push(format!("{js}: {}", ty.ts()));
            params_contract.push(ty.contract());
            params_py.push(format!("{py_name}: {}", ty.py()));
            count += 1;
        }
        // Trailing optional parameters may be left out.
        let mut optional = params_ts.len();
        while optional > 0
            && params_ts[optional - 1].ends_with(" | null")
            && !params_ts[optional - 1].contains(") =>")
        {
            optional -= 1;
        }
        for param in &mut params_ts[optional..] {
            if let Some((name, rest)) = param.split_once(": ") {
                *param = format!("{name}?: {rest}");
            }
        }
        for param in &mut params_py[optional..] {
            param.push_str(" = None");
        }
        let output = match &sig.output {
            ReturnType::Default => None,
            ReturnType::Type(_, ty) => Some(ty.as_ref().clone()),
        };
        let output = match output {
            None => None,
            Some(ty) => match self.future_output(scope, &ty)? {
                Some(inner) => Some(inner),
                None => Some(ty),
            },
        };
        let (ok, error_ts) = match &output {
            None => (Ty::Unit, None),
            Some(ty) => match self.result(scope, ty)? {
                Some((ok, error, error_scope)) => {
                    let ok = self.ty(&error_scope, &ok)?;
                    let thrown = self.thrown(&error_scope, &error)?;
                    (ok, thrown)
                }
                None => (self.ty(scope, ty)?, None),
            },
        };
        let mut py_docs = py_doc_lines(&method.attrs);
        let mut docs = doc_lines(&method.attrs);
        if let Some(thrown) = &error_ts {
            docs.push(format!("@throws {{{thrown}}}"));
            if !py_docs.is_empty() {
                py_docs.push(String::new());
            }
            py_docs.push(format!("Raises {thrown}."));
        }
        let docs = render_jsdoc(&docs, "  ");
        let js = camel(&sig.ident.to_string());
        let py = std::iter::once("self".to_owned())
            .chain(params_py)
            .collect::<Vec<_>>()
            .join(", ");
        let py = format!(
            "    async def {js}({py}) -> {}:\n{}        ...\n",
            ok.py(),
            py_docstring(&py_docs, "        ")
        );
        Ok(Method {
            typescript: format!(
                "{docs}  {js}({}): Promise<{}>\n",
                params_ts.join(", "),
                ok.returned()
            ),
            contract: serde_json::Value::Array(params_contract),
            python: py,
        })
    }

    /// `X` of a future type: `BoxFuture<'_, X>`, `Pin<Box<dyn Future<Output
    /// = X> ...>>` or `impl Future<Output = X>`.
    fn future_output(&mut self, scope: &Scope, ty: &Type) -> Result<Option<Type>> {
        match ty {
            Type::ImplTrait(implemented) => Ok(future_bound(&implemented.bounds)),
            Type::Path(path) if path.qself.is_none() => {
                let segment = path.path.segments.last().expect("a segment");
                match self.resolve_type(scope, &path.path)? {
                    Some(Res::External(found))
                        if found.last().is_some_and(|last| last == "BoxFuture") =>
                    {
                        Ok(Self::type_args(segment).first().cloned())
                    }
                    Some(Res::External(found)) if path_is(&found, &[&["std", "pin", "Pin"]]) => {
                        let Some(Type::Path(boxed)) = Self::type_args(segment).first().cloned()
                        else {
                            return Ok(None);
                        };
                        let Some(inner) = boxed.path.segments.last() else {
                            return Ok(None);
                        };
                        match Self::type_args(inner).first() {
                            Some(Type::TraitObject(object)) => Ok(future_bound(&object.bounds)),
                            _ => Ok(None),
                        }
                    }
                    Some(Res::Item { module, index }) => {
                        match self.sources.item(module, index).clone() {
                            // `type Fut<'a, T> = BoxFuture<'a, T>`, say.
                            Item::Type(alias) => {
                                self.expandable(module, index)?;
                                let params = bind(
                                    &alias.generics,
                                    &Self::type_args(segment),
                                    scope,
                                    module,
                                )?;
                                let alias_scope = Scope {
                                    module,
                                    params: Rc::new(params),
                                };
                                let inner = self.future_output(&alias_scope, &alias.ty)?;
                                Ok(inner.map(|inner| substitute(&inner, &alias_scope)))
                            }
                            _ => Ok(None),
                        }
                    }
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    /// `(T, E)` of a `Result<T, E>` (or an alias of one), with the scope
    /// `T` and `E` are written in.
    fn result(&mut self, scope: &Scope, ty: &Type) -> Result<Option<(Type, Type, Scope)>> {
        let Type::Path(path) = ty else {
            return Ok(None);
        };
        if path.qself.is_some() {
            return Ok(None);
        }
        if let Some(Binding::Type(ty, scope)) = Self::binding(scope, ty) {
            return self.result(&scope, &ty);
        }
        let segment = path.path.segments.last().expect("a segment");
        let args = Self::type_args(segment);
        match self.resolve_type(scope, &path.path)? {
            Some(Res::External(found)) if path_is(&found, &[&["std", "result", "Result"]]) => {
                match args.as_slice() {
                    [ok, error] => Ok(Some((ok.clone(), error.clone(), scope.clone()))),
                    _ => Ok(None),
                }
            }
            Some(Res::External(found)) if path_is(&found, &[&["anyhow", "Result"]]) => {
                let error: Type = syn::parse_quote!(::anyhow::Error);
                match args.as_slice() {
                    [ok] => Ok(Some((ok.clone(), error, scope.clone()))),
                    [ok, error] => Ok(Some((ok.clone(), error.clone(), scope.clone()))),
                    _ => Ok(None),
                }
            }
            // `rutis::Result<T = (), E = rutis::Error>`
            Some(Res::External(found))
                if path_is(
                    &found,
                    &[&["rutis", "Result"], &["rutis_sdk", "rutis", "Result"]],
                ) =>
            {
                let unit: Type = syn::parse_quote!(());
                let error: Type = syn::parse_quote!(::rutis::Error);
                match args.as_slice() {
                    [] => Ok(Some((unit, error, scope.clone()))),
                    [ok] => Ok(Some((ok.clone(), error, scope.clone()))),
                    [ok, error] => Ok(Some((ok.clone(), error.clone(), scope.clone()))),
                    _ => Ok(None),
                }
            }
            Some(Res::External(found)) if path_is(&found, &[&["std", "io", "Result"]]) => {
                let error: Type = syn::parse_quote!(::std::io::Error);
                Ok(args.first().map(|ok| (ok.clone(), error, scope.clone())))
            }
            Some(Res::Item { module, index }) => match self.sources.item(module, index).clone() {
                Item::Type(alias) => {
                    self.expandable(module, index)?;
                    let params = bind(&alias.generics, &args, scope, module)?;
                    let alias_scope = Scope {
                        module,
                        params: Rc::new(params),
                    };
                    match self.result(&alias_scope, &alias.ty)? {
                        // The parameters stay bound to the scope they were
                        // written in: `ty` and `failure` follow them there,
                        // so a `Thing` only in scope at the use site is
                        // found.
                        Some((ok, error, inner)) => Ok(Some((ok, error, inner))),
                        None => Ok(None),
                    }
                }
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    /// The `<Name>Thrown` TypeScript type of what a method returning
    /// `Err(e)` of type `error` throws: none for an error that is not a
    /// path, or for `rutis_bridge::session::Error` (thrown as it is).
    fn thrown(&mut self, scope: &Scope, error: &Type) -> Result<Option<String>> {
        if let Some(Binding::Type(ty, scope)) = Self::binding(scope, error) {
            return self.thrown(&scope, &ty);
        }
        let Type::Path(path) = error else {
            return Ok(None);
        };
        let segment = path.path.segments.last().expect("a segment");
        let res = self.resolve_type(scope, &path.path)?;
        // rutis's error keeps the name it had in 0.16 on the TypeScript side
        // too (`CordisErrorThrown`)
        let ident = match &res {
            Some(Res::External(found)) if rutis_error(found) => "CordisError".to_owned(),
            _ => segment.ident.to_string(),
        };
        // `Err(message)`: named and coded `Error`, declared as `StringThrown`.
        let string = matches!(&res, Some(Res::External(found)) if path_is(found, &[&["std", "string", "String"]]));
        // The thrown error's name, its codes and its data.
        let (name, codes, data) = match &res {
            _ if string => (
                "Error".to_owned(),
                "\"Error\"".to_owned(),
                "undefined".to_owned(),
            ),
            Some(Res::External(found))
                if found.first().is_some_and(|k| k == "rutis_bridge")
                    && found.last().is_some_and(|l| l == "Error") =>
            {
                return Ok(None);
            }
            // rutis's error (0.16's `CordisError`): thrown as `CordisError`,
            // its code the kind's variant, as before
            Some(Res::External(found)) if rutis_error(found) => {
                (ident.clone(), "string".to_owned(), "undefined".to_owned())
            }
            Some(Res::Item { module, index }) => {
                let (module, index) = (*module, *index);
                match self.sources.item(module, index).clone() {
                    Item::Enum(item) => {
                        let serde = SerdeAttrs::of(&item.attrs)?;
                        let serialized = derives(&item.attrs, "Serialize");
                        // Codes as serde names the variants, or else their
                        // Rust names.
                        let named = serialized && !serde.untagged;
                        let variant_scope = Scope::of(module);
                        let codes: Vec<String> = if named {
                            self.variants(&variant_scope, &item, &serde)?
                                .into_iter()
                                .map(|(_, name)| format!("{name:?}"))
                                .collect()
                        } else {
                            item.variants
                                .iter()
                                .filter(|variant| {
                                    self.sources_enabled(&variant_scope, &variant.attrs)
                                })
                                .map(|variant| format!("{:?}", variant.ident.to_string()))
                                .collect()
                        };
                        let codes = if codes.is_empty() {
                            "never".to_owned()
                        } else {
                            codes.join(" | ")
                        };
                        let data = if serialized {
                            self.ty(scope, error)?.ts()
                        } else {
                            "undefined".to_owned()
                        };
                        (ident.clone(), codes, data)
                    }
                    Item::Struct(item) if derives(&item.attrs, "Serialize") => (
                        ident.clone(),
                        "string".to_owned(),
                        self.ty(scope, error)?.ts(),
                    ),
                    Item::Type(alias) => {
                        self.expandable(module, index)?;
                        let params =
                            bind(&alias.generics, &Self::type_args(segment), scope, module)?;
                        let alias_scope = Scope {
                            module,
                            params: Rc::new(params),
                        };
                        return self.thrown(&alias_scope, &alias.ty);
                    }
                    _ => (ident.clone(), format!("{ident:?}"), "undefined".to_owned()),
                }
            }
            _ => (ident.clone(), format!("{ident:?}"), "undefined".to_owned()),
        };
        let thrown_name = format!("{}Thrown", if string { "String" } else { &ident });
        let declaration = format!(
            "/** What a method returning `Err({ident})` throws. */\nexport type {thrown_name} = RustError<{name:?}, {codes}, {data}>\n"
        );
        match self.thrown.get(&thrown_name) {
            Some(known) if known != &declaration => {
                return Err(format!("two error types are named {ident}"));
            }
            _ => {
                self.thrown.insert(thrown_name.clone(), declaration);
            }
        }
        let codes_py = match codes.as_str() {
            "string" => "str".to_owned(),
            "never" => "typing.NoReturn".to_owned(),
            codes => format!("typing.Literal[{}]", codes.replace(" | ", ", ")),
        };
        self.py_thrown.insert(
            thrown_name.clone(),
            format!(
                "#: What a method returning `Err({ident})` raises: a RustError named {name:?}, its\n\
                 #: `code` one of {thrown_name}Code.\n\
                 {thrown_name} = RustError\n\
                 {thrown_name}Code: typing.TypeAlias = {}\n",
                py_quoted(&codes_py)
            ),
        );
        Ok(Some(thrown_name))
    }

    /// The function a parameter of type `ty` takes, if it takes one.
    fn callback(
        &mut self,
        scope: &Scope,
        ty: &Type,
        bounds: &HashMap<String, Vec<TypeParamBound>>,
    ) -> Result<Option<Callback>> {
        if let Some(name) = single_ident(ty) {
            if let Some(list) = bounds.get(&name) {
                return match fn_bound(list) {
                    Some(bound) => self.callback_signature(scope, bound, bounds).map(Some),
                    None => Ok(None),
                };
            }
        }
        match ty {
            Type::ImplTrait(implemented) => {
                match fn_bound(&implemented.bounds.iter().cloned().collect::<Vec<_>>()) {
                    Some(bound) => self.callback_signature(scope, bound, bounds).map(Some),
                    None => Ok(None),
                }
            }
            Type::Path(path) if path.qself.is_none() => {
                let segment = path.path.segments.last().expect("a segment");
                match self.resolve_type(scope, &path.path)? {
                    Some(Res::External(found))
                        if path_is(
                            &found,
                            &[&["std", "sync", "Arc"], &["std", "boxed", "Box"]],
                        ) =>
                    {
                        let Some(Type::TraitObject(object)) =
                            Self::type_args(segment).first().cloned()
                        else {
                            return Ok(None);
                        };
                        let list: Vec<TypeParamBound> = object.bounds.iter().cloned().collect();
                        let Some(bound) = fn_bound(&list) else {
                            return Ok(None);
                        };
                        let marker = |name: &str| {
                            list.iter().any(|bound| matches!(bound, TypeParamBound::Trait(t) if t.path.segments.last().is_some_and(|s| s.ident == name)))
                        };
                        let arc = found.last().is_some_and(|last| last == "Arc");
                        if !marker("Send") || (arc && !marker("Sync")) {
                            return Err(format!(
                                "the function it takes must be `{}<dyn Fn(..) -> BoxFuture<'static, Result<..>> + Send{}>`",
                                if arc { "Arc" } else { "Box" },
                                if arc { " + Sync" } else { " (+ Sync)" }
                            ));
                        }
                        let signature = self.callback_signature(scope, bound, bounds)?;
                        Ok(Some(signature))
                    }
                    Some(Res::Item { module, index }) => {
                        match self.sources.item(module, index).clone() {
                            Item::Type(alias) => {
                                self.expandable(module, index)?;
                                let params = bind(
                                    &alias.generics,
                                    &Self::type_args(segment),
                                    scope,
                                    module,
                                )?;
                                let alias_scope = Scope {
                                    module,
                                    params: Rc::new(params),
                                };
                                self.callback(&alias_scope, &alias.ty, &HashMap::new())
                            }
                            _ => Ok(None),
                        }
                    }
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    fn callback_signature(
        &mut self,
        scope: &Scope,
        bound: &syn::TraitBound,
        bounds: &HashMap<String, Vec<TypeParamBound>>,
    ) -> Result<Callback> {
        let segment = bound.path.segments.last().expect("a segment");
        let PathArguments::Parenthesized(arguments) = &segment.arguments else {
            return Err("its function parameter has no signature".into());
        };
        let mut args = Vec::new();
        for input in &arguments.inputs {
            if matches!(input, Type::Reference(_)) {
                return Err(
                    "the function it takes borrows its arguments: take them by value".into(),
                );
            }
            args.push(self.ty(scope, input)?);
        }
        if args.len() > 4 {
            return Err("the function it takes has more than four arguments".into());
        }
        let ReturnType::Type(_, output) = &arguments.output else {
            return Err("the function it takes must return a future of a Result, for its failure to reach the caller".into());
        };
        // A generic `Fut: Future<Output = ...>`, or a future type.
        let inner = match single_ident(output).and_then(|name| bounds.get(&name)) {
            Some(list) => future_bound(list.iter()),
            None => self.future_output(scope, output)?,
        };
        let inner = inner.ok_or("the function it takes must return a future of a Result (BoxFuture<'static, Result<..>>), for its failure to reach the caller")?;
        let (ok, _, ok_scope) = self
            .result(scope, &inner)?
            .ok_or("the function it takes must return a future of a Result, for its failure to reach the caller")?;
        let ok = self.ty(&ok_scope, &ok)?;
        Ok(Callback { args, ok })
    }
}

enum Body {
    /// The fields of an object, and the types flattened into it; `loose`
    /// when a field is read under another name than it is written.
    Object {
        fields: Vec<Field>,
        flattened: Vec<Ty>,
        loose: bool,
    },
    Alias(Ty),
}

#[derive(Clone)]
struct Field {
    docs: Vec<String>,
    /// As serde writes it.
    key: String,
    optional: bool,
    ty: Ty,
}

impl Body {
    /// As a type expression: on one line inside a union (`compact`).
    fn inline(&self, compact: bool) -> String {
        match self {
            Body::Alias(alias) => alias.ts(),
            Body::Object {
                fields, flattened, ..
            } => std::iter::once(object(fields, compact))
                .chain(flattened.iter().map(Ty::ts))
                .collect::<Vec<_>>()
                .join(" & "),
        }
    }

    /// The contract description of a struct of this body: its fields
    /// as serde reads them, with no others when `deny_unknown_fields`.
    fn contract(&self, deny_unknown_fields: bool) -> serde_json::Value {
        match self {
            Body::Alias(alias) => alias.contract(),
            Body::Object {
                fields,
                flattened,
                loose,
            } => {
                if *loose || !flattened.is_empty() {
                    return serde_json::json!({ "kind": "any" });
                }
                let fields: Vec<serde_json::Value> = fields
                    .iter()
                    .map(|field| {
                        serde_json::json!({
                            "name": field.key,
                            "type": field.ty.contract(),
                            "optional": field.optional,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "kind": "struct",
                    "fields": fields,
                    "denyUnknown": deny_unknown_fields,
                })
            }
        }
    }
}

/// An object type: one field a line with its docs, or on one line.
fn object(fields: &[Field], compact: bool) -> String {
    let field = |field: &Field| {
        format!(
            "{}{}: {}",
            ts_key(&field.key),
            if field.optional { "?" } else { "" },
            field.ty.ts()
        )
    };
    if compact {
        if fields.is_empty() {
            return "{}".into();
        }
        return format!(
            "{{ {} }}",
            fields.iter().map(field).collect::<Vec<_>>().join("; ")
        );
    }
    let mut text = String::from("{\n");
    for each in fields {
        text.push_str(&render_jsdoc(&each.docs, "  "));
        text.push_str(&format!("  {}\n", field(each)));
    }
    text.push('}');
    text
}

// ── Python ──────────────────────────────────────────────────────

/// A key of a Python `TypedDict`.
struct PyField {
    key: String,
    optional: bool,
    py: String,
    docs: Vec<String>,
}

impl PyField {
    fn of(field: &Field) -> Self {
        Self {
            key: field.key.clone(),
            optional: field.optional,
            py: field.ty.py(),
            docs: field
                .docs
                .iter()
                .map(|line| line.replace("*\\/", "*/"))
                .collect(),
        }
    }
}

/// Python's keywords, which a name may not be.
const PY_KEYWORDS: [&str; 35] = [
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

/// A Python identifier that is not a keyword.
fn py_identifier(name: &str) -> bool {
    identifier(name, "") && !PY_KEYWORDS.contains(&name)
}

/// A letter or `_` (or one of `extra`), then letters, digits and those: the
/// identifier rule TypeScript (with `$`) and Python share.
fn identifier(name: &str, extra: &str) -> bool {
    let first = |c: char| c.is_alphabetic() || c == '_' || extra.contains(c);
    name.chars().next().is_some_and(first) && name.chars().all(|c| first(c) || c.is_alphanumeric())
}

/// A Python parameter name for the Rust one.
fn py_param(name: &str) -> String {
    if PY_KEYWORDS.contains(&name) || name == "self" {
        format!("{name}_")
    } else {
        name.to_owned()
    }
}

/// The doc lines of `attrs`, as Python writes them.
fn py_doc_lines(attrs: &[syn::Attribute]) -> Vec<String> {
    doc_lines(attrs)
        .into_iter()
        .map(|line| line.replace("*\\/", "*/"))
        .collect()
}

/// A docstring of `lines` at `indent`, or nothing.
fn py_docstring(lines: &[String], indent: &str) -> String {
    let lines: Vec<String> = lines
        .iter()
        .map(|line| line.replace('\\', "\\\\").replace("\"\"\"", "\\\"\"\""))
        .collect();
    match lines.as_slice() {
        [] => String::new(),
        [line] => format!("{indent}\"\"\"{line}\"\"\"\n"),
        lines => {
            let mut text = format!("{indent}\"\"\"{}\n", lines[0]);
            for line in &lines[1..] {
                if line.is_empty() {
                    text.push('\n');
                } else {
                    text.push_str(&format!("{indent}{line}\n"));
                }
            }
            text.push_str(&format!("{indent}\"\"\"\n"));
            text
        }
    }
}

/// `#:` comment lines of `lines`, before a module-level name.
fn py_comment(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                "#:\n".to_owned()
            } else {
                format!("#: {line}\n")
            }
        })
        .collect()
}

/// `text` as a Python string literal (a forward reference).
fn py_quoted(text: &str) -> String {
    if !text.contains('"') {
        format!("\"{text}\"")
    } else if !text.contains('\'') {
        format!("'{text}'")
    } else {
        format!("{text:?}")
    }
}

/// `name` as a type alias of `py`, quoted so that it may name what is
/// declared after it.
fn py_alias(name: &str, py: &str, docs: &[String]) -> String {
    format!(
        "{}{name}: typing.TypeAlias = {}\n",
        py_comment(docs),
        py_quoted(py)
    )
}

/// The union of `alternatives`: one `Literal` for names only.
fn py_union(alternatives: &[String]) -> String {
    if alternatives.is_empty() {
        return "typing.NoReturn".into();
    }
    let literals: Option<Vec<&str>> = alternatives
        .iter()
        .map(|alternative| {
            alternative
                .strip_prefix("typing.Literal[")
                .and_then(|rest| rest.strip_suffix(']'))
        })
        .collect();
    match literals {
        Some(names) => format!("typing.Literal[{}]", names.join(", ")),
        None => alternatives.join(" | "),
    }
}

/// A class `name(parents)` with `docs` and the annotated `fields`.
fn py_class(name: &str, parents: &[String], docs: &[String], fields: &[&PyField]) -> String {
    let mut text = format!("class {name}({}):\n", parents.join(", "));
    text.push_str(&py_docstring(docs, "    "));
    if !docs.is_empty() && !fields.is_empty() {
        text.push('\n');
    }
    for field in fields {
        text.push_str(&format!("    {}: {}\n", field.key, field.py));
        text.push_str(&py_docstring(&field.docs, "    "));
    }
    if docs.is_empty() && fields.is_empty() {
        text.push_str("    pass\n");
    }
    text
}

/// A `TypedDict` in the functional form, for keys that are not
/// identifiers; its types are forward references.
fn py_functional(name: &str, fields: &[&PyField], total: bool) -> String {
    let keys: Vec<String> = fields
        .iter()
        .map(|field| format!("{:?}: {}", field.key, py_quoted(&field.py)))
        .collect();
    let total = if total { "" } else { ", total=False" };
    format!(
        "{name} = typing.TypedDict({name:?}, {{{}}}{total})\n",
        keys.join(", ")
    )
}

/// The TypeScript of a plugin definition: a `definePlugin` result or a
/// Cordis plugin.
const DEFINITION_TS: &str = "import('@arcships/rutis').Plugin | ((ctx: any, config: any) => unknown) | { apply(ctx: any, config: any): unknown }";

/// `rutis::Definition` (as `rutis_sdk::rutis` re-exports it too), or a
/// reference to one.
fn is_definition(generator: &mut Generator<'_>, scope: &Scope, ty: &Type) -> Result<bool> {
    let ty = match ty {
        Type::Reference(reference) => reference.elem.as_ref(),
        ty => ty,
    };
    let Type::Path(path) = ty else {
        return Ok(false);
    };
    Ok(matches!(
        generator.sources.resolve(scope.module, &path.path)?,
        Some(Res::External(found)) if is_rutis_definition(&found)
    ))
}

fn is_rutis_definition(path: &[String]) -> bool {
    match path {
        [krate, last] => krate == "rutis" && last == "Definition",
        [sdk, rutis, last] => sdk == "rutis_sdk" && rutis == "rutis" && last == "Definition",
        _ => false,
    }
}

/// Whether `ty` is `rutis::Ctx` (or a reference to it).
fn is_ctx(generator: &mut Generator<'_>, scope: &Scope, ty: &Type) -> Result<bool> {
    let ty = match ty {
        Type::Reference(reference) => reference.elem.as_ref(),
        ty => ty,
    };
    let Type::Path(path) = ty else {
        return Ok(false);
    };
    Ok(matches!(
        generator.sources.resolve(scope.module, &path.path)?,
        Some(Res::External(found)) if found.first().is_some_and(|k| k == "rutis") && found.last().is_some_and(|l| l == "Ctx")
    ))
}

/// The paths a type is written with, its arguments' included.
struct PathTypes(Vec<syn::Path>);

impl<'ast> Visit<'ast> for PathTypes {
    fn visit_type_path(&mut self, ty: &'ast syn::TypePath) {
        if ty.qself.is_none() {
            self.0.push(ty.path.clone());
        }
        syn::visit::visit_type_path(self, ty);
    }
}

/// A type's generic parameters bound to `args`, written in `scope`. A
/// default is written where the parameters are, in `module`, with the
/// parameters before it bound: `type Res<T, E = ErrorOf<T>>`.
fn bind(
    generics: &syn::Generics,
    args: &[Type],
    scope: &Scope,
    module: usize,
) -> Result<HashMap<String, Binding>> {
    let mut params = HashMap::new();
    let mut args = args.iter();
    for param in &generics.params {
        if let syn::GenericParam::Type(param) = param {
            let binding = match (args.next(), &param.default) {
                (Some(arg), _) => Binding::Type(Box::new(arg.clone()), scope.clone()),
                (None, Some(default)) => Binding::Type(
                    Box::new(default.clone()),
                    Scope {
                        module,
                        params: Rc::new(params.clone()),
                    },
                ),
                (None, None) => return Err(format!("{} needs its type arguments", param.ident)),
            };
            params.insert(param.ident.to_string(), binding);
        }
    }
    Ok(params)
}

/// `ty` with the parameters of `scope` that are plain names left as they
/// are: resolution goes through the scope, so this only unwraps a bare
/// parameter.
fn substitute(ty: &Type, scope: &Scope) -> Type {
    match Generator::binding(scope, ty) {
        Some(Binding::Type(ty, _)) => *ty,
        _ => ty.clone(),
    }
}

fn single_ident(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else { return None };
    if path.qself.is_some() || path.path.leading_colon.is_some() || path.path.segments.len() != 1 {
        return None;
    }
    let segment = &path.path.segments[0];
    segment
        .arguments
        .is_empty()
        .then(|| segment.ident.to_string())
}

/// A shared receiver: `&self`, or `self: &Self` / `self: &'a Self`.
fn shared_receiver(receiver: &syn::Receiver) -> bool {
    receiver.mutability.is_none()
        && if receiver.colon_token.is_some() {
            matches!(receiver.ty.as_ref(), Type::Reference(reference)
                if reference.mutability.is_none()
                    && matches!(reference.elem.as_ref(), Type::Path(path)
                        if path.qself.is_none() && path.path.is_ident("Self")))
        } else {
            receiver.reference.is_some()
        }
}

fn fn_bound(bounds: &[TypeParamBound]) -> Option<&syn::TraitBound> {
    bounds.iter().find_map(|bound| match bound {
        TypeParamBound::Trait(bound)
            if bound
                .path
                .segments
                .last()
                .is_some_and(|s| s.ident == "Fn" || s.ident == "FnMut" || s.ident == "FnOnce") =>
        {
            Some(bound)
        }
        _ => None,
    })
}

/// `X` of a `Future<Output = X>` bound.
fn future_bound<'a>(bounds: impl IntoIterator<Item = &'a TypeParamBound>) -> Option<Type> {
    bounds.into_iter().find_map(|bound| {
        let TypeParamBound::Trait(bound) = bound else {
            return None;
        };
        let segment = bound.path.segments.last()?;
        if segment.ident != "Future" {
            return None;
        }
        let PathArguments::AngleBracketed(args) = &segment.arguments else {
            return None;
        };
        args.args.iter().find_map(|arg| match arg {
            GenericArgument::AssocType(assoc) if assoc.ident == "Output" => Some(assoc.ty.clone()),
            _ => None,
        })
    })
}

/// What a `#[rutis::service]` (or `#[service]`) attribute among `attrs`
/// says, when there is one.
struct Marker {
    name: Option<String>,
    skip: bool,
}

fn marker(attrs: &[syn::Attribute]) -> Result<Option<Marker>> {
    let Some(attr) = attrs.iter().find(|attr| {
        let path = attr.path();
        path.segments
            .last()
            .is_some_and(|segment| segment.ident == "service")
            && (path.segments.len() == 1
                || path.segments.iter().any(|segment| segment.ident == "rutis"))
    }) else {
        return Ok(None);
    };
    let mut marker = Marker {
        name: None,
        skip: false,
    };
    if matches!(attr.meta, syn::Meta::List(_)) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let name: syn::LitStr = meta.value()?.parse()?;
                marker.name = Some(name.value());
            } else if meta.path.is_ident("skip") {
                marker.skip = true;
            } else if meta.path.is_ident("crate") {
                let _: syn::LitStr = meta.value()?.parse()?;
            }
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    }
    Ok(Some(marker))
}

/// The service name `#[rutis::service]` gives a type by default: its name
/// with the first word lowercased (`Calendar` is `calendar`, `HTTPClient`
/// `httpClient`), as the macro does.
fn default_name(ident: &str) -> String {
    let chars: Vec<char> = ident.chars().collect();
    let upper = chars.iter().take_while(|c| c.is_uppercase()).count();
    let lower = if upper > 1 && upper < chars.len() && chars[upper].is_lowercase() {
        upper - 1
    } else {
        upper.max(1).min(chars.len())
    };
    chars
        .iter()
        .enumerate()
        .flat_map(|(index, c)| {
            if index < lower {
                c.to_lowercase().collect::<Vec<_>>()
            } else {
                vec![*c]
            }
        })
        .collect()
}

fn derives(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("derive"))
        .any(|attr| {
            attr.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            )
            .is_ok_and(|paths| {
                paths
                    .iter()
                    .any(|path| path.segments.last().is_some_and(|s| s.ident == name))
            })
        })
}

/// The `#[serde(...)]` attributes the generator follows.
#[derive(Default)]
struct SerdeAttrs {
    rename: Option<String>,
    rename_all: Option<String>,
    rename_all_fields: Option<String>,
    tag: Option<String>,
    content: Option<String>,
    untagged: bool,
    transparent: bool,
    default: bool,
    skip: bool,
    skip_if: bool,
    flatten: bool,
    deny_unknown_fields: bool,
    /// Read (deserialized) otherwise than written: under another name, by
    /// another function or type, or skipped one way only. Its contract
    /// description is `any`.
    asymmetric: bool,
}

impl SerdeAttrs {
    fn of(attrs: &[syn::Attribute]) -> Result<Self> {
        let mut found = Self::default();
        let mut asymmetric = false;
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            attr.parse_nested_meta(|meta| {
                let key = meta.path.get_ident().map(ToString::to_string).unwrap_or_default();
                // `rename = "x"` or `rename(serialize = "x", ...)`.
                let mut text = |meta: &syn::meta::ParseNestedMeta| -> syn::Result<Option<String>> {
                    if meta.input.peek(syn::Token![=]) {
                        return Ok(Some(meta.value()?.parse::<syn::LitStr>()?.value()));
                    }
                    let mut serialized = None;
                    let mut deserialized = None;
                    meta.parse_nested_meta(|inner| {
                        let value = inner.value()?.parse::<syn::LitStr>()?.value();
                        if inner.path.is_ident("serialize") {
                            serialized = Some(value);
                        } else if inner.path.is_ident("deserialize") {
                            deserialized = Some(value);
                        }
                        Ok(())
                    })?;
                    if serialized != deserialized {
                        asymmetric = true;
                    }
                    Ok(serialized)
                };
                match key.as_str() {
                    "rename" => found.rename = text(&meta)?,
                    "rename_all" => found.rename_all = text(&meta)?,
                    "rename_all_fields" => found.rename_all_fields = text(&meta)?,
                    "tag" => found.tag = text(&meta)?,
                    "content" => found.content = text(&meta)?,
                    "untagged" => found.untagged = true,
                    "transparent" => found.transparent = true,
                    "flatten" => found.flatten = true,
                    "skip" | "skip_serializing" => {
                        found.skip = true;
                        found.asymmetric |= key == "skip_serializing";
                        skip_value(&meta)?;
                    }
                    "deny_unknown_fields" => found.deny_unknown_fields = true,
                    "alias" | "other" | "skip_deserializing" | "deserialize_with" | "from"
                    | "try_from" => {
                        found.asymmetric = true;
                        skip_value(&meta)?;
                    }
                    "default" => {
                        found.default = true;
                        skip_value(&meta)?;
                    }
                    "skip_serializing_if" => {
                        found.skip_if = true;
                        skip_value(&meta)?;
                    }
                    "with" | "serialize_with" | "into" | "remote" | "getter" | "serialize" => {
                        return Err(meta.error(format!(
                            "#[serde({key})] gives it a serde form the generator cannot read: declare its TypeScript with Exports::declared"
                        )));
                    }
                    _ => skip_value(&meta)?,
                }
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        }
        found.asymmetric |= asymmetric;
        Ok(found)
    }
}

fn skip_value(meta: &syn::meta::ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        meta.value()?.parse::<syn::Expr>()?;
    } else if meta.input.peek(syn::token::Paren) {
        meta.parse_nested_meta(|inner| skip_value(&inner))?;
    }
    Ok(())
}

/// A field name as serde's `rename_all` writes it.
fn rename_field(field: &str, rule: Option<&str>) -> String {
    match rule {
        None | Some("snake_case") => field.to_owned(),
        Some("lowercase") => field.to_lowercase(),
        Some("UPPERCASE") | Some("SCREAMING_SNAKE_CASE") => field.to_uppercase(),
        Some("PascalCase") => pascal(field),
        Some("camelCase") => {
            let pascal = pascal(field);
            let mut chars = pascal.chars();
            chars
                .next()
                .map(|first| first.to_lowercase().chain(chars).collect())
                .unwrap_or_default()
        }
        Some("kebab-case") => field.replace('_', "-"),
        Some("SCREAMING-KEBAB-CASE") => field.replace('_', "-").to_uppercase(),
        Some(_) => field.to_owned(),
    }
}

/// A variant name as serde's `rename_all` writes it.
fn rename_variant(variant: &str, rule: Option<&str>) -> String {
    let snake = || {
        let mut snake = String::new();
        for (index, ch) in variant.char_indices() {
            if index > 0 && ch.is_uppercase() {
                snake.push('_');
            }
            snake.extend(ch.to_lowercase());
        }
        snake
    };
    match rule {
        None | Some("PascalCase") => variant.to_owned(),
        Some("lowercase") => variant.to_lowercase(),
        Some("UPPERCASE") => variant.to_uppercase(),
        Some("camelCase") => {
            let mut chars = variant.chars();
            chars
                .next()
                .map(|first| first.to_lowercase().chain(chars).collect())
                .unwrap_or_default()
        }
        Some("snake_case") => snake(),
        Some("SCREAMING_SNAKE_CASE") => snake().to_uppercase(),
        Some("kebab-case") => snake().replace('_', "-"),
        Some("SCREAMING-KEBAB-CASE") => snake().replace('_', "-").to_uppercase(),
        Some(_) => variant.to_owned(),
    }
}

fn pascal(name: &str) -> String {
    let mut result = String::new();
    let mut upper = true;
    for ch in name.chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            result.extend(ch.to_uppercase());
            upper = false;
        } else {
            result.push(ch);
        }
    }
    result
}

fn camel(name: &str) -> String {
    let pascal = pascal(name.trim_start_matches('_'));
    let mut chars = pascal.chars();
    chars
        .next()
        .map(|first| first.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A property key in TypeScript: quoted unless it is an identifier.
fn ts_key(key: &str) -> String {
    if identifier(key, "$") {
        key.to_owned()
    } else {
        format!("{key:?}")
    }
}

/// TypeScript names a declaration may not take.
const RESERVED: [&str; 12] = [
    "RustError",
    "Services",
    "Error",
    "Promise",
    "Record",
    "Map",
    "Set",
    "Date",
    "Uint8Array",
    "Array",
    "Object",
    "Function",
];

fn doc_lines(attrs: &[syn::Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|attr| match &attr.meta {
            syn::Meta::NameValue(pair) if pair.path.is_ident("doc") => match &pair.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                }) => Some(text.value().trim().replace("*/", "*\\/")),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn render_jsdoc(lines: &[String], indent: &str) -> String {
    let lines: Vec<&String> = lines.iter().collect();
    match lines.as_slice() {
        [] => String::new(),
        [line] => format!("{indent}/** {line} */\n"),
        lines => {
            let mut text = format!("{indent}/**\n");
            for line in lines {
                text.push_str(&format!("{indent} * {line}\n").replace(" * \n", " *\n"));
            }
            text.push_str(&format!("{indent} */\n"));
            text
        }
    }
}

fn jsdoc(attrs: &[syn::Attribute], indent: &str) -> String {
    render_jsdoc(&doc_lines(attrs), indent)
}

/// Docs as a one-line JSDoc before a union member.
fn inline_doc(attrs: &[syn::Attribute]) -> String {
    let lines = doc_lines(attrs);
    if lines.is_empty() {
        String::new()
    } else {
        format!("/** {} */ ", lines.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_serde() {
        assert_eq!(rename_field("due_at", Some("camelCase")), "dueAt");
        assert_eq!(rename_field("due_at", Some("PascalCase")), "DueAt");
        assert_eq!(rename_field("due_at", Some("kebab-case")), "due-at");
        assert_eq!(rename_variant("NotFound", Some("snake_case")), "not_found");
        assert_eq!(rename_variant("NotFound", Some("camelCase")), "notFound");
        assert_eq!(
            rename_variant("NotFound", Some("SCREAMING-KEBAB-CASE")),
            "NOT-FOUND"
        );
        assert_eq!(camel("list_items"), "listItems");
        assert_eq!(ts_key("due-at"), "\"due-at\"");
    }

    /// An application crate `host` depending on `dep_crate` (whose files
    /// are `files`) and on rutis and serde, which are not read; the
    /// dependency has its feature `extra` on.
    fn sources(files: &[(&str, &str)]) -> (tempfile::TempDir, Sources) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("host")).unwrap();
        std::fs::write(dir.path().join("host/lib.rs"), "").unwrap();
        // Another package of the workspace, which the host does not use: a
        // marked type there is not the host's.
        std::fs::create_dir_all(dir.path().join("other")).unwrap();
        std::fs::write(
            dir.path().join("other/lib.rs"),
            "pub struct Clock;\n#[rutis::service]\nimpl Clock { pub fn now(&self) -> u32 { 0 } }\n",
        )
        .unwrap();
        for (path, text) in files {
            let path = dir.path().join("dep").join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let lib =
            |id: &str, name: &str, src: PathBuf| json_package(id, name, &src.to_string_lossy());
        let metadata = serde_json::json!({
            "packages": [
                lib("host", "host", dir.path().join("host/lib.rs")),
                lib("dep", "dep_crate", dir.path().join("dep/lib.rs")),
                lib("rutis", "rutis", dir.path().join("none.rs")),
                lib("serde", "serde", dir.path().join("none.rs")),
                lib("bridge", "rutis_bridge", dir.path().join("none.rs")),
                lib("other", "other_crate", dir.path().join("other/lib.rs")),
            ],
            "resolve": { "root": "host", "nodes": [
                { "id": "host", "deps": [{ "name": "dep_crate", "pkg": "dep" }, { "name": "rutis", "pkg": "rutis" }], "features": [] },
                { "id": "dep", "deps": [{ "name": "rutis", "pkg": "rutis" }, { "name": "serde", "pkg": "serde" }, { "name": "rutis_bridge", "pkg": "bridge" }], "features": ["extra"] },
                { "id": "other", "deps": [{ "name": "rutis", "pkg": "rutis" }], "features": [] },
            ] },
        });
        let sources = Sources::from_metadata(&metadata, "host").unwrap();
        (dir, sources)
    }

    fn json_package(id: &str, name: &str, src: &str) -> serde_json::Value {
        serde_json::json!({ "id": id, "name": name, "targets": [{ "kind": ["lib"], "name": name, "src_path": src }] })
    }

    fn generate(exports: Exports, files: &[(&str, &str)]) -> Result<Generated> {
        let (_dir, mut sources) = sources(files);
        exports.generate_from(&mut sources)
    }

    const LIB: &str = r#"
        mod inner;
        pub mod types;
        pub use inner::*;
    "#;

    const INNER: &str = r#"
        use crate::types::{Item, Problem, Res, Single};
        use rutis::host::BoxFuture;
        use rutis::Error;

        /// The store.
        pub struct Store;

        #[rutis::service]
        impl Store {
            pub fn new() -> Self { Store }
            /// Put an item.
            pub fn put(&self, item: Item) -> Res<u32> { Ok(1) }
            pub async fn find(&self, name: &str, limit: Option<u8>) -> Option<Item> { None }
            pub fn watch(&self, each: impl Fn(Item, u8) -> BoxFuture<'static, Result<bool, Error>> + Send) {}
            pub fn reset(&mut self) {}
            pub fn tied(&self, ctx: &rutis::Ctx, n: u8) {}
            pub fn single(&self) -> Result<(), Single> { Ok(()) }
            pub fn borrowing(&self, each: impl Fn(&Item) -> BoxFuture<'static, rutis::Result>) {}
            #[cfg(feature = "extra")]
            pub fn extra(&self) -> String { String::new() }
            #[cfg(feature = "missing")]
            pub fn missing(&self) {}
            fn private(&self) {}
        }

        mod more {
            #[rutis::service]
            impl super::Store {
                pub fn count(&self) -> Result<usize, String> { Ok(0) }
            }
        }
    "#;

    const TYPES: &str = r#"
        use serde::{Deserialize, Serialize};

        pub type Res<T> = std::result::Result<T, Problem>;

        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Item {
            pub item_name: String,
            #[serde(rename = "n")]
            pub count: u64,
            #[serde(skip)]
            pub cache: Vec<u8>,
            #[serde(flatten)]
            pub meta: Meta,
            pub pages: Page<Vec<u16>>,
        }

        #[derive(Serialize, Deserialize)]
        pub struct Meta { pub at: f64 }

        #[derive(Serialize, Deserialize)]
        pub struct Page<T> { pub items: T, pub next: Option<String> }

        #[derive(Debug, Serialize)]
        #[serde(tag = "kind", content = "detail", rename_all = "kebab-case")]
        pub enum Problem { NotThere, TooBig(u64) }

        #[derive(Debug, Serialize)]
        pub struct Single { pub message: String }

        impl std::fmt::Display for Single {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.message)
            }
        }
    "#;

    const MARKED: &str = r#"
        use rutis::service;

        /// The clock.
        pub struct Clock;

        #[service]
        impl Clock {
            pub fn new() -> Self { Clock }
            pub fn now(&self) -> u32 { 0 }
            pub fn explicit(self: &Self) -> u32 { 0 }
            #[service(skip)]
            pub fn reset(&self) {}
        }

        impl Clock {
            pub fn unmarked(&self) -> u8 { 0 }
        }

        pub struct Named;

        #[rutis::service(name = "board")]
        impl Named {
            pub fn read(&self) -> String { String::new() }
        }
    "#;

    /// Types marked `#[rutis::service]` are declared: only their marked
    /// blocks, without their skipped methods.
    #[test]
    fn marked_types_are_declared() {
        let generated = generate(Exports::new(), &[("lib.rs", MARKED)]).unwrap();
        let ts = &generated.typescript;
        // The header names nothing the plugin author did not write (#680).
        assert!(
            ts.starts_with("// Generated from the service declarations; do not edit.\n"),
            "{ts}"
        );
        assert!(ts.contains("  clock: Clock\n"), "{ts}");
        assert!(ts.contains("  board: Named\n"), "{ts}");
        assert!(ts.contains("now(): Promise<number>"), "{ts}");
        assert!(ts.contains("explicit(): Promise<number>"), "{ts}");
        // `self: &Self` is `&self`: no warning that it cannot be bound.
        assert!(generated.warnings.is_empty(), "{:?}", generated.warnings);
        assert!(ts.contains("read(): Promise<string>"), "{ts}");
        assert!(!ts.contains("unmarked") && !ts.contains("reset"), "{ts}");
    }

    /// The contract description names each service's methods and what
    /// each parameter takes; what it cannot be sure of is `any`.
    #[test]
    fn tagged_text_values_are_strings_in_the_contract() {
        let source = r#"
use rutis_bridge::session::{self as s, DateTime};

pub struct Clock;

#[rutis::service]
impl Clock {
    pub fn set(&self, at: DateTime, day: s::Date, time: s::TimeOfDay, price: s::Decimal, id: s::Uuid) {}
}
"#;
        let generated = generate(Exports::new(), &[("lib.rs", source)]).unwrap();
        assert!(generated.warnings.is_empty(), "{:?}", generated.warnings);
        let contract: serde_json::Value = serde_json::from_str(&generated.contract).unwrap();
        let string = serde_json::json!({ "kind": "string" });
        assert_eq!(
            contract["services"]["clock"]["methods"]["set"]["params"],
            serde_json::json!([string, string, string, string, string])
        );
        assert!(generated.typescript.contains(
            "set(at: string, day: string, time: string, price: string, id: string): Promise<void>"
        ), "{}", generated.typescript);
        assert!(generated.python.contains(
            "at: datetime.datetime, day: datetime.date, time: datetime.time, price: decimal.Decimal, id: uuid.UUID"
        ), "{}", generated.python);
    }

    #[test]
    fn the_contract_describes_what_each_parameter_takes() {
        let source = r#"
use std::collections::HashMap;
use rutis::host::BoxFuture;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item { pub item_name: String, #[serde(default)] pub done: bool, pub note: Option<String> }

#[derive(Serialize, Deserialize)]
pub struct Open { pub name: String }

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level { Low, High }

#[derive(Serialize, Deserialize)]
pub enum Shape { Dot, Line(u8) }

#[derive(Serialize, Deserialize)]
pub struct Flat { #[serde(flatten)] pub open: Open }

#[derive(Serialize, Deserialize)]
pub struct Aliased { #[serde(alias = "nm")] pub name: String }

#[derive(Serialize, Deserialize)]
pub struct Page<T> { pub items: Vec<T> }

pub struct Board;

#[rutis::service]
impl Board {
    pub fn put(&self, ctx: &rutis::Ctx, item: Item, open: Open, level: Level, count: i8, ratio: f32) {}
    pub fn more(&self, tags: Vec<String>, pair: (u16, bool), map: HashMap<String, u64>, note: Option<String>) {}
    pub fn loose(&self, shape: Shape, flat: Flat, aliased: Aliased, page: Page<u8>) {}
    pub fn each(&self, visit: impl Fn(Item, u32) -> BoxFuture<'static, rutis::Result<String>> + Send) {}
    pub fn load(&self, def: rutis::Definition) {}
    pub fn odd(&self, it: std::sync::mpsc::Sender<u8>) {}
}
"#;
        let generated = generate(Exports::new(), &[("lib.rs", source)]).unwrap();
        assert_eq!(generated.warnings.len(), 1, "{:?}", generated.warnings);
        let contract: serde_json::Value = serde_json::from_str(&generated.contract).unwrap();
        assert_eq!(contract["version"], 1);
        let methods = &contract["services"]["board"]["methods"];
        let int =
            |min: &str, max: &str| serde_json::json!({ "kind": "int", "min": min, "max": max });
        // The calling row's context is not a parameter.
        assert_eq!(
            methods["put"]["params"],
            serde_json::json!([
                { "kind": "ref", "name": "Item" },
                { "kind": "ref", "name": "Open" },
                { "kind": "ref", "name": "Level" },
                int("-128", "127"),
                { "kind": "float" },
            ])
        );
        assert_eq!(
            methods["more"]["params"],
            serde_json::json!([
                { "kind": "list", "of": { "kind": "string" } },
                { "kind": "tuple", "items": [int("0", "65535"), { "kind": "bool" }] },
                { "kind": "map", "values": int("0", "18446744073709551615") },
                { "kind": "option", "of": { "kind": "string" } },
            ])
        );
        assert_eq!(
            methods["loose"]["params"],
            serde_json::json!([
                { "kind": "ref", "name": "Shape" },
                { "kind": "ref", "name": "Flat" },
                { "kind": "ref", "name": "Aliased" },
                { "kind": "any" },
            ])
        );
        assert_eq!(
            methods["each"]["params"],
            serde_json::json!([{
                "kind": "function",
                "params": [{ "kind": "ref", "name": "Item" }, int("0", "4294967295")],
                "result": { "kind": "string" },
            }])
        );
        assert_eq!(
            methods["load"]["params"],
            serde_json::json!([{ "kind": "plugin" }])
        );
        // A method the generator cannot declare is still the service's: its
        // arguments are not checked.
        assert_eq!(methods["odd"], serde_json::json!({ "params": null }));
        let types = &contract["types"];
        assert_eq!(
            types["Item"],
            serde_json::json!({
                "kind": "struct",
                "fields": [
                    { "name": "itemName", "type": { "kind": "string" }, "optional": false },
                    { "name": "done", "type": { "kind": "bool" }, "optional": true },
                    { "name": "note", "type": { "kind": "option", "of": { "kind": "string" } }, "optional": true },
                ],
                "denyUnknown": true,
            })
        );
        assert_eq!(types["Open"]["denyUnknown"], false);
        assert_eq!(
            types["Level"],
            serde_json::json!({ "kind": "enum", "names": ["low", "high"] })
        );
        for any in ["Shape", "Flat", "Aliased", "Page"] {
            assert_eq!(types[any], serde_json::json!({ "kind": "any" }), "{any}");
        }
    }

    const PY_SOURCE: &str = r#"
use std::collections::HashMap;
use rutis::host::BoxFuture;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item { pub item_name: String, #[serde(default)] pub done: bool, pub note: Option<String> }

#[derive(Serialize, Deserialize)]
pub struct Odd { #[serde(rename = "due-at")] pub due_at: u32, pub class: Option<u8> }

#[derive(Serialize, Deserialize)]
pub struct Meta { pub at: f64 }

#[derive(Serialize, Deserialize)]
pub struct Flat { pub name: String, #[serde(flatten)] pub meta: Meta }

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level { Low, High }

#[derive(Serialize, Deserialize)]
pub enum Shape { Dot, Line(u8), Box { w: u8, h: u8 } }

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Tagged { Plain, Sized { size: u32 } }

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum Adjacent { Empty, Text(String) }

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Either { Number(f64), Words(Vec<String>) }

#[derive(Serialize, Deserialize)]
pub struct Page<T> { pub items: Vec<T> }

#[derive(Serialize, Deserialize)]
pub struct Pair(pub u8, pub String);

#[derive(Debug, Serialize)]
pub enum Problem { Missing, TooBig(u64) }

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("problem") }
}

pub struct Text(pub String);

/// The board.
pub struct Board;

#[rutis::service]
impl Board {
    /// Put an item.
    pub fn put(&self, item: Item, odd: Odd, flat: Flat, level: Level) -> Result<u32, Problem> { Ok(0) }
    pub fn shapes(&self, shape: Shape, tagged: Tagged, adjacent: Adjacent, either: Either) {}
    pub fn more(&self, page: Page<u8>, pair: Pair, map: HashMap<String, Vec<u64>>, from: String, limit: Option<u8>) -> Option<Item> { None }
    pub fn each(&self, visit: impl Fn(Item, u32) -> BoxFuture<'static, rutis::Result<String>> + Send) {}
    pub fn load(&self, def: rutis::Definition, text: Text) {}
}
"#;

    /// The Python module: Protocols, TypedDicts (optional keys in a
    /// `total=False` parent), Literals and unions of TypedDicts.
    #[test]
    fn python_types_follow_serde() {
        let generated = generate(
            Exports::new()
                .python("services.py")
                .declared("dep_crate::Text", "string"),
            &[("lib.rs", PY_SOURCE)],
        )
        .unwrap();
        let py = &generated.python;
        for expected in [
            "# Generated from the service declarations; do not edit.\n\nfrom __future__ import annotations\n\nimport typing\n\nimport rutis\n",
            "RustError = rutis.RemoteError\n",
            "class _ItemOptional(typing.TypedDict, total=False):\n    done: bool\n    note: str | None\n",
            "class Item(_ItemOptional):\n    itemName: str\n",
            "_OddRequired = typing.TypedDict(\"_OddRequired\", {\"due-at\": \"int\"})\n",
            "_OddOptional = typing.TypedDict(\"_OddOptional\", {\"class\": \"int | None\"}, total=False)\n",
            "class Odd(_OddRequired, _OddOptional):\n    pass\n",
            "class Flat(Meta):\n    name: str\n",
            "Level: typing.TypeAlias = 'typing.Literal[\"low\", \"high\"]'\n",
            "class _ShapeLine(typing.TypedDict):\n    Line: int\n",
            "class _ShapeBoxContent(typing.TypedDict):\n    w: int\n    h: int\n",
            "Shape: typing.TypeAlias = 'typing.Literal[\"Dot\"] | _ShapeLine | _ShapeBox'\n",
            "class _TaggedSized(typing.TypedDict):\n    kind: typing.Literal[\"Sized\"]\n    size: int\n",
            "Tagged: typing.TypeAlias = \"_TaggedPlain | _TaggedSized\"\n",
            "class _AdjacentText(typing.TypedDict):\n    t: typing.Literal[\"Text\"]\n    c: str\n",
            "Either: typing.TypeAlias = \"float | list[str]\"\n",
            "class Page(typing.TypedDict):\n    items: list[typing.Any]\n",
            "Pair: typing.TypeAlias = \"tuple[int, str]\"\n",
            "class Board(typing.Protocol):\n    \"\"\"The board.\"\"\"\n\n    async def put(self, item: Item, odd: Odd, flat: Flat, level: Level) -> int:\n        \"\"\"Put an item.\n\n        Raises ProblemThrown.\n        \"\"\"\n        ...\n",
            "    async def more(self, page: Page, pair: Pair, map: dict[str, list[int]], from_: str, limit: int | None = None) -> Item | None:\n",
            "    async def each(self, visit: typing.Callable[[Item, int], str | typing.Awaitable[str]]) -> None:\n",
            "    async def load(self, def_: rutis.Plugin, text: typing.Any) -> None:\n",
            "ProblemThrown = RustError\nProblemThrownCode: typing.TypeAlias = 'typing.Literal[\"Missing\", \"TooBig\"]'\n",
        ] {
            assert!(py.contains(expected), "{expected}\n---\n{py}");
        }
        // A declared type without a Python type is Any, with a warning.
        assert!(
            generated.warnings.iter().any(|warning| warning
                == "dep_crate::Text has no Python type: declare it with .declared_python (written as Any)"),
            "{:?}",
            generated.warnings
        );
        // Without the Python module, no such warning.
        let generated = generate(
            Exports::new().declared("dep_crate::Text", "string"),
            &[("lib.rs", PY_SOURCE)],
        )
        .unwrap();
        assert!(!generated
            .warnings
            .iter()
            .any(|warning| warning.contains("Python")));
    }

    /// `.declared_python` gives a declared type its Python type; alone, it
    /// is an error.
    #[test]
    fn declared_python_gives_a_declared_type_its_python() {
        let generated = generate(
            Exports::new()
                .python("services.py")
                .declared("dep_crate::Text", "string")
                .declared_python("dep_crate::Text", "str"),
            &[("lib.rs", PY_SOURCE)],
        )
        .unwrap();
        assert!(generated.python.contains("def_: rutis.Plugin, text: str)"));
        assert!(!generated
            .warnings
            .iter()
            .any(|warning| warning.contains("Python")));
        let error = generate(
            Exports::new().declared_python("dep_crate::Text", "str"),
            &[("lib.rs", PY_SOURCE)],
        )
        .unwrap_err();
        assert_eq!(
            error,
            "declared_python(\"dep_crate::Text\", ..) needs its TypeScript too: add .declared(\"dep_crate::Text\", ..)"
        );
    }

    /// Two marked types given one name are an error naming both.
    #[test]
    fn two_marked_types_with_one_name_are_an_error() {
        let error = generate(
            Exports::new(),
            &[(
                "lib.rs",
                &format!(
                    "{MARKED}\npub struct Board;\n#[rutis::service]\nimpl Board {{ pub fn a(&self) {{}} }}"
                ),
            )],
        )
        .unwrap_err();
        assert!(
            error.contains("board") && error.contains("Named") && error.contains("Board"),
            "{error}"
        );
    }

    /// A plugin definition, as a parameter or returned, is declared as the
    /// TS plugin type.
    #[test]
    fn plugin_definitions_cross_as_the_plugin_type() {
        let source = r#"
pub struct Parts;
#[rutis::service]
impl Parts {
    pub fn register(&self, scope: String, def: rutis::Definition) {}
    pub fn first(&self) -> rutis::Definition { unimplemented!() }
}
"#;
        let generated = generate(Exports::new(), &[("lib.rs", source)]).unwrap();
        let ts = &generated.typescript;
        let plugin = "import('@arcships/rutis').Plugin | ((ctx: any, config: any) => unknown) | { apply(ctx: any, config: any): unknown }";
        assert!(
            ts.contains(&format!(
                "register(scope: string, def: {plugin}): Promise<void>"
            )),
            "{ts}"
        );
        assert!(ts.contains(&format!("first(): Promise<{plugin}>")), "{ts}");
    }

    fn store() -> Result<Generated> {
        generate(
            Exports::new(),
            &[("lib.rs", LIB), ("inner.rs", INNER), ("types.rs", TYPES)],
        )
    }

    /// A type re-exported from a private module, with methods in another
    /// module, aliases followed and serde's attributes read.
    #[test]
    fn a_dependencys_type_is_read_through_its_reexports() {
        let generated = store().unwrap();
        let ts = &generated.typescript;
        for expected in [
            "export interface Store {",
            "  /**\n   * Put an item.\n   * @throws {ProblemThrown}\n   */\n  put(item: Item): Promise<number>",
            "  find(name: string, limit?: number | null): Promise<Item | null>",
            "  watch(each: (arg0: Item, arg1: number) => boolean | Promise<boolean>): Promise<void>",
            "  extra(): Promise<string>",
            "  tied(n: number): Promise<void>",
            "  /** @throws {StringThrown} */\n  count(): Promise<number | bigint>",
            "export type Item = {\n  itemName: string\n  n: number | bigint\n  pages: Page<number[]>\n} & Meta",
            "export interface Page<T> {\n  items: T\n  next?: string | null\n}",
            "export type Problem = { kind: \"not-there\" } | { kind: \"too-big\"; detail: number | bigint }",
            "export type ProblemThrown = RustError<\"Problem\", \"not-there\" | \"too-big\", Problem>",
            "export interface Services {\n  store: Store\n}",
        ] {
            assert!(ts.contains(expected), "{expected}\n---\n{ts}");
        }
        for absent in ["missing", "private", "reset", "new(", "cache"] {
            assert!(!ts.contains(absent), "{absent}\n---\n{ts}");
        }
    }

    /// What cannot be bound is left out, saying where and why.
    #[test]
    fn methods_that_cannot_be_bound_are_left_out_with_a_warning() {
        let warnings = store().unwrap().warnings;
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("inner.rs:")
                && w.contains("Store::reset")
                && w.contains("&self")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("Store::borrowing") && w.contains("by value")),
            "{warnings:?}"
        );
    }

    /// A type whose serde form the generator cannot read needs its
    /// TypeScript declared; declared, it is used as given.
    #[test]
    fn a_type_without_a_serde_derive_needs_its_typescript_declared() {
        let inner = INNER.replace(
            "pub struct Store;",
            "pub struct Store;\n pub struct Opaque;\n \
             #[derive(serde::Serialize)] pub struct Wrapper { pub inner: Opaque, pub other: Other }\n \
             #[derive(serde::Serialize)] pub struct Other { pub back: Option<Box<Wrapper>> }\n \
             #[rutis::service] impl Store { pub fn opaque(&self) -> Opaque { Opaque } pub fn wrapped(&self) -> Wrapper { todo!() } }",
        );
        let files = [
            ("lib.rs", LIB),
            ("inner.rs", inner.as_str()),
            ("types.rs", TYPES),
        ];
        let generated = generate(Exports::new(), &files).unwrap();
        for method in ["Store::opaque", "Store::wrapped"] {
            assert!(
                generated
                    .warnings
                    .iter()
                    .any(|w| w.contains(method) && w.contains("Exports::declared")),
                "{method}: {:?}",
                generated.warnings
            );
        }
        // Nothing of what could not be declared is left in the declarations.
        for absent in ["Wrapper", "Other"] {
            assert!(
                !generated.typescript.contains(absent),
                "{}",
                generated.typescript
            );
        }
        let generated = generate(
            Exports::new().declared("dep_crate::Opaque", "import('./opaque').Opaque"),
            &files,
        )
        .unwrap();
        assert!(
            generated
                .typescript
                .contains("opaque(): Promise<import('./opaque').Opaque>"),
            "{}",
            generated.typescript
        );
    }

    // ── Cycles (#459) ───────────────────────────────────────────

    /// Runs `run` on a thread of its own and fails the test should it take
    /// longer than `limit`, so that a lookup that does not end fails fast
    /// instead of hanging the test run.
    fn within<T: Send + 'static>(
        limit: std::time::Duration,
        run: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(move || {
                let _ = send.send(run());
            })
            .unwrap();
        match receive.recv_timeout(limit) {
            Ok(value) => value,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("did not finish within {limit:?}")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("the run panicked"),
        }
    }

    /// [`generate`] under [`within`] 10 s, asserting it took under 2 s.
    fn generate_quickly(exports: Exports, files: &[(&str, &str)]) -> Generated {
        let files: Vec<(String, String)> = files
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect();
        within(std::time::Duration::from_secs(10), move || {
            let files: Vec<(&str, &str)> = files
                .iter()
                .map(|(path, text)| (path.as_str(), text.as_str()))
                .collect();
            let started = std::time::Instant::now();
            let generated = generate(exports, &files).unwrap();
            let took = started.elapsed();
            assert!(took < std::time::Duration::from_secs(2), "took {took:?}");
            generated
        })
    }

    const NAMES: &str = r#"
        use crate::*;
        pub struct Service;
        #[rutis::service]
        impl Service {
            pub fn names(&self) -> Result<Vec<String>, String> { Ok(Vec::new()) }
            pub fn first(&self) -> Option<Item6> { None }
        }
    "#;

    fn assert_names(generated: &Generated) {
        let ts = &generated.typescript;
        assert!(
            ts.contains("/** @throws {StringThrown} */\n  names(): Promise<string[]>"),
            "{ts}"
        );
        assert!(ts.contains("first(): Promise<Item6 | null>"), "{ts}");
        assert!(generated.warnings.is_empty(), "{:?}", generated.warnings);
    }

    /// The layout of #459: the root re-exports every module with
    /// `pub use m::*;`, and every module has `use crate::*;`.
    #[test]
    fn glob_imports_of_the_crate_root_from_its_modules_end() {
        let lib = (1..=6)
            .map(|n| format!("mod m{n}; pub use m{n}::*;\n"))
            .collect::<String>();
        let items: Vec<(String, String)> = (2..=6)
            .map(|n| {
                (
                    format!("m{n}.rs"),
                    format!(
                        "use crate::*;\n#[derive(serde::Serialize)] pub struct Item{n} {{ pub at: Option<String> }}\n"
                    ),
                )
            })
            .collect();
        let mut files = vec![("lib.rs", lib.as_str()), ("m1.rs", NAMES)];
        files.extend(
            items
                .iter()
                .map(|(path, text)| (path.as_str(), text.as_str())),
        );
        let generated = generate_quickly(Exports::new(), &files);
        assert_names(&generated);
        assert!(
            generated
                .typescript
                .contains("export interface Item6 {\n  at?: string | null\n}"),
            "{}",
            generated.typescript
        );
    }

    /// `use super::*;` in a module its parent globs.
    #[test]
    fn glob_imports_of_the_parent_end() {
        let generated = generate_quickly(
            Exports::new(),
            &[
                ("lib.rs", "pub mod a; pub use a::*;"),
                ("a.rs", "use super::*; pub mod b; pub use b::*;"),
                (
                    "a/b.rs",
                    &format!(
                        "use super::*;\n#[derive(serde::Serialize)] pub struct Item6;\n{NAMES}"
                    ),
                ),
            ],
        );
        assert_names(&generated);
    }

    /// root → a → b → root, each a glob of the next.
    #[test]
    fn a_longer_chain_of_globs_ends() {
        let generated = generate_quickly(
            Exports::new(),
            &[
                ("lib.rs", "mod a; pub use a::*; mod b; mod m1;"),
                ("a.rs", "pub use crate::b::*;"),
                ("b.rs", "pub use crate::*; pub use crate::m1::*;"),
                (
                    "m1.rs",
                    &format!(
                        "use crate::b::*;\n#[derive(serde::Serialize)] pub struct Item6;\n{}",
                        NAMES.replace("use crate::*;", "")
                    ),
                ),
            ],
        );
        assert_names(&generated);
    }

    /// Two crates that glob each other's root (a dev-dependency cycle).
    #[test]
    fn globs_across_two_crates_that_import_each_other_end() {
        let generated = within(std::time::Duration::from_secs(10), || {
            let dir = tempfile::tempdir().unwrap();
            let write = |path: &str, text: &str| {
                let path = dir.path().join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            };
            write("host/lib.rs", "");
            write("a/lib.rs", "pub use b_crate::*; mod m1; pub use m1::*;");
            write("a/m1.rs", NAMES);
            write(
                "b/lib.rs",
                "pub use a_crate::*;\n#[derive(serde::Serialize)] pub struct Item6;\n",
            );
            let lib = |id: &str, name: &str, src: &str| {
                json_package(id, name, &dir.path().join(src).to_string_lossy())
            };
            let metadata = serde_json::json!({
                "packages": [
                    lib("host", "host", "host/lib.rs"),
                    lib("a", "a_crate", "a/lib.rs"),
                    lib("b", "b_crate", "b/lib.rs"),
                    lib("rutis", "rutis", "none.rs"),
                    lib("serde", "serde", "none.rs"),
                ],
                "resolve": { "root": "host", "nodes": [
                    { "id": "host", "deps": [{ "name": "a_crate", "pkg": "a" }], "features": [] },
                    { "id": "a", "deps": [{ "name": "b_crate", "pkg": "b" }, { "name": "rutis", "pkg": "rutis" }, { "name": "serde", "pkg": "serde" }], "features": [] },
                    { "id": "b", "deps": [{ "name": "a_crate", "pkg": "a" }, { "name": "serde", "pkg": "serde" }], "features": [] },
                ] },
            });
            let mut sources = Sources::from_metadata(&metadata, "host").unwrap();
            let started = std::time::Instant::now();
            let generated = Exports::new().generate_from(&mut sources).unwrap();
            let took = started.elapsed();
            assert!(took < std::time::Duration::from_secs(2), "took {took:?}");
            generated
        });
        assert_names(&generated);
    }

    /// `type A = B; type B = A;` leaves the method out with a warning;
    /// an alias used twice in one type is no cycle.
    #[test]
    fn a_type_alias_that_refers_to_itself_is_a_warning() {
        let generated = generate_quickly(
            Exports::new(),
            &[
                ("lib.rs", "mod m1; pub use m1::*;"),
                (
                    "m1.rs",
                    &format!(
                        "pub type A = B;\npub type B = A;\npub type Pair<T> = (T, T);\n\
                         #[derive(serde::Serialize)] pub struct Item6;\n\
                         #[rutis::service] impl Service {{ pub fn bad(&self) -> A {{ todo!() }} \
                         pub fn pairs(&self) -> Pair<Pair<u8>> {{ todo!() }} }}\n{NAMES}"
                    ),
                ),
            ],
        );
        assert!(
            generated
                .warnings
                .iter()
                .any(|w| w.contains("Service::bad") && w.contains("refers to itself")),
            "{:?}",
            generated.warnings
        );
        assert_eq!(generated.warnings.len(), 1, "{:?}", generated.warnings);
        assert!(
            generated
                .typescript
                .contains("pairs(): Promise<[[number, number], [number, number]]>"),
            "{}",
            generated.typescript
        );
    }

    /// A generic alias's type argument is read where the alias is used,
    /// not where it is defined: `Thing` is not in scope next to `Res`.
    #[test]
    fn an_alias_argument_is_read_at_its_use() {
        let generated = generate(
            Exports::new(),
            &[
                ("lib.rs", "mod err; mod model; mod svc; pub use svc::*;"),
                (
                    "err.rs",
                    "use crate::model::Problem;\n\
                     pub type Res<T> = std::result::Result<T, Problem>;\n\
                     pub type More<T> = Res<Vec<T>>;\n\
                     pub type ResE<T, E> = std::result::Result<T, E>;",
                ),
                (
                    "model.rs",
                    "#[derive(serde::Serialize)] pub struct Thing { pub id: String }\n\
                     #[derive(Debug, serde::Serialize)] pub struct Problem { pub message: String }",
                ),
                (
                    "svc.rs",
                    "use crate::err::{More, Res, ResE}; use crate::model::{Problem, Thing};\n\
                     pub struct Service;\n\
                     #[rutis::service] impl Service {\n\
                         pub fn one(&self) -> Res<Thing> { todo!() }\n\
                         pub fn many(&self) -> More<Thing> { todo!() }\n\
                         pub fn other(&self) -> ResE<Thing, Problem> { todo!() }\n\
                     }",
                ),
            ],
        )
        .unwrap();
        for expected in [
            "one(): Promise<Thing>",
            "many(): Promise<Thing[]>",
            "other(): Promise<Thing>",
        ] {
            assert!(
                generated.typescript.contains(expected),
                "{expected}\n{}",
                generated.typescript
            );
        }
        assert!(generated.warnings.is_empty(), "{:?}", generated.warnings);
    }

    /// A defaulted type parameter is written where the alias is defined:
    /// its `Problem` is not in scope at the use site.
    #[test]
    fn an_alias_default_is_read_where_the_alias_is_defined() {
        let generated = generate(
            Exports::new(),
            &[
                ("lib.rs", "mod err; mod model; mod svc; pub use svc::*;"),
                (
                    "err.rs",
                    "use crate::model::Problem;\n\
                     pub type Res<T, E = Problem> = std::result::Result<T, E>;",
                ),
                (
                    "model.rs",
                    "#[derive(serde::Serialize)] pub struct Thing { pub id: String }\n\
                     #[derive(Debug, serde::Serialize)]\n\
                     #[serde(tag = \"kind\", content = \"detail\")]\n\
                     pub enum Problem { Missing }",
                ),
                (
                    "svc.rs",
                    "use crate::err::Res; use crate::model::Thing;\n\
                     pub struct Service;\n\
                     #[rutis::service] impl Service { pub fn one(&self) -> Res<Thing> { todo!() } }",
                ),
            ],
        )
        .unwrap();
        assert!(
            generated.typescript.contains("one(): Promise<Thing>"),
            "{}",
            generated.typescript
        );
        assert!(
            generated
                .typescript
                .contains("RustError<\"Problem\", \"Missing\", Problem>"),
            "{}",
            generated.typescript
        );
    }

    /// A `#[path]` back to a file being read is skipped with a warning.
    #[test]
    fn a_module_path_back_to_its_own_file_is_a_warning() {
        let generated = generate_quickly(
            Exports::new(),
            &[
                (
                    "lib.rs",
                    "mod m1; pub use m1::*; #[derive(serde::Serialize)] pub struct Item6;",
                ),
                (
                    "m1.rs",
                    &format!("#[path = \"lib.rs\"] mod again;\n{NAMES}"),
                ),
            ],
        );
        assert!(
            generated
                .warnings
                .iter()
                .any(|w| w.contains("m1.rs") && w.contains("`again`")),
            "{:?}",
            generated.warnings
        );
        assert!(
            generated.typescript.contains("names(): Promise<string[]>"),
            "{}",
            generated.typescript
        );
    }
}
