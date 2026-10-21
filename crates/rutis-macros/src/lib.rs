//! `#[rutis::service]`: serve the public `&self` methods of an inherent
//! `impl` block to the plugins of other languages by name.
//!
//! The expansion keeps the block as written and adds, next to it, a
//! dispatcher and its registration with rutis (`rutis::service`). What
//! depends on a type (is it a `Result`, how its error is thrown, does a
//! parameter take a function) is decided by traits in the expanded,
//! concrete code, so type aliases are recognized as what they name. Use it
//! through `rutis` (`use rutis::service;` or `#[rutis::service]`).

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as Tokens};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{
    FnArg, GenericParam, ImplItem, ImplItemFn, ItemImpl, LitStr, PathArguments, ReturnType, Type,
    TypeParamBound,
};

/// See `rutis::service`.
#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: Tokens = item.into();
    match expand(attr.into(), item.clone()) {
        Ok(tokens) => tokens.into(),
        Err(error) => {
            // The block as written, so the only errors are the macro's.
            let block = syn::parse2::<ItemImpl>(item)
                .map(|mut block| {
                    strip(&mut block);
                    quote!(#block)
                })
                .unwrap_or_default();
            let error = error.to_compile_error();
            quote!(#block #error).into()
        }
    }
}

struct Options {
    name: Option<LitStr>,
    krate: Option<syn::Path>,
}

fn options(attr: Tokens) -> syn::Result<Options> {
    let mut options = Options {
        name: None,
        krate: None,
    };
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            let name: LitStr = meta.value()?.parse()?;
            if name.value().is_empty() {
                return Err(syn::Error::new(name.span(), "the service name is empty"));
            }
            options.name = Some(name);
            Ok(())
        } else if meta.path.is_ident("crate") {
            let path: LitStr = meta.value()?.parse()?;
            options.krate = Some(path.parse()?);
            Ok(())
        } else if meta.path.is_ident("skip") {
            Err(meta.error("`skip` goes on a method: #[service(skip)]"))
        } else {
            Err(meta.error("expected `name = \"...\"`"))
        }
    });
    syn::parse::Parser::parse2(parser, attr)?;
    Ok(options)
}

/// Whether `attr` is this macro: `#[service(...)]` or `#[rutis::service(...)]`.
fn is_marker(attr: &syn::Attribute) -> bool {
    let path = attr.path();
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == "service")
        && (path.segments.len() == 1
            || path.segments.iter().any(|segment| segment.ident == "rutis"))
}

/// Remove the `#[service(skip)]` markers from the block's methods.
fn strip(block: &mut ItemImpl) {
    for item in &mut block.items {
        if let ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| !is_marker(attr));
        }
    }
}

/// Whether a method is marked `#[service(skip)]`; other method options are
/// errors.
fn skipped(method: &ImplItemFn) -> syn::Result<bool> {
    let mut skip = false;
    for attr in method.attrs.iter().filter(|attr| is_marker(attr)) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                skip = true;
                Ok(())
            } else {
                Err(meta.error("a method takes only `#[service(skip)]`"))
            }
        })?;
    }
    Ok(skip)
}

