use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{FnArg, GenericArgument, ImplItem, Item, PathArguments, ReturnType, Type, Visibility};

type Result<T> = std::result::Result<T, syn::Error>;

fn unsupported(node: &impl ToTokens, reason: &str) -> syn::Error {
    syn::Error::new_spanned(node, reason)
}

fn camel(name: &str) -> String {
    let mut words = name.split('_');
    let mut result = words.next().unwrap_or_default().to_owned();
    for word in words {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            result.extend(first.to_uppercase());
            result.extend(chars);
        }
    }
    result
}

/// A type the generator binds: its TypeScript type, and what is checked
/// before a value of it crosses.
enum Shape {
    Void,
    Number,
    Boolean,
    String,
    Instant,
    BigInt,
    Bytes,
    Map(Box<Shape>, Box<Shape>),
    Set(Box<Shape>),
    Array(Box<Shape>),
}

impl Shape {
    fn ts(&self) -> String {
        match self {
            Self::Void => "void".into(),
            Self::Number => "number".into(),
            Self::Boolean => "boolean".into(),
            Self::String => "string".into(),
            Self::Instant => "Date".into(),
            Self::BigInt => "bigint".into(),
            Self::Bytes => "Uint8Array".into(),
            Self::Map(key, value) => format!("Map<{}, {}>", key.ts(), value.ts()),
            Self::Set(element) => format!("Set<{}>", element.ts()),
            Self::Array(element) => format!("{}[]", element.ts()),
        }
    }

    /// Statements refusing `value` where crossing would lose information:
    /// a NaN or an infinity would become `null`, wherever it is nested,
    /// map keys and values and set elements included (#409).
    fn validate(&self, value: &str) -> String {
        match self {
            Self::Number => format!("if !{value}.is_finite() {{ return Err(::rutis_bridge::cordis::Error::Value(\"non-finite number\".into())); }}"),
            Self::Array(element) | Self::Set(element) => {
                let check = element.validate("item");
                let items = if matches!(self, Self::Set(_)) { ".0" } else { "" };
                if check.is_empty() {
                    check
                } else {
                    format!("for item in {value}{items}.iter() {{ {check} }}")
                }
            }
            Self::Map(key, item) => {
                let (key, item) = (key.validate("key"), item.validate("item"));
                if key.is_empty() && item.is_empty() {
                    return String::new();
                }
                let bind = |check: &str, name| if check.is_empty() { "_" } else { name };
                format!(
                    "for ({}, {}) in {value}.0.iter() {{ {key} {item} }}",
                    bind(&key, "key"),
                    bind(&item, "item")
                )
            }
            _ => String::new(),
        }
    }
}

/// The modules an entry source may name rutis' tagged value types (#308)
/// in; `rutis_bridge::cordis::rpc` is `rutis_bridge::session`.
const VALUE_MODULES: [&[&str]; 4] = [
    &["rutis_bridge", "session"],
    &["rutis_bridge", "session", "values"],
    &["rutis_bridge", "cordis", "rpc"],
    &["rutis_bridge", "cordis", "rpc", "values"],
];

/// The name of the tagged value type at `path`, a path from the crate root.
fn tagged(path: &[String]) -> Option<&'static str> {
    let (name, module) = path.split_last()?;
    if !VALUE_MODULES.iter().any(|known| known.iter().eq(module)) {
        return None;
    }
    // `values` has the scalar ones; `session` has them all.
    let session = module.last().is_some_and(|last| last != "values");
    match name.as_str() {
        "Instant" => Some("Instant"),
        "BigInt" => Some("BigInt"),
        "Bytes" if session => Some("Bytes"),
        "ValueMap" if session => Some("ValueMap"),
        "ValueSet" if session => Some("ValueSet"),
        _ => None,
    }
}

/// What the names of an entry source stand for: its `use` items and its
/// own declarations. Tagged value types are known by the rutis path a name
/// resolves to, not by the name (#408): a user's own `type Instant = ...`
/// is not rutis' `Instant`.
#[derive(Default)]
struct Names {
    imported: HashMap<String, Vec<String>>,
    globs: Vec<Vec<String>>,
    declared: HashSet<String>,
}

