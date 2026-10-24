//! Existing Rust services exported by name to the plugins of other
//! languages: a dispatcher and TypeScript declarations generated from the
//! Rust definitions, read from the source of the crates that define them.

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

/// The Rust services to export by name, as an application's `build.rs`
/// lists them; [`Exports::generate`] writes the generated module (declared
/// with [`include_exports!`](crate::include_exports)) and TypeScript
/// declarations into `OUT_DIR`.
///
/// ```ignore
/// rutis_bridge::build::Exports::new()
///     .service("calendar", "calendar_services::Calendar")
///     .service_in("notes", "calendar_services::Notes", "session")
///     .typescript("rows/services.d.ts")
///     .generate()
///     .expect("generate the exported services");
/// ```
///
/// A type is named by a path from the application crate (`crate::...`, or
/// a dependency's name): it may be a re-export, and its source is found
/// through `cargo metadata`. Its public `&self` methods are exported, each
/// `async` for the caller (a Promise / awaitable); one the generator cannot
/// bind is left out with a build warning saying why.
///
/// Types marked `#[rutis::service]` need no line here: they are served by
/// name when provided. The declarations cover them too, found in the
/// package and every crate it uses that depends on rutis, so
/// `Exports::new().typescript("services.d.ts").generate()` keeps their
/// declarations current; listing a marked type is an error. List only
/// types whose source you cannot change.
#[derive(Default)]
pub struct Exports {
    services: Vec<Wanted>,
    declared: Vec<(String, String)>,
    typescript: Option<PathBuf>,
}

struct Wanted {
    name: String,
    path: String,
    group: Option<String>,
}

impl Exports {
    pub fn new() -> Self {
        Self::default()
    }

    /// Export the type at `path` as the service `name`, outside instances.
    pub fn service(mut self, name: &str, path: &str) -> Self {
        self.services.push(Wanted {
            name: name.to_owned(),
            path: path.to_owned(),
            group: None,
        });
        self
    }

    /// Export it inside each instance of the instanced group `group`, as
    /// `register_shared_instance` names a service: rows use their own
    /// instance's.
    pub fn service_in(mut self, name: &str, path: &str, group: &str) -> Self {
        self.services.push(Wanted {
            name: name.to_owned(),
            path: path.to_owned(),
            group: Some(group.to_owned()),
        });
        self
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

    /// Also write the TypeScript declarations to `path` (relative to the
    /// package's `Cargo.toml`), where the rows' TypeScript finds them. It is
    /// written only when it changes.
    pub fn typescript(mut self, path: impl AsRef<Path>) -> Self {
        self.typescript = Some(path.as_ref().to_owned());
        self
    }

    pub fn generate(self) -> std::result::Result<(), Box<dyn std::error::Error>> {
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
        let output = PathBuf::from(std::env::var("OUT_DIR")?);
        std::fs::write(output.join("rutis_exports.rs"), &generated.rust)?;
        std::fs::write(output.join("rutis_exports.d.ts"), &generated.typescript)?;
        if let Some(path) = &self.typescript {
            let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
            let path = root.join(path);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(&generated.typescript) {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &generated.typescript)?;
            }
        }
        Ok(())
    }

    /// The TypeScript declarations of the services of the crate in
    /// `crate_dir` (its `Cargo.toml`): the types it lists here and those
    /// marked `#[rutis::service]`. What `rutis-host types` writes, outside
    /// a build script.
    pub fn declarations(
        self,
        crate_dir: impl AsRef<Path>,
    ) -> std::result::Result<String, Box<dyn std::error::Error>> {
        let mut sources = Sources::of_crate(crate_dir.as_ref())?;
        let generated = self.generate_from(&mut sources)?;
        for warning in &generated.warnings {
            eprintln!("warning: {warning}");
        }
        Ok(generated.typescript)
    }