/// The path generated code reaches rutis by: `::rutis`, or, in a crate that
/// depends on the dylib SDK instead, `::rutis_sdk::rutis`.
fn rutis_path(given: Option<syn::Path>) -> Tokens {
    if let Some(path) = given {
        return quote!(#path);
    }
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(|dir| std::path::PathBuf::from(dir).join("Cargo.toml"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let names = |name: &str| {
        manifest.lines().any(|line| {
            let line = line.trim();
            line.strip_prefix(name)
                .is_some_and(|rest| rest.trim_start().starts_with(['=', '.']))
                || line == format!("[dependencies.{name}]")
        })
    };
    if !names("rutis") && names("rutis-sdk") {
        quote!(::rutis_sdk::rutis)
    } else {
        quote!(::rutis)
    }
}

/// The service name a type is exported as by default: its name with the
/// first word lowercased (`Calendar` is `calendar`, `TreeCommands`
/// `treeCommands`, `HTTPClient` `httpClient`).
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

/// `snake_case` to `camelCase`, as the generator writes method names.
fn camel(name: &str) -> String {
    let mut pascal = String::new();
    let mut upper = true;
    for ch in name.trim_start_matches('_').chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            pascal.extend(ch.to_uppercase());
            upper = false;
        } else {
            pascal.push(ch);
        }
    }
    let mut chars = pascal.chars();
    chars
        .next()
        .map(|first| first.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

fn expand(attr: Tokens, item: Tokens) -> syn::Result<Tokens> {
    let options = options(attr)?;
    let mut block: ItemImpl = syn::parse2(item).map_err(|error| {
        syn::Error::new(
            error.span(),
            "#[rutis::service] goes on an inherent impl block: `impl Type { ... }`",
        )
    })?;
    if let Some((_, path, _)) = &block.trait_ {
        return Err(syn::Error::new(
            path.span(),
            "#[rutis::service] goes on an inherent impl block, not a trait impl",
        ));
    }
    if !block.generics.params.is_empty() {
        return Err(syn::Error::new(
            block.generics.span(),
            "a marked type cannot be generic: other languages call one concrete type",
        ));
    }
    let Type::Path(self_ty) = block.self_ty.as_ref() else {
        return Err(syn::Error::new(
            block.self_ty.span(),
            "a marked type is a named struct or enum",
        ));
    };
    let ident = match self_ty.path.segments.last() {
        Some(segment) if self_ty.qself.is_none() && segment.arguments.is_empty() => {
            segment.ident.to_string()
        }
        _ => {
            return Err(syn::Error::new(
                self_ty.span(),
                "a marked type is a named struct or enum without generic arguments",
            ))
        }
    };
    let rutis = rutis_path(options.krate);
    let (name, named) = match &options.name {
        Some(name) => (name.value(), true),
        None => (default_name(&ident), false),
    };
    let ty = block.self_ty.as_ref().clone();
    let ty = &ty;

    let mut errors: Option<syn::Error> = None;
    let mut push = |error: syn::Error| match &mut errors {
        Some(errors) => errors.combine(error),
        None => errors = Some(error),
    };
    let mut arms = Vec::new();
    let mut methods = Vec::new();
    for item in &block.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };
        let skip = match skipped(method) {
            Ok(skip) => skip,
            Err(error) => {
                push(error);
                continue;
            }
        };
        if skip || !matches!(method.vis, syn::Visibility::Public(_)) {
            continue;
        }
        match method.sig.inputs.first() {
            Some(FnArg::Receiver(receiver))
                if receiver.reference.is_some()
                    && receiver.mutability.is_none()
                    && receiver.colon_token.is_none() => {}
            // Constructors and associated functions are not the service's.
            None | Some(FnArg::Typed(_)) => continue,
            Some(FnArg::Receiver(receiver)) => {
                push(syn::Error::new(
                    receiver.span(),
                    format!(
                        "`{}` cannot be called from other languages: a marked service is shared, so its methods take `&self`; mark it #[service(skip)] to leave it out",
                        method.sig.ident
                    ),
                ));
                continue;
            }
        }
        let js = camel(&method.sig.ident.to_string());
        if methods.contains(&js) {
            push(syn::Error::new(
                method.sig.ident.span(),
                format!("two methods are `{js}` for other languages"),
            ));
            continue;
        }
        match arm(&rutis, ty, method, &js) {
            Ok(arm) => {
                arms.push(arm);
                methods.push(js);
            }
            Err(error) => push(error),
        }
    }
    if let Some(errors) = errors {
        return Err(errors);
    }
    strip(&mut block);
    let type_name = &ident;
    Ok(quote! {
        #block

        #[doc(hidden)]
        const _: () = {
            fn __rutis_type_id() -> ::std::any::TypeId {
                ::std::any::TypeId::of::<#ty>()
            }
            fn __rutis_type_name() -> &'static str {
                ::std::any::type_name::<#ty>()
            }
            fn __rutis_erase(
                provided: ::std::boxed::Box<dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync>,
            ) -> ::std::option::Option<::std::sync::Arc<dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync>> {
                provided
                    .downcast::<::std::sync::Arc<#ty>>()
                    .ok()
                    .map(|service| *service as ::std::sync::Arc<dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync>)
            }
            #[allow(unused_variables, unused_mut, clippy::all)]
            fn __rutis_invoke(
                service: &::std::sync::Arc<dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync>,
                method: &str,
                args: #rutis::service::Encoded,
                wire: &'static dyn #rutis::service::Wire,
            ) -> ::std::result::Result<#rutis::service::Answer, #rutis::service::CallError> {
                #[allow(unused_imports)]
                use #rutis::service::{
                    FailCordis as _, FailDebug as _, FailDisplay as _, FailIo as _, FailSerde as _,
                    FailString as _, SplitResult as _, SplitValue as _, ViaData as _,
                    ViaFunction as _,
                };
                match method {
                    #(#arms)*
                    other => ::std::result::Result::Err(wire.error(::std::format!(
                        "{} has no method {}", #type_name, other
                    ))),
                }
            }
            static __RUTIS_ENTRY: #rutis::service::Entry = #rutis::service::Entry::new(
                __rutis_type_id,
                __rutis_type_name,
                #name,
                #named,
                &[#(#methods),*],
                __rutis_erase,
                __rutis_invoke,
            );
            fn __rutis_register() {
                #rutis::service::register(&__RUTIS_ENTRY);
            }
            #rutis::__on_load!(__rutis_register);
        };
    })
}