impl Names {
    fn of(file: &syn::File) -> Self {
        let mut names = Self::default();
        for item in &file.items {
            let declared = match item {
                Item::Use(item) => {
                    names.import(Vec::new(), &item.tree);
                    continue;
                }
                Item::Type(item) => &item.ident,
                Item::Struct(item) => &item.ident,
                Item::Enum(item) => &item.ident,
                Item::Union(item) => &item.ident,
                Item::Trait(item) => &item.ident,
                _ => continue,
            };
            names.declared.insert(declared.to_string());
        }
        names
    }

    fn import(&mut self, mut prefix: Vec<String>, tree: &syn::UseTree) {
        let (ident, alias) = match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                return self.import(prefix, &path.tree);
            }
            syn::UseTree::Glob(_) => return self.globs.push(prefix),
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.import(prefix.clone(), tree);
                }
                return;
            }
            syn::UseTree::Name(item) => (&item.ident, None),
            syn::UseTree::Rename(item) => (&item.ident, Some(&item.rename)),
        };
        if ident != "self" {
            prefix.push(ident.to_string());
        }
        let name = alias.map_or_else(|| prefix.last().cloned(), |alias| Some(alias.to_string()));
        if let Some(name) = name.filter(|name| name != "_") {
            self.imported.insert(name, prefix);
        }
    }

    /// The tagged value type `path` names, if it names one: written from
    /// the crate root, or through what the source imports.
    fn tagged(&self, path: &syn::Path) -> Option<&'static str> {
        let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let (first, rest) = segments.split_first()?;
        if path.leading_colon.is_some() {
            return tagged(&segments);
        }
        if let Some(imported) = self.imported.get(first) {
            return tagged(&[imported.as_slice(), rest].concat());
        }
        if self.declared.contains(first) {
            return None;
        }
        // From the crate root, or else through a glob import.
        tagged(&segments).or_else(|| {
            self.globs
                .iter()
                .find_map(|glob| tagged(&[glob.as_slice(), &segments].concat()))
        })
    }

    fn shape(&self, ty: &Type) -> Result<Shape> {
        let path = match ty {
            Type::Tuple(tuple) if tuple.elems.is_empty() => return Ok(Shape::Void),
            Type::Path(path) if path.qself.is_none() => &path.path,
            _ => return Err(unsupported(ty, "type binding not implemented")),
        };
        let segment = path.segments.last().unwrap();
        // The type arguments of `name<...>`, if it has `count`.
        let arguments = |count: usize| match &segment.arguments {
            PathArguments::AngleBracketed(args) if args.args.len() == count => args
                .args
                .iter()
                .map(|arg| match arg {
                    GenericArgument::Type(inner) => self.shape(inner).map(Box::new),
                    _ => Err(unsupported(ty, "unsupported type argument")),
                })
                .collect::<Result<Vec<_>>>(),
            _ => Err(unsupported(ty, "unsupported type arguments")),
        };
        if path.leading_colon.is_none() && path.segments.len() == 1 {
            match segment.ident.to_string().as_str() {
                "f64" => return Ok(Shape::Number),
                "bool" => return Ok(Shape::Boolean),
                "String" => return Ok(Shape::String),
                "Vec" => return Ok(Shape::Array(arguments(1)?.remove(0))),
                _ => {}
            }
        }
        // Tagged values (#308), as rutis_bridge::session names them.
        match self.tagged(path) {
            Some("Instant") => Ok(Shape::Instant),
            Some("BigInt") => Ok(Shape::BigInt),
            Some("Bytes") => Ok(Shape::Bytes),
            Some("ValueMap") => {
                let mut types = arguments(2)?;
                let value = types.remove(1);
                Ok(Shape::Map(types.remove(0), value))
            }
            Some("ValueSet") => Ok(Shape::Set(arguments(1)?.remove(0))),
            _ => Err(unsupported(ty, "type binding not implemented")),
        }
    }
}

#[derive(Default)]
struct Registrations {
    services: BTreeSet<String>,
    error: Option<syn::Error>,
}