    pub(super) fn generate_from(&self, sources: &mut Sources) -> Result<Generated> {
        let root = sources.host_root()?;
        let mut generator = Generator {
            sources,
            declared: HashMap::new(),
            named: HashMap::new(),
            names: HashSet::new(),
            declarations: Vec::new(),
            thrown: BTreeMap::new(),
            warnings: Vec::new(),
            cyclic: HashMap::new(),
        };
        for (path, typescript) in &self.declared {
            let parsed: syn::Path =
                syn::parse_str(path).map_err(|error| format!("{path}: {error}"))?;
            let res = generator
                .sources
                .resolve(root, &parsed)?
                .ok_or_else(|| format!("declared type {path} is not found"))?;
            generator.declared.insert(res, typescript.clone());
        }
        let mut rust = String::from(
            "// Generated by rutis-bridge (build::Exports) from the Rust sources; do not edit.\n",
        );
        let mut services_ts = String::new();
        let mut interfaces: HashMap<(usize, usize), String> = HashMap::new();
        let mut functions = HashSet::new();
        let mut all = Vec::new();
        let mut names = HashSet::new();
        for wanted in &self.services {
            if !names.insert(wanted.name.clone()) {
                return Err(format!(
                    "the service name {} is exported twice",
                    wanted.name
                ));
            }
            let parsed: syn::Path = syn::parse_str(&wanted.path)
                .map_err(|error| format!("{}: {error}", wanted.path))?;
            let (module, index) = match generator.sources.resolve(root, &parsed)? {
                Some(Res::Item { module, index }) => (module, index),
                Some(_) => {
                    return Err(format!(
                        "{} is not a struct or enum of a crate the generator reads",
                        wanted.path
                    ))
                }
                None => {
                    return Err(format!(
                        "{} is not found from the crate being built",
                        wanted.path
                    ))
                }
            };
            let ident = match generator.sources.item(module, index) {
                Item::Struct(item) if item.generics.params.is_empty() => item.ident.to_string(),
                Item::Enum(item) if item.generics.params.is_empty() => item.ident.to_string(),
                Item::Struct(_) | Item::Enum(_) => {
                    return Err(format!(
                        "{}: a generic type cannot be exported",
                        wanted.path
                    ))
                }
                _ => return Err(format!("{} is not a struct or enum", wanted.path)),
            };
            if generator.marked(module, index)?.is_some() {
                return Err(format!(
                    "{} is marked #[rutis::service], so it is served by name when provided: remove its .service(...) line from build.rs",
                    wanted.path
                ));
            }
            let interface = match interfaces.get(&(module, index)) {
                Some(interface) => interface.clone(),
                None => {
                    let (interface, dispatch) =
                        generator.service(module, index, &ident, &wanted.path, false)?;
                    rust.push_str(&dispatch);
                    interfaces.insert((module, index), interface.clone());
                    interface
                }
            };
            let function = rust_ident(&wanted.name);
            if !functions.insert(function.clone()) {
                return Err(format!(
                    "service names collide as the Rust function {function}"
                ));
            }
            let group = wanted
                .group
                .as_ref()
                .map(|group| format!(".in_instances({group:?})"))
                .unwrap_or_default();
            rust.push_str(&format!(
                "/// `{path}`, exported as `{name}`.\npub fn {function}() -> ::rutis_bridge::exports::Export {{\n    ::rutis_bridge::exports::Export::new::<{path}>({name:?}, |service| ::std::sync::Arc::new(Dispatch{interface}(service))){group}\n}}\n",
                path = wanted.path,
                name = wanted.name,
            ));
            all.push(format!("{function}()"));
            services_ts.push_str(&format!("  {}: {interface}\n", ts_key(&wanted.name)));
        }
        // The types marked `#[rutis::service]`: declarations only, since
        // they are served when provided.
        for (module, index, ident, name) in generator.marked_types()? {
            if !names.insert(name.clone()) {
                return Err(format!(
                    "the service name {name} is exported twice (one of them is {ident}, marked #[rutis::service])"
                ));
            }
            let interface = match interfaces.get(&(module, index)) {
                Some(interface) => interface.clone(),
                None => {
                    let (interface, _) = generator.service(module, index, &ident, &ident, true)?;
                    interfaces.insert((module, index), interface.clone());
                    interface
                }
            };
            services_ts.push_str(&format!("  {}: {interface}\n", ts_key(&name)));
        }
        rust.push_str(&format!(
            "/// Every export.\npub fn all() -> ::std::vec::Vec<::rutis_bridge::exports::Export> {{\n    vec![{}]\n}}\n/// The TypeScript declarations of the exported services.\npub const TYPESCRIPT: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/rutis_exports.d.ts\"));\n",
            all.join(", ")
        ));
        let mut typescript = String::from(
            "// Generated by rutis-bridge (build::Exports) from the Rust sources; do not edit.\n\n\
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
        let mut warnings = std::mem::take(&mut generator.sources.warnings);
        warnings.extend(generator.warnings);
        Ok(Generated {
            rust,
            typescript,
            warnings,
        })
    }
}

#[derive(Debug)]
pub(super) struct Generated {
    pub(super) rust: String,
    pub(super) typescript: String,
    pub(super) warnings: Vec<String>,
}

// ── Types ───────────────────────────────────────────────────────

/// A type as it crosses: its TypeScript.
#[derive(Clone, Debug)]
enum Ty {
    Unit,
    Bool,
    Number,
    /// An integer of 64 bits or more: beyond ±(2^53−1) it crosses as a
    /// bigint.
    Big,
    Str,
    Json,
    Option(Box<Ty>),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    Record(Box<Ty>),
    Ts(String),
}

impl Ty {
    fn ts(&self) -> String {
        match self {
            Ty::Unit => "null".into(),
            Ty::Bool => "boolean".into(),
            Ty::Number => "number".into(),
            Ty::Big => "number | bigint".into(),
            Ty::Str => "string".into(),
            Ty::Json => "unknown".into(),
            Ty::Option(inner) => format!("{} | null", inner.ts()),
            Ty::Array(inner) => match inner.as_ref() {
                Ty::Big | Ty::Option(_) => format!("({})[]", inner.ts()),
                Ty::Ts(text) if text.contains(['|', '&', '=']) => format!("({text})[]"),
                _ => format!("{}[]", inner.ts()),
            },
            Ty::Tuple(items) => format!(
                "[{}]",
                items.iter().map(Ty::ts).collect::<Vec<_>>().join(", ")
            ),
            Ty::Record(value) => format!("Record<string, {}>", value.ts()),
            Ty::Ts(text) => text.clone(),
        }
    }