/// A parameter as the dispatcher passes it.
enum Param {
    /// The calling row's context, by reference or by value.
    Ctx { by_ref: bool },
    /// A closure calling back a function the caller passed.
    Closure { args: usize },
    /// Decoded to `ty` (data, or a function by `FromFunction`), passed as
    /// it is or by reference.
    Value { ty: Box<Type>, by_ref: bool },
}

fn arm(rutis: &Tokens, ty: &Type, method: &ImplItemFn, js: &str) -> syn::Result<Tokens> {
    let sig = &method.sig;
    let rust = &sig.ident;
    if let Some(unsafety) = &sig.unsafety {
        return Err(syn::Error::new(
            unsafety.span(),
            format!("`{rust}` cannot be called from other languages: it is unsafe"),
        ));
    }
    if let Some(variadic) = &sig.variadic {
        return Err(syn::Error::new(
            variadic.span(),
            "a variadic method cannot be exported",
        ));
    }
    // Type parameters: only functions the caller passes.
    let mut bounds: Vec<(String, Vec<TypeParamBound>)> = Vec::new();
    for param in &sig.generics.params {
        match param {
            GenericParam::Lifetime(_) => {}
            GenericParam::Type(param) => bounds.push((
                param.ident.to_string(),
                param.bounds.iter().cloned().collect(),
            )),
            GenericParam::Const(param) => {
                return Err(syn::Error::new(
                    param.span(),
                    format!(
                        "`{rust}` cannot be called from other languages: it has a const parameter"
                    ),
                ))
            }
        }
    }
    if let Some(clause) = &sig.generics.where_clause {
        for predicate in &clause.predicates {
            if let syn::WherePredicate::Type(predicate) = predicate {
                if let Some(name) = single_ident(&predicate.bounded_ty) {
                    if let Some((_, list)) = bounds.iter_mut().find(|(known, _)| *known == name) {
                        list.extend(predicate.bounds.iter().cloned());
                    }
                }
            }
        }
    }
    let mut params = Vec::new();
    for input in sig.inputs.iter().skip(1) {
        let FnArg::Typed(input) = input else {
            unreachable!("only the first input is a receiver")
        };
        params.push(param(rust, &input.ty, &bounds)?);
    }
    let count = params
        .iter()
        .filter(|param| !matches!(param, Param::Ctx { .. }))
        .count();

    // How the result comes back: awaited, or now.
    let awaited = sig.asyncness.is_some()
        || match &sig.output {
            ReturnType::Default => false,
            ReturnType::Type(_, output) => is_future(output),
        };
    if let ReturnType::Type(_, output) = &sig.output {
        if let Type::Reference(reference) = output.as_ref() {
            if reference
                .lifetime
                .as_ref()
                .is_some_and(|lifetime| lifetime.ident == "static")
            {
                // `&'static str` and the like are owned enough.
            } else {
                return Err(syn::Error::new(
                reference.span(),
                format!("`{rust}` cannot be called from other languages: it returns a borrow; return an owned value"),
            ));
            }
        }
        if let Type::ImplTrait(implemented) = output.as_ref() {
            if !is_future(output) {
                return Err(syn::Error::new(
                    implemented.span(),
                    format!("`{rust}` cannot be called from other languages: it returns `impl Trait`; return a concrete type"),
                ));
            }
        }
    }

    let mut decode = Vec::new();
    let mut call = Vec::new();
    let mut takes_ctx = false;
    for (index, param) in params.iter().enumerate() {
        let local = format_ident!("a{}", index);
        match param {
            Param::Ctx { by_ref } => {
                takes_ctx = true;
                call.push(if *by_ref {
                    quote!(&caller)
                } else {
                    quote!(caller.clone())
                });
            }
            Param::Closure { args } => {
                let names: Vec<_> = (0..*args).map(|i| format_ident!("p{}", i)).collect();
                decode.push(quote! {
                    let #local = {
                        let function = #rutis::service::function(wire, args.next().unwrap())?;
                        move |#(#names),*| #rutis::service::call_back(
                            &function,
                            wire,
                            ::std::vec![#(#rutis::service::encode_arg(wire, &#names)),*],
                        )
                    };
                });
                call.push(quote!(#local));
            }
            Param::Value {
                ty: param_ty,
                by_ref,
            } => {
                let span = param_ty.span();
                decode.push(quote_spanned! {span=>
                    let #local: #param_ty = (&#rutis::service::Param::<#param_ty>::NEW)
                        .__rutis_param(wire, args.next().unwrap())?;
                });
                call.push(if *by_ref {
                    quote!(&#local)
                } else {
                    quote!(#local)
                });
            }
        }
    }
    let caller = takes_ctx.then(|| quote!(let caller = #rutis::service::caller(wire, #js)?;));
    let invoke = quote!(<#ty>::#rust(&*service #(, #call)*));
    let output_span = match &sig.output {
        ReturnType::Type(_, output) => output.span(),
        ReturnType::Default => rust.span(),
    };
    let reply = quote_spanned! {output_span=>
        {
            let returned = #rutis::service::Returned::new(returned);
            match (&returned).__rutis_split() {
                ::std::result::Result::Ok(value) => #rutis::service::encode_ok(wire, &value),
                ::std::result::Result::Err(error) => ::std::result::Result::Err(
                    #rutis::service::forwarded(wire, &error).unwrap_or_else(|| {
                        (&&&&&#rutis::service::Failure(&error)).__rutis_fail(wire)
                    }),
                ),
            }
        }
    };
    let body = if awaited {
        quote! {
            ::std::result::Result::Ok(#rutis::service::Answer::Later(::std::boxed::Box::pin(async move {
                let returned = #invoke.await;
                #reply
            })))
        }
    } else {
        quote! {
            let returned = #invoke;
            ::std::result::Result::Ok(#rutis::service::Answer::Ready(#reply?))
        }
    };
    Ok(quote! {
        #js => {
            #caller
            let mut args = wire.arguments(args, #count)?.into_iter();
            #(#decode)*
            let service = #rutis::service::downcast::<#ty>(service);
            #body
        }
    })
}

fn param(
    method: &syn::Ident,
    ty: &Type,
    bounds: &[(String, Vec<TypeParamBound>)],
) -> syn::Result<Param> {
    let refused = |span: Span, why: &str| {
        syn::Error::new(
            span,
            format!("`{method}` cannot be called from other languages: {why}"),
        )
    };
    // The calling row's context.
    if is_ctx(ty) {
        return match ty {
            Type::Reference(reference) if reference.mutability.is_some() => Err(refused(
                reference.span(),
                "it takes the context by mutable reference",
            )),
            Type::Reference(_) => Ok(Param::Ctx { by_ref: true }),
            _ => Ok(Param::Ctx { by_ref: false }),
        };
    }
    // A function: `impl Fn(..)`, or a type parameter bound by one.
    let bound = match ty {
        Type::ImplTrait(implemented) => Some(
            fn_bound(&implemented.bounds.iter().cloned().collect::<Vec<_>>())
                .cloned()
                .ok_or_else(|| {
                    refused(
                        implemented.span(),
                        "it takes an `impl Trait` that is not a function",
                    )
                })?,
        ),
        _ => {
            match single_ident(ty).and_then(|name| bounds.iter().find(|(known, _)| *known == name))
            {
                Some((name, list)) => Some(fn_bound(list).cloned().ok_or_else(|| {
                    refused(
                        ty.span(),
                        &format!("its type parameter {name} is not a function"),
                    )
                })?),
                None => None,
            }
        }
    };
    if let Some(bound) = bound {
        let segment = bound.path.segments.last().expect("a segment");
        let PathArguments::Parenthesized(arguments) = &segment.arguments else {
            return Err(refused(
                bound.span(),
                "its function parameter has no signature",
            ));
        };
        if let Some(input) = arguments
            .inputs
            .iter()
            .find(|input| matches!(input, Type::Reference(_)))
        {
            return Err(refused(
                input.span(),
                "the function it takes borrows its arguments: take them by value",
            ));
        }
        if arguments.inputs.len() > 4 {
            return Err(refused(
                arguments.inputs.span(),
                "the function it takes has more than four arguments",
            ));
        }
        if matches!(arguments.output, ReturnType::Default) {
            return Err(refused(
                bound.span(),
                "the function it takes must return a future of a Result (BoxFuture<'static, Result<..>>), for its failure to reach the caller",
            ));
        }
        return Ok(Param::Closure {
            args: arguments.inputs.len(),
        });
    }
    if single_ident(ty).is_some_and(|name| bounds.iter().any(|(known, _)| *known == name)) {
        return Err(refused(ty.span(), "it has a generic parameter"));
    }
    match ty {
        Type::Reference(reference) if reference.mutability.is_some() => {
            Err(refused(reference.span(), "it takes a mutable reference"))
        }
        Type::Reference(reference) => {
            let owned: Type = match reference.elem.as_ref() {
                Type::Slice(slice) => {
                    let elem = &slice.elem;
                    syn::parse_quote!(::std::vec::Vec<#elem>)
                }
                Type::Path(path) if path.path.is_ident("str") => {
                    syn::parse_quote!(::std::string::String)
                }
                Type::Path(path)
                    if path.qself.is_none()
                        && path
                            .path
                            .segments
                            .last()
                            .is_some_and(|s| s.ident == "Path" && s.arguments.is_empty()) =>
                {
                    syn::parse_quote!(::std::path::PathBuf)
                }
                elem => elem.clone(),
            };
            Ok(Param::Value {
                ty: Box::new(owned),
                by_ref: true,
            })
        }
        Type::ImplTrait(implemented) => {
            Err(refused(implemented.span(), "it takes an `impl Trait`"))
        }
        _ => Ok(Param::Value {
            ty: Box::new(ty.clone()),
            by_ref: false,
        }),
    }
}

/// `Ctx`, `rutis::Ctx` or a reference to one.
fn is_ctx(ty: &Type) -> bool {
    let ty = match ty {
        Type::Reference(reference) => reference.elem.as_ref(),
        ty => ty,
    };
    let Type::Path(path) = ty else {
        return false;
    };
    path.qself.is_none()
        && path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Ctx" && segment.arguments.is_empty())
}

/// Whether a return type is a future: `BoxFuture<..>`, `Pin<Box<dyn
/// Future<..>>>` or `impl Future<..>`.
fn is_future(ty: &Type) -> bool {
    match ty {
        Type::ImplTrait(implemented) => future(&implemented.bounds),
        Type::Path(path) if path.qself.is_none() => {
            let Some(last) = path.path.segments.last() else {
                return false;
            };
            if last.ident == "BoxFuture" {
                return true;
            }
            if last.ident != "Pin" {
                return false;
            }
            let PathArguments::AngleBracketed(args) = &last.arguments else {
                return false;
            };
            args.args.iter().any(|arg| {
                let syn::GenericArgument::Type(Type::Path(boxed)) = arg else {
                    return false;
                };
                boxed.path.segments.last().is_some_and(|segment| {
                    let PathArguments::AngleBracketed(inner) = &segment.arguments else {
                        return false;
                    };
                    inner.args.iter().any(|arg| {
                        matches!(arg, syn::GenericArgument::Type(Type::TraitObject(object)) if future(&object.bounds))
                    })
                })
            })
        }
        _ => false,
    }
}

/// Whether `bounds` include `Future<..>`.
fn future<'a>(bounds: impl IntoIterator<Item = &'a TypeParamBound>) -> bool {
    bounds.into_iter().any(|bound| {
        matches!(bound, TypeParamBound::Trait(bound) if bound.path.segments.last().is_some_and(|s| s.ident == "Future"))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names_lowercase_the_first_word() {
        assert_eq!(default_name("Calendar"), "calendar");
        assert_eq!(default_name("TreeCommands"), "treeCommands");
        assert_eq!(default_name("HTTPClient"), "httpClient");
        assert_eq!(default_name("IO"), "io");
        assert_eq!(default_name("A"), "a");
    }

    #[test]
    fn method_names_are_camel_case() {
        assert_eq!(camel("add_entry"), "addEntry");
        assert_eq!(camel("list"), "list");
        assert_eq!(camel("_private"), "private");
    }

    #[test]
    fn futures_are_recognized_by_their_form() {
        let future = |text: &str| is_future(&syn::parse_str::<Type>(text).unwrap());
        assert!(future("BoxFuture<'_, u32>"));
        assert!(future("rutis::BoxFuture<'static, Result<u32, E>>"));
        assert!(future("Pin<Box<dyn Future<Output = u32> + Send>>"));
        assert!(future("impl Future<Output = u32> + Send"));
        assert!(!future("Vec<u32>"));
        assert!(!future("Result<u32, E>"));
    }

    #[test]
    fn only_methods_that_cross_are_exported() {
        let refused = |source: &str| {
            let item: Tokens = source.parse().unwrap();
            expand(Tokens::new(), item)
                .err()
                .map(|error| error.to_string())
        };
        assert!(refused(
            "impl A { pub fn a(&self) -> u32 { 1 } fn b(&mut self) {} pub fn new() -> Self { A } }"
        )
        .is_none());
        assert!(refused("impl A { pub fn a(&mut self) {} }")
            .unwrap()
            .contains("take `&self`"));
        assert!(refused("impl A { pub fn a(&self) -> &str { \"\" } }")
            .unwrap()
            .contains("returns a borrow"));
        assert!(refused("impl A { pub fn a(&self, x: &mut u32) {} }")
            .unwrap()
            .contains("mutable reference"));
        assert!(refused("impl A { pub fn a<T>(&self, x: T) {} }")
            .unwrap()
            .contains("not a function"));
        assert!(refused(
            "impl A { pub fn a(&self, f: impl Fn(&str) -> BoxFuture<'static, Result<(), E>>) {} }"
        )
        .unwrap()
        .contains("borrows its arguments"));
        assert!(refused("impl A { #[service(skip)] pub fn a(&mut self) {} }").is_none());
        // `a_` is `a` for other languages too.
        assert!(refused("impl A { pub fn a(&self) {} pub fn a_(&self) {} }")
            .unwrap()
            .contains("two methods are `a`"));
        assert!(refused("impl<T> A<T> { pub fn a(&self) {} }")
            .unwrap()
            .contains("generic"));
        assert!(refused("impl Trait for A { fn a(&self) {} }")
            .unwrap()
            .contains("trait impl"));
    }
}