impl<'ast> Visit<'ast> for Registrations {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "provide" {
            let ty = match node.args.first() {
                Some(syn::Expr::Call(call)) => match call.func.as_ref() {
                    syn::Expr::Path(path) if path.path.segments.len() == 2 => {
                        Some(path.path.segments[0].ident.to_string())
                    }
                    _ => None,
                },
                Some(syn::Expr::Struct(value)) => value.path.get_ident().map(ToString::to_string),
                _ => None,
            };
            if let Some(ty) = ty {
                self.services.insert(ty);
            } else {
                self.error = Some(unsupported(
                    node,
                    "service discovery currently requires a concrete constructor or struct literal",
                ));
            }
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

fn generate(
    source: &str,
    crate_name: &str,
    node_package: &Path,
) -> Result<(String, String, String)> {
    let file = syn::parse_file(source)?;
    let types = Names::of(&file);
    for item in &file.items {
        if matches!(item, Item::Mod(_) | Item::Macro(_)) {
            return Err(unsupported(
                item,
                "module and macro expansion are required before discovering this interface",
            ));
        }
    }
    let implementations: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Impl(item)
                if item.trait_.as_ref().is_some_and(|(_, path, _)| {
                    // the plugin trait (`rutis::Plugin`) or the object-safe
                    // one (`rutis::host::DynPlugin`)
                    path.segments
                        .last()
                        .is_some_and(|s| s.ident == "Plugin" || s.ident == "DynPlugin")
                }) =>
            {
                Some(item)
            }
            _ => None,
        })
        .collect();
    if implementations.len() != 1 {
        return Err(syn::Error::new(
            file.span(),
            "expected one native Plugin implementation in the entry source",
        ));
    }
    let implementation = implementations[0];
    let plugin_name = implementation.self_ty.to_token_stream().to_string();
    let plugin = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Struct(item) if item.ident == plugin_name => Some(item),
            _ => None,
        })
        .ok_or_else(|| {
            unsupported(
                implementation,
                "plugin must be a concrete struct in the entry source",
            )
        })?;
    if !plugin.generics.params.is_empty() || !matches!(plugin.vis, Visibility::Public(_)) {
        return Err(unsupported(plugin, "plugin must be public and non-generic"));
    }
    let mut config_rust = String::new();
    let mut config_ts = String::new();
    let mut config_fields = Vec::new();
    for field in &plugin.fields {
        let name = field
            .ident
            .as_ref()
            .ok_or_else(|| unsupported(field, "named configuration fields required"))?;
        if !matches!(field.vis, Visibility::Public(_)) {
            return Err(unsupported(
                field,
                "configuration construction requires public fields",
            ));
        }
        let shape = types.shape(&field.ty)?;
        let ty = shape.ts();
        let key = camel(&name.to_string());
        config_ts.push_str(&format!("{key}: {ty};\n"));
        config_rust.push_str(&format!("let {name}: {} = ::rutis_bridge::cordis::decode(config.get({key:?}).cloned().ok_or_else(|| ::rutis_bridge::cordis::Error::Value({key:?}.into()))?)?; {}\n", field.ty.to_token_stream(), shape.validate(&name.to_string())));
        config_fields.push(name.to_string());
    }
    let mut registrations = Registrations::default();
    registrations.visit_item_impl(implementation);
    if let Some(error) = registrations.error {
        return Err(error);
    }
    if registrations.services.is_empty() {
        return Err(unsupported(
            implementation,
            "no native service registrations found",
        ));
    }
    let mut arms = String::new();
    let mut classes = String::new();
    let mut declarations = String::new();
    let mut provide = String::new();
    let mut context = String::new();
    let mut fields = String::new();
    let mut capture = String::new();
    let mut dependencies = Vec::new();
    for service in registrations.services {
        let field = format!("service_{}", dependencies.len());
        let declaration = file
            .items
            .iter()
            .find_map(|item| match item {
                Item::Struct(item) if item.ident == service => Some(item),
                _ => None,
            })
            .ok_or_else(|| {
                unsupported(
                    implementation,
                    "service must be declared in the entry source",
                )
            })?;
        if !matches!(declaration.vis, Visibility::Public(_))
            || !declaration.generics.params.is_empty()
        {
            return Err(unsupported(
                declaration,
                "service must be public and non-generic",
            ));
        }
        if declaration
            .fields
            .iter()
            .any(|field| matches!(field.vis, Visibility::Public(_)))
        {
            return Err(unsupported(
                declaration,
                "public field binding not implemented",
            ));
        }
        let mut service_chars = service.chars();
        let key = service_chars.next().unwrap().to_lowercase().to_string() + service_chars.as_str();
        let mut methods_js = String::new();
        let mut methods_ts = String::new();
        let mut names = BTreeSet::new();
        for item in &file.items {
            let Item::Impl(item) = item else { continue };
            if item.trait_.is_some() || item.self_ty.to_token_stream().to_string() != service {
                continue;
            }
            for item in &item.items {
                if matches!(item, ImplItem::Macro(_)) {
                    return Err(unsupported(
                        item,
                        "macro-expanded service interfaces require further binding support",
                    ));
                }
                let ImplItem::Fn(method) = item else { continue };
                if !matches!(method.vis, Visibility::Public(_)) {
                    continue;
                }
                let sig = &method.sig;
                if !sig.generics.params.is_empty() || sig.unsafety.is_some() {
                    return Err(unsupported(
                        sig,
                        "generic or unsafe method binding not implemented",
                    ));
                }
                if !matches!(sig.inputs.first(), Some(FnArg::Receiver(receiver)) if receiver.reference.is_some() && receiver.mutability.is_none())
                {
                    return Err(unsupported(sig, "service methods currently require &self"));
                }
                let rust_name = sig.ident.to_string();
                let js_name = camel(&rust_name);
                if !names.insert(js_name.clone()) {
                    return Err(unsupported(sig, "method names collide in TypeScript"));
                }
                let mut args_rust = Vec::new();
                let mut args_js = Vec::new();
                let mut params_ts = Vec::new();
                let mut decode = String::new();
                for arg in sig.inputs.iter().skip(1) {
                    let FnArg::Typed(arg) = arg else {
                        unreachable!()
                    };
                    let syn::Pat::Ident(name) = arg.pat.as_ref() else {
                        return Err(unsupported(arg, "named parameters required"));
                    };
                    let name = &name.ident;
                    let js = camel(&name.to_string());
                    let shape = types.shape(&arg.ty)?;
                    let ty = shape.ts();
                    decode.push_str(&format!(
                        "let {name}: {} = ::rutis_bridge::cordis::decode_value(args[{}].clone())?; {}\n",
                        arg.ty.to_token_stream(),
                        args_rust.len(),
                        shape.validate(&name.to_string())
                    ));
                    args_rust.push(name.to_string());
                    args_js.push(js.clone());
                    params_ts.push(format!("{js}: {ty}"));
                }
                let unit: Type = syn::parse_quote!(());
                let mut output = match &sig.output {
                    ReturnType::Default => &unit,
                    ReturnType::Type(_, ty) => ty,
                };
                let mut fallible = false;
                if let Type::Path(path) = output {
                    if let Some(segment) = path
                        .path
                        .segments
                        .last()
                        .filter(|segment| segment.ident == "Result")
                    {
                        if let PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(GenericArgument::Type(ty)) = args.args.first() {
                                output = ty;
                                fallible = true;
                            }
                        }
                    }
                }
                let output_shape = types.shape(output)?;
                let output_ts = output_shape.ts();
                let wait = if sig.asyncness.is_some() {
                    ".await"
                } else {
                    ""
                };
                let error = if fallible {
                    ".map_err(::rutis_bridge::cordis::server::native_error)?"
                } else {
                    ""
                };
                let wrong_arity = if args_rust.is_empty() {
                    "!args.is_empty()".to_owned()
                } else {
                    format!("args.len() != {}", args_rust.len())
                };
                let invoke = format!("service.{rust_name}({}){wait}{error}", args_rust.join(", "));
                let returned = if output_ts == "void" {
                    format!("{invoke}; Ok(::rutis_bridge::cordis::rpc::Value::Undefined)")
                } else {
                    format!(
                        "let result: {} = {invoke}; {} ::rutis_bridge::cordis::arg(&result)",
                        output.to_token_stream(),
                        output_shape.validate("result")
                    )
                };
                let returned = if sig.asyncness.is_some() {
                    format!("Ok(::rutis_bridge::cordis::rpc::Value::future(async move {{ {returned} }}))")
                } else {
                    returned
                };
                arms.push_str(&format!(r#"({key:?}, {rust_name:?}) => {{
                    let args = args.list()?;
                    if {wrong_arity} {{ return Err(::rutis_bridge::cordis::Error::Value("wrong argument count".into())); }}
                    {decode}
                    let service = self.{field}.clone();
                    {returned}
                }},"#));
                let async_js = if sig.asyncness.is_some() {
                    "async "
                } else {
                    ""
                };
                let call = if sig.asyncness.is_some() {
                    "callAsync"
                } else {
                    "call"
                };
                let expression = format!(
                    "this.#process.{call}({key:?}, {rust_name:?}, [{}])",
                    args_js.join(", ")
                );
                let returned = if output_ts == "void" {
                    format!(
                        "{}{expression};",
                        if sig.asyncness.is_some() {
                            "await "
                        } else {
                            ""
                        }
                    )
                } else {
                    format!("return {expression};")
                };
                methods_js.push_str(&format!(
                    "{async_js}{js_name}({}) {{ {returned} }}\n",
                    args_js.join(", ")
                ));
                let returned = if sig.asyncness.is_some() {
                    format!("Promise<{output_ts}>")
                } else {
                    output_ts
                };
                methods_ts.push_str(&format!(
                    "{js_name}({}): {returned};\n",
                    params_ts.join(", ")
                ));
            }
        }
        if names.is_empty() {
            return Err(unsupported(
                declaration,
                "service has no supported public methods",
            ));
        }
        classes.push_str(&format!("export class {service} {{ #process; constructor(process) {{ this.#process = process; }} {methods_js} }}\n"));
        declarations.push_str(&format!(
            "export declare class {service} {{ private constructor(); {methods_ts} }}\n"
        ));
        context.push_str(&format!("{key}: {service};\n"));
        provide.push_str(&format!(
            "yield ctx.provide({key:?}, new {service}(process));\n"
        ));
        fields.push_str(&format!(
            "{field}: ::std::sync::Arc<{crate_name}::{service}>,\n"
        ));
        capture.push_str(&format!(
            "{field}: ::rutis::host::CtxHost::require::<{crate_name}::{service}>(ctx)?,\n"
        ));
        dependencies.push(format!(
            "::rutis::host::TypeKey::of::<{crate_name}::{service}>()"
        ));
    }
    let rust = format!(
        r#"
    struct Exports {{ {fields} }}
    struct Exporter {{
        dependencies: Vec<::rutis::host::TypeKey>,
        exports: ::std::sync::Arc<::std::sync::Mutex<Option<Exports>>>,
    }}
    impl ::rutis::host::DynPlugin for Exporter {{
        fn name(&self) -> &str {{ "interop-export" }}
        fn injects(&self) -> Vec<::rutis::host::TypeKey> {{ self.dependencies.clone() }}
        fn apply<'a>(&'a self, ctx: &'a ::rutis::Ctx) -> ::rutis::host::BoxFuture<'a, ::rutis::Result<::rutis::host::Effect>> {{
            Box::pin(async move {{
                *self.exports.lock().unwrap() = Some(Exports {{ {capture} }});
                Ok(::rutis::host::Effect::Done)
            }})
        }}
    }}
    impl ::rutis_bridge::cordis::server::Dispatch for Exports {{
        fn invoke(&self, target: &str, method: &str, args: ::rutis_bridge::cordis::rpc::Value) -> ::rutis_bridge::cordis::rpc::Reply {{
            match (target, method) {{ {arms}
                _ => Err(::rutis_bridge::cordis::Error::Value("unknown service method".into()))
            }}
        }}
    }}
    async fn mount(ctx: ::rutis::Ctx, config: ::rutis_bridge::cordis::serde_json::Value) -> Result<Exports, ::rutis_bridge::cordis::Error> {{
            {config_rust}
            let mounted = ctx.plugin({crate_name}::{plugin_name} {{ {} }});
            (&mounted).await.map_err(::rutis_bridge::cordis::server::native_error)?;
            let exports = ::std::sync::Arc::new(::std::sync::Mutex::new(None));
            let exporter = ctx.plugin(Exporter {{ dependencies: vec![{}], exports: exports.clone() }});
            (&exporter).await.map_err(::rutis_bridge::cordis::server::native_error)?;
            let exports = exports.lock().unwrap().take().ok_or_else(|| ::rutis_bridge::cordis::Error::Value("native service dependencies are unresolved".into()))?;
            Ok(exports)
    }}
    async fn run() -> Result<(), ::rutis_bridge::cordis::Error> {{
        ::rutis_bridge::cordis::server::serve(mount).await
    }}"#,
        config_fields.join(", "),
        dependencies.join(", ")
    );
    let import =
        serde_json::to_string(&node_package.join("src/client.mjs").to_string_lossy()).unwrap();
    let js = format!(
        r#"import {{ Process }} from {import};
    {classes}
    export function plugin(executable) {{
        return {{ name: "rutis:{plugin_name}", async apply(ctx, config) {{
            const process = await Process.launch(executable, config);
            ctx.effect(function* () {{
                yield () => process.dispose();
                {provide}
            }});
        }} }};
    }}"#
    );
    let cordis_types = serde_json::to_string(
        &node_package
            .join("node_modules/@deepseek-ai/cordis/lib/types/index.d.ts")
            .to_string_lossy(),
    )
    .unwrap();
    let dts = format!(
        r#"import type {{ Context }} from {cordis_types};
    export interface Config {{ {config_ts} }}
    {declarations}
    declare module {cordis_types} {{ interface Context {{ {context} }} }}
    export declare function plugin(executable: string): {{ name: string; apply(ctx: Context, config: Config): Promise<void> }};
    "#
    );
    Ok((rust, js, dts))
}