    /// As a method's result: `void` for nothing.
    fn returned(&self) -> String {
        match self {
            Ty::Unit => "void".into(),
            other => other.ts(),
        }
    }
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

/// A parameter that takes a function.
struct Callback {
    args: Vec<Ty>,
    ok: Ty,
    /// An `impl Fn` / generic `F: Fn` (a closure is passed), or else an
    /// `Arc<dyn Fn>` / `Box<dyn Fn>` (built with `FromCallback`).
    closure: bool,
}

/// How a method's `Err(e)` is thrown.
enum Failure {
    /// `e` has serde: data, and the code from its serde form.
    Serde {
        name: String,
        tag: Option<String>,
        /// An enum variant: its serde form names it. A struct's serde form
        /// does not, even when it is one field.
        variant: bool,
    },
    /// An enum serde does not name the variants of: the code from `Debug`.
    Variant {
        name: String,
        data: bool,
    },
    Display {
        name: String,
    },
    Str,
    /// `rutis_bridge::session::Error`: thrown as it is.
    Pass,
}

struct Generator<'a> {
    sources: &'a mut Sources,
    declared: HashMap<Res, String>,
    /// Declared items and their TypeScript names.
    named: HashMap<(usize, usize), String>,
    names: HashSet<String>,
    declarations: Vec<String>,
    /// `<Name>Thrown` declarations, by error type.
    thrown: BTreeMap<String, String>,
    warnings: Vec<String>,
    /// Whether a type alias refers to itself, by alias.
    cyclic: HashMap<(usize, usize), bool>,
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
                Binding::Ts(name) => Ok(Ty::Ts(name)),
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
                if let Some(typescript) = self.declared.get(&res) {
                    return Ok(Ty::Ts(typescript.clone()));
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
                "u64" | "i64" | "u128" | "i128" | "usize" | "isize" => Ty::Big,
                _ => Ty::Number,
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
                        if args.is_empty() {
                            Ok(Ty::Ts(name))
                        } else {
                            let args = args
                                .iter()
                                .map(|arg| self.ty(scope, arg).map(|ty| ty.ts()))
                                .collect::<Result<Vec<_>>>()?;
                            Ok(Ty::Ts(format!("{name}<{}>", args.join(", "))))
                        }
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
            return Ok(Ty::Ts("{ secs: number | bigint; nanos: number }".into()));
        }
        if path_is(path, &[&["std", "time", "SystemTime"]]) {
            return Ok(Ty::Ts(
                "{ secs_since_epoch: number | bigint; nanos_since_epoch: number }".into(),
            ));
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
                "Instant" => Ok(Ty::Ts("Date".into())),
                "BigInt" => Ok(Ty::Ts("bigint".into())),
                "Bytes" => Ok(Ty::Ts("Uint8Array".into())),
                "DateTime" | "Date" | "TimeOfDay" | "Decimal" | "Uuid" => Ok(Ty::Str),
                "ValueMap" => {
                    let (key, value) =
                        (self.arg(scope, args, 0, ty)?, self.arg(scope, args, 1, ty)?);
                    Ok(Ty::Ts(format!("Map<{}, {}>", key.ts(), value.ts())))
                }
                "ValueSet" => Ok(Ty::Ts(format!(
                    "Set<{}>",
                    self.arg(scope, args, 0, ty)?.ts()
                ))),
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
        );
        let declared = self.declare_new(module, index);
        if declared.is_err() {
            self.declarations.truncate(snapshot.0);
            self.named = snapshot.1;
            self.names = snapshot.2;
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
        let body = match &item {
            Item::Struct(item) => self
                .struct_body(&scope, &item.fields, &serde, serde.rename_all.as_deref())
                .map_err(|error| format!("{location}: {ident}: {error}"))?,
            Item::Enum(item) => self
                .enum_body(&scope, item, &serde)
                .map_err(|error| format!("{location}: {ident}: {error}"))?,
            _ => unreachable!(),
        };
        let docs = jsdoc(&attrs, "");
        let declaration = match body {
            Body::Object { fields, flattened } if flattened.is_empty() => {
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
            syn::Fields::Unit => Ok(Body::Alias("null".into())),
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
                    return Ok(Body::Alias(self.ty(scope, &kept[0].ty)?.ts()));
                }
                let items = kept
                    .iter()
                    .map(|field| self.ty(scope, &field.ty).map(|ty| ty.ts()))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Body::Alias(format!("[{}]", items.join(", "))))
            }
            syn::Fields::Named(named) => {
                if serde.transparent {
                    let field = named
                        .named
                        .iter()
                        .find(|field| !SerdeAttrs::of(&field.attrs).is_ok_and(|attrs| attrs.skip))
                        .ok_or("a transparent struct needs a field")?;
                    return Ok(Body::Alias(self.ty(scope, &field.ty)?.ts()));
                }
                let mut fields = Vec::new();
                let mut flattened = Vec::new();
                for field in &named.named {
                    if !self.sources_enabled(scope, &field.attrs) {
                        continue;
                    }
                    let attrs = SerdeAttrs::of(&field.attrs)?;
                    if attrs.skip {
                        continue;
                    }
                    let ident = field.ident.as_ref().expect("a named field").to_string();
                    let ident = ident.strip_prefix("r#").unwrap_or(&ident).to_owned();
                    let ty = self.ty(scope, &field.ty)?;
                    if attrs.flatten {
                        flattened.push(ty.ts());
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
                        key: ts_key(&key),
                        optional,
                        ty: ty.ts(),
                    });
                }
                Ok(Body::Object { fields, flattened })
            }
        }
    }

    fn sources_enabled(&self, scope: &Scope, attrs: &[syn::Attribute]) -> bool {
        self.sources
            .enabled(self.sources.package_of(scope.module), attrs)
    }

    /// What each variant is as serde writes it, and the codes they give.
    fn enum_body(
        &mut self,
        scope: &Scope,
        item: &syn::ItemEnum,
        serde: &SerdeAttrs,
    ) -> Result<Body> {
        let mut alternatives = Vec::new();
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
            // An internally tagged variant's fields sit beside the tag.
            if let (Some(tag), None, false, Some(Body::Object { fields, flattened })) =
                (&serde.tag, &serde.content, serde.untagged, &body)
            {
                let mut fields = fields.clone();
                fields.insert(
                    0,
                    Field {
                        docs: Vec::new(),
                        key: ts_key(tag),
                        optional: false,
                        ty: literal.clone(),
                    },
                );
                let merged = Body::Object {
                    fields,
                    flattened: flattened.clone(),
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
        if alternatives.is_empty() {
            return Ok(Body::Alias("never".into()));
        }
        let long = alternatives.iter().map(String::len).sum::<usize>() > 80
            || alternatives
                .iter()
                .any(|alternative| alternative.starts_with("/**"));
        if long {
            Ok(Body::Alias(
                alternatives
                    .iter()
                    .map(|alternative| format!("\n  | {alternative}"))
                    .collect(),
            ))
        } else {
            Ok(Body::Alias(alternatives.join(" | ")))
        }
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

    // ── Methods ──────────────────────────────────────────────────

    /// The dispatcher and the TypeScript interface of the service type at
    /// `(module, index)`; returns the interface's name and the Rust.
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

    /// The interface of the service type at `module`/`index` and its
    /// dispatcher; `marked`: only its `#[rutis::service]` blocks, without
    /// their `#[service(skip)]` methods.
    fn service(
        &mut self,
        module: usize,
        index: usize,
        ident: &str,
        path: &str,
        marked: bool,
    ) -> Result<(String, String)> {
        let interface = self.unique(ident);
        let item = self.sources.item(module, index).clone();
        let item_attrs = match &item {
            Item::Struct(item) => item.attrs.clone(),
            Item::Enum(item) => item.attrs.clone(),
            _ => Vec::new(),
        };
        let mut methods_ts = String::new();
        let mut arms = String::new();
        let mut shapes = Vec::new();
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
                if marked && marker(&implementation.attrs)?.is_none() {
                    continue;
                }
                for member in &implementation.items {
                    let ImplItem::Fn(method) = member else {
                        continue;
                    };
                    if marked && marker(&method.attrs)?.is_some_and(|options| options.skip) {
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
                    match self.method(&scope, method, path) {
                        Ok((arm, signature)) => {
                            arms.push_str(&format!("            {js:?} => {arm},\n"));
                            methods_ts.push_str(&signature);
                            shapes.push(format!("{js:?}"));
                        }
                        Err(reason) => {
                            seen.remove(&js);
                            self.warnings.push(format!(
                                "{location}: {ident}::{rust_name} is not exported: {reason}"
                            ));
                        }
                    }
                }
            }
        }
        if shapes.is_empty() {
            self.warnings.push(format!(
                "{}: {ident} has no public &self method to export",
                self.sources.location(module, &item)
            ));
        }
        let docs = jsdoc(&item_attrs, "");
        self.declarations.push(format!(
            "{docs}export interface {interface} {{\n{methods_ts}}}\n"
        ));
        let rust = format!(
            r#"#[doc(hidden)]
pub struct Dispatch{interface}(pub ::std::sync::Arc<{path}>);
impl ::rutis_bridge::session::HostDispatch for Dispatch{interface} {{
    fn invoke(&self, method: &str, args: ::rutis_bridge::session::Value) -> ::rutis_bridge::session::Reply {{
        let service = self.0.clone();
        match method {{
{arms}            other => Err(::rutis_bridge::session::Error::Value(format!("{ident} has no method {{other}}"))),
        }}
    }}
    fn methods(&self) -> ::std::option::Option<::rutis_bridge::exports::Json> {{
        ::std::option::Option::Some(::rutis_bridge::exports::shapes(&[{shapes}]))
    }}
}}
"#,
            shapes = shapes.join(", ")
        );
        Ok((interface, rust))
    }

    /// One method: its dispatch arm and its TypeScript signature.
    fn method(
        &mut self,
        scope: &Scope,
        method: &syn::ImplItemFn,
        path: &str,
    ) -> Result<(String, String)> {
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
        let mut decode = String::new();
        let mut call = Vec::new();
        let mut params_ts = Vec::new();
        let mut count = 0;
        let mut takes_ctx = false;
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
            let local = format!("a{count}");
            let js = camel(name.strip_prefix("r#").unwrap_or(&name));
            // The calling row's context: not a parameter for the caller.
            if is_ctx(self, scope, &input.ty)? {
                if matches!(input.ty.as_ref(), Type::Reference(reference) if reference.mutability.is_some())
                {
                    return Err("it takes a mutable reference".into());
                }
                takes_ctx = true;
                call.push(match input.ty.as_ref() {
                    Type::Reference(_) => "&caller".to_owned(),
                    _ => "caller.clone()".to_owned(),
                });
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
                decode.push_str(&format!(
                    "let {local} = ::rutis_bridge::exports::callback(args.next().unwrap())?;\n"
                ));
                if callback.closure {
                    let params: Vec<String> =
                        (0..callback.args.len()).map(|i| format!("p{i}")).collect();
                    let encoded: Vec<String> = params
                        .iter()
                        .map(|param| format!("::rutis_bridge::exports::encode(&{param})"))
                        .collect();
                    call.push(format!(
                        "move |{}| {local}.invoke(vec![{}])",
                        params.join(", "),
                        encoded.join(", ")
                    ));
                } else {
                    call.push(format!(
                        "::rutis_bridge::exports::FromCallback::from_callback({local})"
                    ));
                }
                count += 1;
                continue;
            }
            if let Some(name) = single_ident(&input.ty).filter(|name| bounds.contains_key(name)) {
                return Err(format!("its parameter {name} is generic"));
            }
            let ty = self.ty(scope, &input.ty)?;
            params_ts.push(format!("{js}: {}", ty.ts()));
            let (binding, passed) = self.passing(scope, &input.ty, &local)?;
            decode.push_str(&format!(
                "let {local}{binding} = ::rutis_bridge::exports::decode(args.next().unwrap())?;\n"
            ));
            call.push(passed);
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
        let output = match &sig.output {
            ReturnType::Default => None,
            ReturnType::Type(_, ty) => Some(ty.as_ref().clone()),
        };
        let (awaited, output) = match output {
            None => (sig.asyncness.is_some(), None),
            Some(ty) => match self.future_output(scope, &ty)? {
                Some(inner) => (true, Some(inner)),
                None => (sig.asyncness.is_some(), Some(ty)),
            },
        };
        let (ok, failure, error_ts) = match &output {
            None => (Ty::Unit, None, None),
            Some(ty) => match self.result(scope, ty)? {
                Some((ok, error, error_scope)) => {
                    let ok = self.ty(&error_scope, &ok)?;
                    let (failure, thrown) = self.failure(&error_scope, &error)?;
                    (ok, Some(failure), thrown)
                }
                None => (self.ty(scope, ty)?, None, None),
            },
        };
        let rust_name = sig.ident.to_string();
        let wait = if awaited { ".await" } else { "" };
        let invoke = format!(
            "<{path}>::{rust_name}(&*service{}{}){wait}",
            if call.is_empty() { "" } else { ", " },
            call.join(", ")
        );
        let encode_ok = match ok {
            Ty::Unit => {
                "{ let _ = result; Ok(::rutis_bridge::session::Value::Undefined) }".to_owned()
            }
            _ => "::rutis_bridge::exports::encode(&result)".to_owned(),
        };
        let returned = match &failure {
            None => format!("let result = {invoke}; {encode_ok}"),
            Some(failure) => {
                let thrown = match failure {
                    Failure::Serde { name, tag, variant } => format!(
                        "::rutis_bridge::exports::serde_failure({name:?}, {}, {variant}, &error)",
                        tag.as_ref()
                            .map_or("None".to_owned(), |tag| format!("Some({tag:?})"))
                    ),
                    Failure::Variant { name, data } => format!(
                        "::rutis_bridge::exports::variant_failure({name:?}, &error, {})",
                        if *data {
                            "::rutis_bridge::exports::data(&error)"
                        } else {
                            "None"
                        }
                    ),
                    Failure::Display { name } => {
                        format!("::rutis_bridge::exports::display_failure({name:?}, &error)")
                    }
                    Failure::Str => "::rutis_bridge::exports::string_failure(&error)".to_owned(),
                    Failure::Pass => "error".to_owned(),
                };
                format!(
                    "match {invoke} {{ Ok(result) => {encode_ok}, Err(error) => Err({thrown}) }}"
                )
            }
        };
        let body = format!(
            "let mut args = ::rutis_bridge::exports::arguments(args, {count})?;\n{decode}{returned}"
        );
        // Read while `invoke` runs: the future it returns does not see it.
        let caller = if takes_ctx {
            format!(
                "let caller = ::rutis_bridge::exports::caller_ctx({:?})?;\n",
                camel(&sig.ident.to_string())
            )
        } else {
            String::new()
        };
        let arm = if awaited {
            format!("{{\n{caller}Ok(::rutis_bridge::session::Value::future(async move {{\n{body}\n}}))\n}}")
        } else {
            format!("{{\n{caller}(|| -> ::rutis_bridge::session::Reply {{\n{body}\n}})()\n}}")
        };
        let mut docs = doc_lines(&method.attrs);
        if let Some(thrown) = error_ts {
            docs.push(format!("@throws {{{thrown}}}"));
        }
        let docs = render_jsdoc(&docs, "  ");
        let js = camel(&rust_name);
        let signature = format!(
            "{docs}  {js}({}): Promise<{}>\n",
            params_ts.join(", "),
            ok.returned()
        );
        Ok((arm, signature))
    }

    /// How a parameter is decoded and passed: a type to decode into when
    /// the parameter borrows one inference cannot name, and the argument.
    fn passing(&mut self, scope: &Scope, ty: &Type, local: &str) -> Result<(String, String)> {
        let Type::Reference(reference) = ty else {
            return Ok((String::new(), local.to_owned()));
        };
        if reference.mutability.is_some() {
            return Err("it takes a mutable reference".into());
        }
        Ok(match reference.elem.as_ref() {
            Type::Slice(_) => (": ::std::vec::Vec<_>".into(), format!("&{local}")),
            Type::Path(path) if path.qself.is_none() => {
                match self.resolve_type(scope, &path.path)? {
                    Some(Res::Primitive(name)) if name == "str" => {
                        (": ::std::string::String".into(), format!("&{local}"))
                    }
                    Some(Res::External(found)) if path_is(&found, &[&["std", "path", "Path"]]) => {
                        (": ::std::path::PathBuf".into(), format!("&{local}"))
                    }
                    _ => (String::new(), format!("&{local}")),
                }
            }
            _ => (String::new(), format!("&{local}")),
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

    /// How `Err(e)` of type `error` is thrown, and its `<Name>Thrown`
    /// TypeScript type.
    fn failure(&mut self, scope: &Scope, error: &Type) -> Result<(Failure, Option<String>)> {
        if let Some(Binding::Type(ty, scope)) = Self::binding(scope, error) {
            return self.failure(&scope, &ty);
        }
        let Type::Path(path) = error else {
            return Ok((
                Failure::Display {
                    name: "Error".into(),
                },
                None,
            ));
        };
        let segment = path.path.segments.last().expect("a segment");
        let res = self.resolve_type(scope, &path.path)?;
        // rutis's error keeps the name it had in 0.16 on the TypeScript side
        // too (`CordisErrorThrown`)
        let ident = match &res {
            Some(Res::External(found)) if rutis_error(found) => "CordisError".to_owned(),
            _ => segment.ident.to_string(),
        };
        let (failure, codes, data) = match &res {
            Some(Res::External(found)) if path_is(found, &[&["std", "string", "String"]]) => {
                (Failure::Str, "\"Error\"".to_owned(), "undefined".to_owned())
            }
            Some(Res::External(found))
                if found.first().is_some_and(|k| k == "rutis_bridge")
                    && found.last().is_some_and(|l| l == "Error") =>
            {
                return Ok((Failure::Pass, None));
            }
            // rutis's error (0.16's `CordisError`): thrown as `CordisError`,
            // its code the kind's variant, as before
            Some(Res::External(found)) if rutis_error(found) => (
                Failure::Variant {
                    name: "CordisError".into(),
                    data: false,
                },
                "string".to_owned(),
                "undefined".to_owned(),
            ),
            Some(Res::Item { module, index }) => {
                let (module, index) = (*module, *index);
                match self.sources.item(module, index).clone() {
                    Item::Enum(item) => {
                        let serde = SerdeAttrs::of(&item.attrs)?;
                        let serialized = derives(&item.attrs, "Serialize");
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
                        let failure = if named {
                            Failure::Serde {
                                name: ident.clone(),
                                tag: serde.tag.clone(),
                                variant: true,
                            }
                        } else if derives(&item.attrs, "Debug") {
                            Failure::Variant {
                                name: ident.clone(),
                                data: serialized,
                            }
                        } else {
                            Failure::Display {
                                name: ident.clone(),
                            }
                        };
                        (failure, codes, data)
                    }
                    Item::Struct(item) if derives(&item.attrs, "Serialize") => (
                        Failure::Serde {
                            name: ident.clone(),
                            tag: None,
                            variant: false,
                        },
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
                        return self.failure(&alias_scope, &alias.ty);
                    }
                    _ => (
                        Failure::Display {
                            name: ident.clone(),
                        },
                        format!("{ident:?}"),
                        "undefined".to_owned(),
                    ),
                }
            }
            _ => (
                Failure::Display {
                    name: ident.clone(),
                },
                format!("{ident:?}"),
                "undefined".to_owned(),
            ),
        };
        let name = match &failure {
            Failure::Serde { name, .. }
            | Failure::Variant { name, .. }
            | Failure::Display { name } => name.clone(),
            Failure::Str => "Error".to_owned(),
            Failure::Pass => unreachable!(),
        };
        let thrown_name = format!(
            "{}Thrown",
            if matches!(failure, Failure::Str) {
                "String"
            } else {
                &ident
            }
        );
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
        Ok((failure, Some(thrown_name)))
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
                    Some(bound) => self
                        .callback_signature(scope, bound, bounds, true)
                        .map(Some),
                    None => Ok(None),
                };
            }
        }
        match ty {
            Type::ImplTrait(implemented) => {
                match fn_bound(&implemented.bounds.iter().cloned().collect::<Vec<_>>()) {
                    Some(bound) => self
                        .callback_signature(scope, bound, bounds, true)
                        .map(Some),
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
                        let signature = self.callback_signature(scope, bound, bounds, false)?;
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
        closure: bool,
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
        Ok(Callback { args, ok, closure })
    }
}

enum Body {
    /// The fields of an object, and the types flattened into it.
    Object {
        fields: Vec<Field>,
        flattened: Vec<String>,
    },
    Alias(String),
}

#[derive(Clone)]
struct Field {
    docs: Vec<String>,
    key: String,
    optional: bool,
    ty: String,
}

impl Body {
    /// As a type expression: on one line inside a union (`compact`).
    fn inline(&self, compact: bool) -> String {
        match self {
            Body::Alias(alias) => alias.clone(),
            Body::Object { fields, flattened } => std::iter::once(object(fields, compact))
                .chain(flattened.iter().cloned())
                .collect::<Vec<_>>()
                .join(" & "),
        }
    }
}

/// An object type: one field a line with its docs, or on one line.
fn object(fields: &[Field], compact: bool) -> String {
    let field = |field: &Field| {
        format!(
            "{}{}: {}",
            field.key,
            if field.optional { "?" } else { "" },
            field.ty
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
}

impl SerdeAttrs {
    fn of(attrs: &[syn::Attribute]) -> Result<Self> {
        let mut found = Self::default();
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            attr.parse_nested_meta(|meta| {
                let key = meta.path.get_ident().map(ToString::to_string).unwrap_or_default();
                // `rename = "x"` or `rename(serialize = "x", ...)`.
                let text = |meta: &syn::meta::ParseNestedMeta| -> syn::Result<Option<String>> {
                    if meta.input.peek(syn::Token![=]) {
                        return Ok(Some(meta.value()?.parse::<syn::LitStr>()?.value()));
                    }
                    let mut serialized = None;
                    meta.parse_nested_meta(|inner| {
                        let value = inner.value()?.parse::<syn::LitStr>()?.value();
                        if inner.path.is_ident("serialize") {
                            serialized = Some(value);
                        }
                        Ok(())
                    })?;
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

/// A Rust function name for a service name.
fn rust_ident(name: &str) -> String {
    let mut ident: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if ident.is_empty()
        || ident.starts_with(|c: char| c.is_ascii_digit())
        || matches!(
            ident.as_str(),
            "all"
                | "self"
                | "super"
                | "crate"
                | "type"
                | "fn"
                | "mod"
                | "use"
                | "struct"
                | "enum"
                | "impl"
                | "trait"
                | "move"
                | "match"
                | "loop"
                | "ref"
                | "static"
                | "const"
                | "where"
                | "async"
                | "await"
                | "dyn"
        )
    {
        ident.insert(0, '_');
    }
    ident
}

/// A property key in TypeScript: quoted unless it is an identifier.
fn ts_key(key: &str) -> String {
    let identifier = key
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    if identifier {
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
        assert_eq!(rust_ident("calendar-v2"), "calendar_v2");
        assert_eq!(rust_ident("all"), "_all");
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
                lib("other", "other_crate", dir.path().join("other/lib.rs")),
            ],
            "resolve": { "root": "host", "nodes": [
                { "id": "host", "deps": [{ "name": "dep_crate", "pkg": "dep" }, { "name": "rutis", "pkg": "rutis" }], "features": [] },
                { "id": "dep", "deps": [{ "name": "rutis", "pkg": "rutis" }, { "name": "serde", "pkg": "serde" }], "features": ["extra"] },
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

    /// Types marked `#[rutis::service]` are declared with no list: only
    /// their marked blocks, without their skipped methods, and no Rust is
    /// generated for them (they are served when provided).
    #[test]
    fn marked_types_are_declared_without_a_list() {
        let generated = generate(Exports::new(), &[("lib.rs", MARKED)]).unwrap();
        let ts = &generated.typescript;
        assert!(ts.contains("  clock: Clock\n"), "{ts}");
        assert!(ts.contains("  board: Named\n"), "{ts}");
        assert!(ts.contains("now(): Promise<number>"), "{ts}");
        assert!(ts.contains("explicit(): Promise<number>"), "{ts}");
        // `self: &Self` is `&self`: no warning that it cannot be bound.
        assert!(generated.warnings.is_empty(), "{:?}", generated.warnings);
        assert!(ts.contains("read(): Promise<string>"), "{ts}");
        assert!(!ts.contains("unmarked") && !ts.contains("reset"), "{ts}");
        assert!(!generated.rust.contains("Clock"), "{}", generated.rust);
    }

    /// Marking a type writes exactly the declarations listing it wrote
    /// (§8): the same type, its blocks marked (the methods build.rs left
    /// out skipped, as the marker requires), against `.service(..)`.
    #[test]
    fn a_marked_type_is_declared_as_listing_it_was() {
        let listed = store().unwrap().typescript;
        let marked_inner = INNER
            .replace(
                "        impl Store {",
                "        #[rutis::service(name = \"store\")]\n        impl Store {",
            )
            .replace(
                "            impl super::Store {",
                "            #[rutis::service]\n            impl super::Store {",
            )
            .replace(
                "pub fn reset(&mut self)",
                "#[service(skip)] pub fn reset(&mut self)",
            )
            .replace("pub fn borrowing(", "#[service(skip)] pub fn borrowing(");
        let marked = generate(
            Exports::new(),
            &[
                ("lib.rs", LIB),
                ("inner.rs", &marked_inner),
                ("types.rs", TYPES),
            ],
        )
        .unwrap()
        .typescript;
        assert_eq!(marked, listed);
    }

    /// A marked type listed in build.rs as well is an error naming the fix.
    #[test]
    fn a_marked_type_listed_too_is_an_error() {
        let error = generate(
            Exports::new().service("clock", "dep_crate::Clock"),
            &[("lib.rs", MARKED)],
        )
        .unwrap_err();
        assert!(error.contains("remove its .service(...) line"), "{error}");
        let error = generate(
            Exports::new().service("board", "dep_crate::Other"),
            &[(
                "lib.rs",
                &format!("{MARKED}\npub struct Other;\nimpl Other {{ pub fn a(&self) {{}} }}"),
            )],
        )
        .unwrap_err();
        assert!(error.contains("exported twice"), "{error}");
    }

    fn store() -> Result<Generated> {
        generate(
            Exports::new().service("store", "dep_crate::Store"),
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
        let rust = &generated.rust;
        assert!(
            rust.contains("<dep_crate::Store>::find(&*service, &a0, a1).await"),
            "{rust}"
        );
        assert!(rust.contains("let a0: ::std::string::String"), "{rust}");
        assert!(
            rust.contains("serde_failure(\"Problem\", Some(\"kind\"), true, &error)"),
            "{rust}"
        );
        assert!(
            rust.contains("serde_failure(\"Single\", None, false, &error)"),
            "{rust}"
        );
        assert!(rust.contains("string_failure(&error)"), "{rust}");
        assert!(
            rust.contains("let caller = ::rutis_bridge::exports::caller_ctx(\"tied\")?;"),
            "{rust}"
        );
        assert!(
            rust.contains("<dep_crate::Store>::tied(&*service, &caller, a0)"),
            "{rust}"
        );
        assert!(rust.contains("move |p0, p1| a0.invoke("), "{rust}");
        assert!(
            rust.contains("Export::new::<dep_crate::Store>(\"store\""),
            "{rust}"
        );
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
             impl Store { pub fn opaque(&self) -> Opaque { Opaque } pub fn wrapped(&self) -> Wrapper { todo!() } }",
        );
        let files = [
            ("lib.rs", LIB),
            ("inner.rs", inner.as_str()),
            ("types.rs", TYPES),
        ];
        let generated =
            generate(Exports::new().service("store", "dep_crate::Store"), &files).unwrap();
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
            Exports::new()
                .service("store", "dep_crate::Store")
                .declared("dep_crate::Opaque", "import('./opaque').Opaque"),
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

    #[test]
    fn names_that_are_not_services_are_refused() {
        let files = [("lib.rs", LIB), ("inner.rs", INNER), ("types.rs", TYPES)];
        let error =
            generate(Exports::new().service("x", "dep_crate::Nothing"), &files).unwrap_err();
        assert!(error.contains("not found"), "{error}");
        let error = generate(
            Exports::new().service("x", "dep_crate::types::Page"),
            &files,
        )
        .unwrap_err();
        assert!(error.contains("generic"), "{error}");
        let error = generate(
            Exports::new()
                .service("x", "dep_crate::Store")
                .service("x", "dep_crate::Store"),
            &files,
        )
        .unwrap_err();
        assert!(error.contains("twice"), "{error}");
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
        let rust = &generated.rust;
        assert!(rust.contains("string_failure(&error)"), "{rust}");
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
        let generated = generate_quickly(
            Exports::new().service("service", "dep_crate::Service"),
            &files,
        );
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
            Exports::new().service("service", "dep_crate::Service"),
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
            Exports::new().service("service", "dep_crate::Service"),
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
                    lib("serde", "serde", "none.rs"),
                ],
                "resolve": { "root": "host", "nodes": [
                    { "id": "host", "deps": [{ "name": "a_crate", "pkg": "a" }], "features": [] },
                    { "id": "a", "deps": [{ "name": "b_crate", "pkg": "b" }, { "name": "serde", "pkg": "serde" }], "features": [] },
                    { "id": "b", "deps": [{ "name": "a_crate", "pkg": "a" }, { "name": "serde", "pkg": "serde" }], "features": [] },
                ] },
            });
            let mut sources = Sources::from_metadata(&metadata, "host").unwrap();
            let started = std::time::Instant::now();
            let generated = Exports::new()
                .service("service", "a_crate::Service")
                .generate_from(&mut sources)
                .unwrap();
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
            Exports::new().service("service", "dep_crate::Service"),
            &[
                ("lib.rs", "mod m1; pub use m1::*;"),
                (
                    "m1.rs",
                    &format!(
                        "pub type A = B;\npub type B = A;\npub type Pair<T> = (T, T);\n\
                         #[derive(serde::Serialize)] pub struct Item6;\n\
                         impl Service {{ pub fn bad(&self) -> A {{ todo!() }} \
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
            Exports::new().service("service", "dep_crate::Service"),
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
                     impl Service {\n\
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
            Exports::new().service("service", "dep_crate::Service"),
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
                     impl Service { pub fn one(&self) -> Res<Thing> { todo!() } }",
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
            Exports::new().service("service", "dep_crate::Service"),
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