/// Generate a native Cordis mount and a compiled dispatcher for a Rust entry source.
/// This initial source adapter supports concrete public plugins and value methods;
/// Cargo checks every generated call against the original crate's actual types.
/// Tagged value types (`Instant`, `BigInt`, `Bytes`, `ValueMap`, `ValueSet`) are
/// known by their `rutis_bridge::session` path: imported with `use`, or written
/// in full; a type of the source's own with one of their names is not one.
pub fn rutis_plugin(
    source: impl AsRef<Path>,
    crate_name: &str,
    node_package: impl AsRef<Path>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let source = source.as_ref().canonicalize()?;
    let node_package = node_package.as_ref().canonicalize()?;
    syn::parse_str::<syn::Ident>(crate_name)?;
    println!("cargo:rerun-if-changed={}", source.display());
    let (rust, js, dts) = generate(
        &std::fs::read_to_string(&source)?,
        crate_name,
        &node_package,
    )
    .map_err(|error| {
        let start = error.span().start();
        format!(
            "{}:{}:{}: {error}",
            source.display(),
            start.line,
            start.column + 1
        )
    })?;
    let output = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(output.join("rutis.rs"), rust)?;
    std::fs::write(output.join("rutis.mjs"), js)?;
    std::fs::write(output.join("rutis.d.mts"), dts)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGIN: &str = include_str!("../../../../../examples/native-mount/src/lib.rs");

    #[test]
    fn rejects_borrowed_results_instead_of_copying_remote_state() {
        let source = PLUGIN.replace(
            "pub fn current(&self) -> f64",
            "pub fn current(&self) -> &f64",
        );
        let error = generate(&source, "fixture", Path::new("/node")).unwrap_err();
        assert!(error.to_string().contains("type binding not implemented"));
        assert!(error.span().start().line > 1);
    }

    /// Tagged values (#308) are typed for TypeScript and cross through
    /// `arg` / `decode_value`, not as JSON.
    #[test]
    fn tagged_value_types_become_their_typescript_types() {
        let source = with_current(
            "use rutis_bridge::session::{BigInt, Bytes, Instant, ValueMap, ValueSet};",
            "at: Instant, raw: Bytes, tags: ValueSet<String>, \
             counts: ValueMap<String, f64>, big: BigInt",
        );
        let (rust, _, dts) = generate(&source, "fixture", Path::new("/node")).unwrap();
        assert!(
            dts.contains(
                "current(at: Date, raw: Uint8Array, tags: Set<string>, \
                 counts: Map<string, number>, big: bigint): number;"
            ),
            "{dts}"
        );
        assert!(
            rust.contains(
                "let at: Instant = ::rutis_bridge::cordis::decode_value(args[0].clone())?;"
            ),
            "{rust}"
        );
        assert!(
            rust.contains("::rutis_bridge::cordis::arg(&result)"),
            "{rust}"
        );
    }

    /// The entry source with `import` added and `current` taking `params`.
    fn with_current(import: &str, params: &str) -> String {
        let source = PLUGIN.replace(
            "pub fn current(&self) -> f64",
            &format!("pub fn current(&self, {params}) -> f64"),
        );
        format!("{source}\n{import}")
    }

    /// The TypeScript signature of `current` generated for `source`.
    fn current_ts(source: &str) -> Result<String> {
        let (_, _, dts) = generate(source, "fixture", Path::new("/node"))?;
        let start = dts.find("current(").unwrap();
        Ok(dts[start..start + dts[start..].find(';').unwrap()].to_owned())
    }

    /// Tagged value types are known by the rutis path they resolve to (#408):
    /// imported from either module rutis names them in, renamed, through a
    /// glob or written in full.
    #[test]
    fn tagged_value_types_are_known_by_their_rutis_path() {
        for (import, params) in [
            ("use rutis_bridge::session::values::Instant;", "at: Instant"),
            (
                "use rutis_bridge::cordis::rpc::{self, Instant as When};",
                "at: When",
            ),
            ("use rutis_bridge::cordis::rpc;", "at: rpc::Instant"),
            ("use rutis_bridge::session::*;", "at: Instant"),
            ("use rutis_bridge::*;", "at: session::Instant"),
            ("", "at: rutis_bridge::session::Instant"),
            ("", "at: ::rutis_bridge::cordis::rpc::values::Instant"),
        ] {
            let source = with_current(import, params);
            assert_eq!(
                current_ts(&source).unwrap(),
                "current(at: Date): number",
                "{import} {params}"
            );
        }
    }

    /// A user's own type named like a tagged value type is not one: it is
    /// refused, not declared as the tagged value's TypeScript type (#408).
    #[test]
    fn a_users_own_instant_is_not_rutis_instant() {
        for (import, params) in [
            ("type Instant = String;", "at: Instant"),
            ("use std::time::Instant;", "at: Instant"),
            // A local declaration shadows a glob import.
            (
                "use rutis_bridge::session::*; type Instant = String;",
                "at: Instant",
            ),
            ("", "at: Instant"),
            ("use rutis_bridge::session::values::Bytes;", "raw: Bytes"),
        ] {
            let source = with_current(import, params);
            let error = current_ts(&source).unwrap_err();
            assert!(
                error.to_string().contains("type binding not implemented"),
                "{import} {params}: {error}"
            );
        }
    }

    /// NaN and the infinities in map keys and values and set elements are
    /// refused as they are in fields, not turned into `null` (#409).
    #[test]
    fn numbers_in_maps_and_sets_are_checked() {
        let source = with_current(
            "use rutis_bridge::session::{ValueMap, ValueSet};",
            "counts: ValueMap<String, f64>, keys: ValueMap<f64, Vec<ValueSet<f64>>>, \
             names: ValueMap<String, String>, tags: ValueSet<f64>",
        );
        let (rust, _, _) = generate(&source, "fixture", Path::new("/node")).unwrap();
        let refuse =
            "{ return Err(::rutis_bridge::cordis::Error::Value(\"non-finite number\".into())); }";
        for check in [
            format!("for (_, item) in counts.0.iter() {{  if !item.is_finite() {refuse} }}"),
            format!(
                "for (key, item) in keys.0.iter() {{ if !key.is_finite() {refuse} \
                 for item in item.iter() {{ for item in item.0.iter() {{ if !item.is_finite() {refuse} }} }} }}"
            ),
            format!("for item in tags.0.iter() {{ if !item.is_finite() {refuse} }}"),
        ] {
            assert!(rust.contains(&check), "{check}\n{rust}");
        }
        assert!(!rust.contains("names.0"), "{rust}");
    }

    #[test]
    fn does_not_silently_drop_public_fields_or_macro_methods() {
        let field = PLUGIN.replace("value: Mutex<f64>", "pub value: Mutex<f64>");
        assert!(generate(&field, "fixture", Path::new("/node"))
            .unwrap_err()
            .to_string()
            .contains("public field"));
        let methods = PLUGIN.replace("impl Counter {", "impl Counter { generated_methods!();");
        assert!(generate(&methods, "fixture", Path::new("/node"))
            .unwrap_err()
            .to_string()
            .contains("macro-expanded"));
        for hidden in ["mod extra;", "more_service_methods!();"] {
            let source = format!("{PLUGIN}\n{hidden}");
            assert!(generate(&source, "fixture", Path::new("/node"))
                .unwrap_err()
                .to_string()
                .contains("expansion"));
        }
    }
}
