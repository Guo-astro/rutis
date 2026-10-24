//! Rewrites Rust code that uses rutis 0.16 for rutis 0.17.
//!
//! The goal is code that compiles on 0.17 and behaves as before: an old
//! `impl Plugin` becomes `impl host::DynPlugin` (the same trait, renamed),
//! `ctx.events().on(&ctx, &key, |c, e, call| Box::pin(async move { … }))`
//! becomes `ctx.on::<E>(|e, call| async move { … })`, `CordisError` becomes
//! `rutis::Error`, and names moved off the root get their new paths. Where
//! a rewrite is not mechanical, a `// TODO(rutis 0.17): …` comment marks
//! the spot and the compile error points there.
//!
//! The edits are made on the original text (the syntax tree only locates
//! them), so comments and formatting are kept.

use std::collections::BTreeSet;

use proc_macro2::{Delimiter, LineColumn, Span, TokenStream, TokenTree};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

/// The result of migrating one file.
#[derive(Debug, Clone)]
pub struct Migrated {
    pub source: String,
    /// The `TODO(rutis 0.17)` comments added, by line (1-based).
    pub todos: Vec<Todo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    pub line: usize,
    pub message: String,
}

/// Migrates one file. A file that does not parse, or does not use rutis,
/// is returned unchanged.
pub fn migrate(source: &str) -> Migrated {
    let unchanged = Migrated {
        source: source.to_owned(),
        todos: Vec::new(),
    };
    if !uses_rutis(source) {
        return unchanged;
    }
    if already_migrated(source) {
        // only the rutis-loader paths, which are safe to rewrite again
        return match rewrite_loader_paths(source) {
            Some(text) => Migrated {
                todos: todos_in(&text),
                source: text,
            },
            None => unchanged,
        };
    }
    let mut text = source.to_owned();
    let krate = crate_path(&text);
    let error_name = error_name(&text);

    // 1. renames of tokens, once (also inside macros)
    let Some(next) = rename_tokens(&text, &error_name) else {
        return unchanged;
    };
    text = next;

    // 2. structural rewrites, until nothing changes
    for _ in 0..12 {
        let Ok(file) = syn::parse_file(&text) else {
            break;
        };
        let index = LineIndex::new(&text);
        let mut rules = Rules {
            src: &text,
            index: &index,
            krate: &krate,
            error_name: &error_name,
            edits: Vec::new(),
            fn_lets: Vec::new(),
            consts: Vec::new(),
            listeners: Vec::new(),
        };
        rules.prescan(&file);
        rules.visit_file(&file);
        rules.matches_in_macros(&file);
        let edits = std::mem::take(&mut rules.edits);
        if edits.is_empty() {
            break;
        }
        text = apply(&text, edits);
    }

    // 3. imports and qualified paths, once
    if let Some(next) = rewrite_imports(&text, &krate, &error_name) {
        text = next;
    }
    if let Some(next) = rewrite_loader_paths(&text) {
        text = next;
    }

    let todos = todos_in(&text);
    Migrated {
        source: text,
        todos,
    }
}

fn uses_rutis(source: &str) -> bool {
    source.contains("rutis::")
        || source.contains("rutis_sdk::")
        || source.contains("rutis_loader::")
}

/// A file already written for 0.17 is left alone: its `Plugin` is the new
/// trait and must not be renamed again.
fn already_migrated(source: &str) -> bool {
    source.contains("rutis::host")
        || source.contains("DynPlugin")
        || source.contains("TODO(rutis 0.17)")
        // the new plugin trait's shape and the 0.17 result alias
        || source.contains("async fn apply(")
        || source.contains("rutis::Result")
}

/// `rutis` or, for a dylib plugin that only depends on the SDK,
/// `rutis_sdk::rutis`.
fn crate_path(source: &str) -> String {
    if source.contains("rutis_sdk::rutis")
        && !source.contains("use rutis::")
        && !source.contains("use rutis;")
    {
        "rutis_sdk::rutis".to_owned()
    } else {
        "rutis".to_owned()
    }
}

/// `Error`, unless the file has another `Error` (then `RutisError`).
fn error_name(source: &str) -> String {
    if let Ok(file) = syn::parse_file(source) {
        return if has_other_error(&file.items) {
            "RutisError"
        } else {
            "Error"
        }
        .to_owned();
    }
    let other_error = source.lines().any(|line| {
        let l = line.trim_start();
        (l.starts_with("use ")
            && !l.contains("rutis")
            && (l.contains("::Error;")
                || l.contains(" Error,")
                || l.contains("{Error")
                || l.contains(", Error}")
                || l.contains("::Error}")))
            || l.starts_with("enum Error")
            || l.starts_with("pub enum Error")
            || l.starts_with("struct Error")
            || l.starts_with("pub struct Error")
            || l.starts_with("type Error")
            || l.starts_with("pub type Error")
    });
    if other_error {
        "RutisError".to_owned()
    } else {
        "Error".to_owned()
    }
}

/// An item named `Error`, or an `Error` imported from outside rutis, at
/// the top level of the file or of an inline module.
fn has_other_error(items: &[syn::Item]) -> bool {
    fn use_names(tree: &syn::UseTree, from_rutis: bool, out: &mut bool) {
        match tree {
            syn::UseTree::Path(p) => {
                let rutis = from_rutis || p.ident == "rutis" || p.ident == "rutis_sdk";
                use_names(&p.tree, rutis, out)
            }
            syn::UseTree::Name(n) => *out |= !from_rutis && n.ident == "Error",
            syn::UseTree::Rename(r) => *out |= !from_rutis && r.rename == "Error",
            syn::UseTree::Group(g) => g.items.iter().for_each(|t| use_names(t, from_rutis, out)),
            syn::UseTree::Glob(_) => {}
        }
    }
    let mut found = false;
    for item in items {
        match item {
            syn::Item::Use(u) => use_names(&u.tree, false, &mut found),
            syn::Item::Struct(i) => found |= i.ident == "Error",
            syn::Item::Enum(i) => found |= i.ident == "Error",
            syn::Item::Type(i) => found |= i.ident == "Error",
            syn::Item::Trait(i) => found |= i.ident == "Error",
            _ => {}
        }
    }
    found
}

/// The file defines or imports a `Key` of its own.
fn has_own_key(source: &str) -> bool {
    let Ok(file) = syn::parse_file(source) else {
        return source.contains("type Key")
            || source.contains("struct Key")
            || source.contains("enum Key");
    };
    fn visit(items: &[syn::Item]) -> bool {
        items.iter().any(|item| match item {
            syn::Item::Struct(i) => i.ident == "Key",
            syn::Item::Enum(i) => i.ident == "Key",
            syn::Item::Type(i) => i.ident == "Key",
            syn::Item::Trait(i) => i.ident == "Key",
            syn::Item::Use(u) => {
                let text = quote_use(&u.tree);
                !text.starts_with("rutis")
                    && (text.ends_with("::Key")
                        || text.contains("::Key,")
                        || text.contains(" as Key"))
            }
            syn::Item::Mod(m) => m.content.as_ref().is_some_and(|(_, items)| visit(items)),
            syn::Item::Fn(f) => f
                .sig
                .generics
                .params
                .iter()
                .any(|p| matches!(p, syn::GenericParam::Type(t) if t.ident == "Key")),
            _ => false,
        })
    }
    visit(&file.items)
}

fn quote_use(tree: &syn::UseTree) -> String {
    match tree {
        syn::UseTree::Path(p) => format!("{}::{}", p.ident, quote_use(&p.tree)),
        syn::UseTree::Name(n) => n.ident.to_string(),
        syn::UseTree::Rename(r) => format!("{} as {}", r.ident, r.rename),
        syn::UseTree::Glob(_) => "*".to_owned(),
        syn::UseTree::Group(g) => g.items.iter().map(quote_use).collect::<Vec<_>>().join(","),
    }
}

// ── text edits ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

/// Applies non-overlapping edits; an edit overlapping an earlier one (by
/// start) is left for the next pass.
fn apply(text: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    let mut last_end: Option<usize> = None;
    for edit in edits {
        if let Some(end) = last_end {
            if edit.start < end
                || (edit.start == end && edit.start == edit.end && end == pos && false)
            {
                continue;
            }
        }
        if edit.start < pos {
            continue;
        }
        out.push_str(&text[pos..edit.start]);
        out.push_str(&edit.text);
        pos = edit.end;
        last_end = Some(edit.end);
    }
    out.push_str(&text[pos..]);
    out
}

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self { starts }
    }

    fn offset(&self, text: &str, at: LineColumn) -> usize {
        let line_start = self
            .starts
            .get(at.line.saturating_sub(1))
            .copied()
            .unwrap_or(text.len());
        let line_end = text[line_start..]
            .find('\n')
            .map(|n| line_start + n)
            .unwrap_or(text.len());
        let line = &text[line_start..line_end];
        match line.char_indices().nth(at.column) {
            Some((i, _)) => line_start + i,
            None => line_end,
        }
    }

    fn range(&self, text: &str, span: Span) -> (usize, usize) {
        (
            self.offset(text, span.start()),
            self.offset(text, span.end()),
        )
    }

    fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }
}

// ── 1. token renames ─────────────────────────────────────────────

fn rename_tokens(text: &str, error_name: &str) -> Option<String> {
    let stream: TokenStream = text.parse().ok()?;
    let index = LineIndex::new(text);
    let mut tokens = Vec::new();
    flatten(stream, &mut tokens);
    let rutis_plugin = text.contains("rutis::Plugin")
        || text.contains("use rutis::*")
        || text.contains("use rutis_sdk::rutis::*")
        || text.contains("rutis::{") && import_lists_name(text, "Plugin")
        || text.contains("Plugin for") && import_lists_name(text, "Plugin");
    let mut edits = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        let TokenTree::Ident(ident) = tok else {
            continue;
        };
        let name = ident.to_string();
        let replacement = match name.as_str() {
            "CordisError" => {
                let shared = i >= 2
                    && matches!(&tokens[i - 1], TokenTree::Punct(p) if p.as_char() == '<')
                    && matches!(&tokens[i - 2], TokenTree::Ident(arc) if arc == "Arc");
                let path_follows = matches!(tokens.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == ':')
                    && matches!(tokens.get(i + 3), Some(TokenTree::Ident(next)) if next.to_string().chars().next().is_some_and(|c| c.is_uppercase()));
                let qualified = i >= 2
                    && matches!(&tokens[i - 1], TokenTree::Punct(p) if p.as_char() == ':')
                    && matches!(&tokens[i - 2], TokenTree::Punct(p) if p.as_char() == ':');
                if path_follows || shared {
                    "ErrorKind".to_owned()
                } else if qualified {
                    // `rutis::CordisError`: the path names the real item
                    "Error".to_owned()
                } else {
                    error_name.to_owned()
                }
            }
            "TypedPlugin" => "Plugin".to_owned(),
            // `bindings::Plugin` and other crates' `Plugin`s stay
            "Plugin" if rutis_plugin && !foreign_path(&tokens, i) => "DynPlugin".to_owned(),
            "ServiceKey" => "TypeKey".to_owned(),
            "DepKey" => "Key".to_owned(),
            _ => continue,
        };
        let (start, end) = index.range(text, ident.span());
        edits.push(Edit {
            start,
            end,
            text: replacement,
        });
    }
    // `TypedPlugin` must not become `DynPlugin`: renames are decided on the
    // original tokens, in one pass.
    Some(apply(text, edits))
}

/// The identifier at `i` is the last segment of a path whose previous
/// segment is not `rutis` (`bindings::Plugin`).
fn foreign_path(tokens: &[TokenTree], i: usize) -> bool {
    if i < 3 {
        return false;
    }
    let colons = matches!(&tokens[i - 1], TokenTree::Punct(p) if p.as_char() == ':')
        && matches!(&tokens[i - 2], TokenTree::Punct(p) if p.as_char() == ':');
    colons && !matches!(&tokens[i - 3], TokenTree::Ident(prev) if prev == "rutis")
}

fn import_lists_name(text: &str, name: &str) -> bool {
    // a `use rutis::{..., Name, ...}` (possibly multi-line) mentions it
    let mut rest = text;
    while let Some(i) = rest.find("use rutis") {
        let after = &rest[i..];
        let end = after.find(';').unwrap_or(after.len());
        let item = &after[..end];
        if item
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|w| w == name)
        {
            return true;
        }
        rest = &after[end.min(after.len())..];
        if rest.is_empty() {
            break;
        }
        rest = &rest[1.min(rest.len())..];
    }
    false
}

/// All tokens, groups flattened (their delimiters dropped).
fn flatten(stream: TokenStream, out: &mut Vec<TokenTree>) {
    for tt in stream {
        match tt {
            TokenTree::Group(g) => {
                out.push(TokenTree::Group(g.clone()));
                flatten(g.stream(), out);
            }
            other => out.push(other),
        }
    }
}

// ── 2. structural rules ──────────────────────────────────────────

struct Rules<'a> {
    src: &'a str,
    index: &'a LineIndex,
    krate: &'a str,
    error_name: &'a str,
    edits: Vec<Edit>,
    /// `let` bindings of the enclosing functions (innermost last): name and
    /// initializer text, for resolving event keys and `.events()` handles.
    fn_lets: Vec<Vec<(String, String, Option<String>)>>,
    /// File-level `const` / `static` items: name, initializer, type.
    consts: Vec<(String, String, Option<String>)>,
    /// Struct and fn listeners found in the file: name → kind and event type.
    listeners: Vec<(String, ListenerKindOf, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ListenerKindOf {
    AsyncStruct,
    SyncStruct,
    PatternStruct,
    SyncPatternStruct,
    AsyncFn,
    SyncFn,
}

const EVENT_METHODS: &[&str] = &[
    "on",
    "on_opt",
    "once",
    "on_sync",
    "on_sync_opt",
    "on_pattern",
    "on_pattern_opt",
    "on_sync_pattern",
    "on_sync_pattern_opt",
    "emit",
    "parallel",
    "serial",
    "waterfall",
    "bail_sync",
    "waterfall_sync",
];

impl<'a> Rules<'a> {
    fn range(&self, node: &impl Spanned) -> (usize, usize) {
        self.index.range(self.src, node.span())
    }

    fn text(&self, node: &impl Spanned) -> &'a str {
        let (s, e) = self.range(node);
        &self.src[s..e]
    }

    fn edit(&mut self, node: &impl Spanned, text: String) {
        let (start, end) = self.range(node);
        if self.src[start..end] != text {
            self.edits.push(Edit { start, end, text });
        }
    }

    fn edit_range(&mut self, start: usize, end: usize, text: String) {
        if self.src[start..end] != text {
            self.edits.push(Edit { start, end, text });
        }
    }

    fn todo(&mut self, node: &impl Spanned, message: impl Into<String>) {
        let (start, _) = self.range(node);
        let message = message.into();
        if let Some(edit) = todo_edit(self.src, self.index, start, &message) {
            if !self
                .edits
                .iter()
                .any(|e| e.start == edit.start && e.text == edit.text)
            {
                self.edits.push(edit);
            }
        }
    }

    fn error_path(&self) -> String {
        format!("{}::Error", self.krate)
    }

    fn result_path(&self) -> String {
        format!("{}::Result", self.krate)
    }

    /// The initializer of a local `let NAME = …;` in the enclosing functions.
    fn local(&self, name: &str) -> Option<(String, Option<String>)> {
        self.fn_lets
            .iter()
            .rev()
            .flat_map(|lets| lets.iter().rev())
            .chain(self.consts.iter().rev())
            .find(|(n, _, _)| n == name)
            .map(|(_, init, ty)| (init.clone(), ty.clone()))
    }

    /// File-level consts and the struct / fn listeners of the file (in their
    /// old form, or already rewritten by an earlier pass).
    fn prescan(&mut self, file: &syn::File) {
        for item in &file.items {
            match item {
                syn::Item::Const(c) => self.consts.push((
                    c.ident.to_string(),
                    self.text(&*c.expr).to_owned(),
                    Some(self.text(&*c.ty).to_owned()),
                )),
                syn::Item::Static(st) => self.consts.push((
                    st.ident.to_string(),
                    self.text(&*st.expr).to_owned(),
                    Some(self.text(&*st.ty).to_owned()),
                )),
                syn::Item::Impl(i) => {
                    let syn::Type::Path(self_ty) = &*i.self_ty else {
                        continue;
                    };
                    let Some(name) = self_ty.path.segments.last().map(|s| s.ident.to_string())
                    else {
                        continue;
                    };
                    if let Some((_, path, _)) = &i.trait_ {
                        let Some(last) = path.segments.last() else {
                            continue;
                        };
                        let kind = match last.ident.to_string().as_str() {
                            "Listener" => ListenerKindOf::AsyncStruct,
                            "SyncListener" => ListenerKindOf::SyncStruct,
                            "PatternListener" => ListenerKindOf::PatternStruct,
                            "SyncPatternListener" => ListenerKindOf::SyncPatternStruct,
                            _ => continue,
                        };
                        if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
                            if let Some(arg) = args.args.first() {
                                self.listeners.push((name, kind, self.text(arg).to_owned()));
                            }
                        }
                    } else {
                        // `impl S { async fn on_event(&self, _: &Ctx, _: &E, …) }`
                        for it in &i.items {
                            let syn::ImplItem::Fn(f) = it else { continue };
                            let kind = match f.sig.ident.to_string().as_str() {
                                "on_event" if f.sig.inputs.len() == 4 => {
                                    ListenerKindOf::AsyncStruct
                                }
                                "on_event" if f.sig.inputs.len() == 5 => {
                                    ListenerKindOf::PatternStruct
                                }
                                "on_event_sync" if f.sig.inputs.len() == 4 => {
                                    ListenerKindOf::SyncStruct
                                }
                                "on_event_sync" if f.sig.inputs.len() == 5 => {
                                    ListenerKindOf::SyncPatternStruct
                                }
                                _ => continue,
                            };
                            let ev_at = if matches!(
                                kind,
                                ListenerKindOf::PatternStruct | ListenerKindOf::SyncPatternStruct
                            ) {
                                3
                            } else {
                                2
                            };
                            if let Some(syn::FnArg::Typed(t)) = f.sig.inputs.iter().nth(ev_at) {
                                let ty =
                                    self.text(&*t.ty).trim_start_matches('&').trim().to_owned();
                                self.listeners.push((name.clone(), kind, ty));
                            }
                        }
                    }
                }
                syn::Item::Fn(f) => {
                    // fn l<'a>(ctx: &'a Ctx, e: &'a E, call: Call<'a, V>) -> BoxFuture<…>
                    // or the rewritten `async fn l(ctx: &Ctx, e: &E, call: Call<E>)`
                    if f.sig.inputs.len() != 3 {
                        continue;
                    }
                    let tys: Vec<String> = f
                        .sig
                        .inputs
                        .iter()
                        .map(|a| match a {
                            syn::FnArg::Typed(t) => self.text(&*t.ty).to_owned(),
                            _ => String::new(),
                        })
                        .collect();
                    if !tys[0].contains("Ctx") {
                        continue;
                    }
                    let ev = tys[1]
                        .trim_start_matches('&')
                        .trim_start_matches("'a")
                        .trim()
                        .to_owned();
                    let kind = if tys[2].contains("SyncCall") {
                        ListenerKindOf::SyncFn
                    } else if tys[2].contains("Call") {
                        ListenerKindOf::AsyncFn
                    } else {
                        continue;
                    };
                    self.listeners.push((f.sig.ident.to_string(), kind, ev));
                }
                _ => {}
            }
        }
    }

    fn listener_of(&self, expr: &syn::Expr) -> Option<(ListenerKindOf, String)> {
        let text = self.text(expr).trim();
        let head: String = text
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
            .collect();
        let head = head.rsplit("::").next().unwrap_or(&head).to_owned();
        let head = head.trim_end_matches(':').to_owned();
        self.listeners
            .iter()
            .find(|(n, _, _)| *n == head)
            .map(|(_, k, e)| (*k, e.clone()))
    }
}

/// What an `EventKey` argument says.
enum KeyArg {
    /// `EventKey::<E>::of()`: the event type.
    Of(String),
    /// `EventKey::<E>::dynamic(name)`: the type and the name expression.
    Dynamic(String, String),
    /// `EventKey::dynamic(name)` whose type is not written there: the name.
    Named(String),
    Unknown,
}

fn strip_ref(text: &str) -> &str {
    let t = text.trim();
    t.strip_prefix("&mut ")
        .or_else(|| t.strip_prefix('&'))
        .map(str::trim)
        .unwrap_or(t)
}

fn event_type_from(init: &str, ty: Option<&str>) -> KeyArg {
    let init = init.trim();
    // EventKey::<E>::of() / EventKey::<E>::dynamic(x)
    if let Some(rest) = init
        .strip_prefix("EventKey::<")
        .or_else(|| init.split("EventKey::<").nth(1))
    {
        if let Some(close) = matching_angle(rest) {
            let ty = rest[..close].trim().to_owned();
            let after = rest[close + 1..].trim_start();
            if after.starts_with("::of()") || after.starts_with(":: of()") {
                return KeyArg::Of(ty);
            }
            if let Some(args) = after
                .strip_prefix("::dynamic(")
                .or_else(|| after.strip_prefix("::named("))
            {
                if let Some(end) = args.rfind(')') {
                    return KeyArg::Dynamic(ty, args[..end].trim().to_owned());
                }
            }
        }
    }
    if init.contains("EventKey::of()") || init.ends_with("EventKey::default()") {
        if let Some(ty) = ty {
            if let Some(inner) = ty
                .trim()
                .strip_prefix("EventKey<")
                .and_then(|t| t.strip_suffix('>'))
            {
                return KeyArg::Of(inner.trim().to_owned());
            }
        }
    }
    if let Some(args) = init
        .split("EventKey::dynamic(")
        .nth(1)
        .or_else(|| init.split("EventKey::named(").nth(1))
    {
        if let Some(end) = args.rfind(')') {
            let name = args[..end].trim().to_owned();
            if let Some(inner) = ty.and_then(|ty| {
                ty.trim()
                    .strip_prefix("EventKey<")
                    .and_then(|t| t.strip_suffix('>'))
            }) {
                return KeyArg::Dynamic(inner.trim().to_owned(), name);
            }
            return KeyArg::Named(name);
        }
    }
    KeyArg::Unknown
}

fn matching_angle(s: &str) -> Option<usize> {
    let mut depth = 1i32;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// A closure parameter: its pattern text without a type, and whether it is
/// used (`_`-patterns are not).
struct Param {
    pat: String,
    used: bool,
}

fn closure_params(rules: &Rules<'_>, closure: &syn::ExprClosure) -> Vec<Param> {
    closure
        .inputs
        .iter()
        .map(|p| {
            let pat = match p {
                syn::Pat::Type(t) => rules.text(&*t.pat).to_owned(),
                other => rules.text(other).to_owned(),
            };
            let used = !(pat == "_" || pat.starts_with('_'));
            Param { pat, used }
        })
        .collect()
}

fn param_ident(p: &Param) -> Option<&str> {
    let pat = p.pat.trim().trim_start_matches("mut ").trim();
    (!pat.is_empty()
        && pat.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !pat.starts_with('_'))
    .then_some(pat)
}

/// `Box::pin(EXPR)` → `EXPR`.
fn unbox_pin(expr: &syn::Expr) -> Option<&syn::Expr> {
    let expr = strip_block(expr);
    if let syn::Expr::Call(call) = expr {
        if let syn::Expr::Path(p) = &*call.func {
            let segs: Vec<_> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if segs.ends_with(&["Box".to_owned(), "pin".to_owned()]) && call.args.len() == 1 {
                return call.args.first();
            }
        }
    }
    None
}

/// A block `{ EXPR }` with no statements → `EXPR`.
fn strip_block(expr: &syn::Expr) -> &syn::Expr {
    if let syn::Expr::Block(b) = expr {
        if b.block.stmts.len() == 1 {
            if let syn::Stmt::Expr(inner, None) = &b.block.stmts[0] {
                return strip_block(inner);
            }
        }
    }
    expr
}

fn async_block(expr: &syn::Expr) -> Option<&syn::ExprAsync> {
    match strip_block(expr) {
        syn::Expr::Async(a) => Some(a),
        _ => None,
    }
}

impl<'a> Rules<'a> {
    /// The inside of an async block's braces, as text.
    fn async_inner(&self, a: &syn::ExprAsync) -> String {
        let block = self.text(&a.block);
        block[1..block.len() - 1].to_owned()
    }

    /// An old async listener closure → the new shape, or `None`.
    ///
    /// `|c, e, call| Box::pin(async move { B })` →
    /// `|e, call| { let c_ctx = call.ctx().clone(); async move { let c = &c_ctx; let e = &*e; B } }`
    fn async_listener(&self, closure: &syn::ExprClosure, pattern: bool) -> Option<String> {
        let params = closure_params(self, closure);
        let (ctx_p, key_p, ev_p, call_p) = match (pattern, params.len()) {
            (false, 3) => (&params[0], None, &params[1], &params[2]),
            (true, 4) => (&params[0], Some(&params[1]), &params[2], &params[3]),
            _ => return None,
        };
        let body = unbox_pin(&closure.body)?;
        let mv = if closure.capture.is_some() {
            "move "
        } else {
            ""
        };
        let call_name = if call_p.used {
            call_p.pat.clone()
        } else {
            "call".to_owned()
        };
        let call_used_by_prelude = ctx_p.used || key_p.is_some_and(|k| k.used);
        let call_pat = if call_p.used || call_used_by_prelude {
            call_name.clone()
        } else {
            "_".to_owned()
        };
        let ev_pat = if ev_p.used {
            ev_p.pat.clone()
        } else {
            "_".to_owned()
        };
        let mut outer = String::new();
        let mut inner = String::new();
        if let Some(c) = param_ident(ctx_p) {
            outer.push_str(&format!("let {c}_ctx = {call_name}.ctx().clone(); "));
            inner.push_str(&format!("let {c} = &{c}_ctx; "));
        }
        if let Some(k) = key_p.and_then(param_ident) {
            outer.push_str(&format!("let {k} = {call_name}.name().to_owned(); "));
        }
        if let Some(e) = param_ident(ev_p) {
            inner.push_str(&format!("let {e} = &*{e}; "));
        }
        let new_body = match async_block(body) {
            Some(a) => {
                let mv_async = if a.capture.is_some() { "move " } else { "" };
                let b = self.async_inner(a);
                let b = if let Some(k) = key_p.and_then(param_ident) {
                    b.replace(&format!("{k}.name()"), &format!("{k}.as_str()"))
                } else {
                    b
                };
                format!("async {mv_async}{{ {inner}{b}}}")
            }
            None => {
                if !inner.is_empty() || !outer.is_empty() {
                    return None;
                }
                self.text(body).to_owned()
            }
        };
        if outer.is_empty() {
            Some(format!("{mv}|{ev_pat}, {call_pat}| {new_body}"))
        } else {
            Some(format!(
                "{mv}|{ev_pat}, {call_pat}| {{ {outer}{new_body} }}"
            ))
        }
    }

    /// An old sync listener closure → the new shape, or `None`.
    fn sync_listener(&self, closure: &syn::ExprClosure, pattern: bool) -> Option<String> {
        let params = closure_params(self, closure);
        let (ctx_p, key_p, ev_p, call_p) = match (pattern, params.len()) {
            (false, 3) => (&params[0], None, &params[1], &params[2]),
            (true, 4) => (&params[0], Some(&params[1]), &params[2], &params[3]),
            _ => return None,
        };
        let mv = if closure.capture.is_some() {
            "move "
        } else {
            ""
        };
        let call_name = if call_p.used {
            call_p.pat.clone()
        } else {
            "call".to_owned()
        };
        let needs_call = call_p.used || ctx_p.used || key_p.is_some_and(|k| k.used);
        let call_pat = if needs_call {
            call_name.clone()
        } else {
            "_".to_owned()
        };
        let ev_pat = if ev_p.used {
            ev_p.pat.clone()
        } else {
            "_".to_owned()
        };
        let mut prelude = String::new();
        if let Some(c) = param_ident(ctx_p) {
            prelude.push_str(&format!("let {c} = &{call_name}.ctx().clone(); "));
        }
        if let Some(k) = key_p.and_then(param_ident) {
            prelude.push_str(&format!("let {k} = {call_name}.name().to_owned(); "));
        }
        if let Some(e) = param_ident(ev_p) {
            prelude.push_str(&format!("let {e} = &*{e}; "));
        }
        let body = self.text(&*closure.body);
        let body = match &*closure.body {
            syn::Expr::Block(b) => b
                .block
                .span()
                .join(b.span())
                .map(|_| body.to_owned())
                .unwrap_or_else(|| body.to_owned()),
            _ => body.to_owned(),
        };
        let body = if let Some(k) = key_p.and_then(param_ident) {
            body.replace(&format!("{k}.name()"), &format!("{k}.as_str()"))
        } else {
            body
        };
        if prelude.is_empty() {
            Some(format!("{mv}|{ev_pat}, {call_pat}| {body}"))
        } else {
            let body = body.trim();
            let inner = if body.starts_with('{') && body.ends_with('}') {
                body[1..body.len() - 1].to_owned()
            } else {
                body.to_owned()
            };
            Some(format!("{mv}|{ev_pat}, {call_pat}| {{ {prelude}{inner} }}"))
        }
    }

    /// The `EventKey` argument (`&key`, `&EventKey::<E>::of()`).
    fn key_arg(&self, expr: &syn::Expr) -> KeyArg {
        let text = strip_ref(self.text(expr));
        match event_type_from(text, None) {
            KeyArg::Unknown => {}
            known => return known,
        }
        if text.chars().all(|c| c.is_alphanumeric() || c == '_') {
            if let Some((init, ty)) = self.local(text) {
                return event_type_from(&init, ty.as_deref());
            }
        }
        KeyArg::Unknown
    }

    /// `x.events()` or a local bound to it: the context expression the
    /// bus came from, if any.
    fn bus_receiver(&self, expr: &syn::Expr) -> Option<(String, Option<String>)> {
        // returns (bus expression text, filter expression text)
        match expr {
            syn::Expr::MethodCall(m) if m.method == "events" && m.args.is_empty() => {
                Some((self.text(&*m.receiver).to_owned(), None))
            }
            syn::Expr::MethodCall(m) if m.method == "filter" && m.args.len() == 1 => {
                let (bus, _) = self.bus_receiver(&m.receiver)?;
                Some((bus, Some(self.text(&m.args[0]).to_owned())))
            }
            syn::Expr::Path(p) if p.path.segments.len() == 1 && p.qself.is_none() => {
                let name = p.path.segments[0].ident.to_string();
                let (init, _) = self.local(&name)?;
                let init = init.trim();
                let stripped = init.strip_suffix(".events()")?;
                Some((stripped.trim().to_owned(), None))
            }
            syn::Expr::Reference(r) => self.bus_receiver(&r.expr),
            syn::Expr::Paren(p) => self.bus_receiver(&p.expr),
            _ => None,
        }
    }

    /// `ctx.events().METHOD(&ctx, &key, …)` → the new form.
    fn rewrite_event_call(&mut self, call: &syn::ExprMethodCall) {
        let method = call.method.to_string();
        if !EVENT_METHODS.contains(&method.as_str()) {
            return;
        }
        let Some((_bus, filter)) = self.bus_receiver(&call.receiver) else {
            return;
        };
        let args: Vec<&syn::Expr> = call.args.iter().collect();
        if args.len() < 2 {
            return;
        }
        let ctx = strip_ref(self.text(args[0])).to_owned();
        let on_ctx = match &filter {
            Some(f) => format!("{ctx}.filter({f})"),
            None => ctx.clone(),
        };
        let registration = method.starts_with("on") || method == "once";
        if registration {
            if filter.is_some() {
                return;
            }
            let pattern = method.contains("pattern");
            let sync = method.contains("sync");
            let (ty, target) = if pattern {
                // &EventPattern::<E>::prefix(p) or a local
                let text = strip_ref(self.text(args[1]));
                let resolved = if let Some(rest) = text.strip_prefix("EventPattern::<") {
                    Some(rest.to_owned())
                } else if text.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    self.local(text).and_then(|(init, _)| {
                        init.trim()
                            .strip_prefix("EventPattern::<")
                            .map(str::to_owned)
                    })
                } else {
                    None
                };
                let Some(rest) = resolved else {
                    self.todo(call, "this event pattern could not be read: use ctx.listen::<E>().prefix(p).on(…)");
                    return;
                };
                let Some(close) = matching_angle(&rest) else {
                    return;
                };
                let ty = rest[..close].trim().to_owned();
                let after = rest[close + 1..].trim_start();
                let Some(p) = after
                    .strip_prefix("::prefix(")
                    .and_then(|a| a.rfind(')').map(|e| a[..e].to_owned()))
                else {
                    return;
                };
                (ty, format!(".prefix({p})"))
            } else {
                let turbofish = call
                    .turbofish
                    .as_ref()
                    .and_then(|t| t.args.first())
                    .map(|a| self.text(a).trim().to_owned());
                let from_listener = || {
                    args.get(2)
                        .and_then(|l| self.listener_of(l))
                        .map(|(_, e)| e)
                };
                match self.key_arg(args[1]) {
                    KeyArg::Of(ty) => (ty, String::new()),
                    KeyArg::Dynamic(ty, name) => (ty, format!(".name({name})")),
                    KeyArg::Named(name) => match turbofish.clone().or_else(from_listener) {
                        Some(ty) => (ty, format!(".name({name})")),
                        None => {
                            self.todo(call, "the event key's type could not be found: use ctx.listen::<E>().name(n).on(|event, call| async move { … })");
                            return;
                        }
                    },
                    KeyArg::Unknown
                        if turbofish.is_some() && {
                            let key_text = strip_ref(self.text(args[1]));
                            key_text.ends_with("EventKey::of()")
                                || key_text.ends_with("EventKey::default()")
                        } =>
                    {
                        (turbofish.clone().unwrap(), String::new())
                    }
                    KeyArg::Unknown => {
                        // `&EventKey::of()`: the type may come from a struct or fn listener
                        let from_listener = args
                            .get(2)
                            .and_then(|l| self.listener_of(l))
                            .map(|(_, e)| e);
                        let closure_ty = args.get(2).and_then(|l| match l {
                            syn::Expr::Closure(c) => c.inputs.iter().nth(1).and_then(|p| match p {
                                syn::Pat::Type(t) => Some(
                                    self.text(&*t.ty)
                                        .trim_start_matches('&')
                                        .trim_start_matches("'a")
                                        .trim()
                                        .to_owned(),
                                ),
                                _ => None,
                            }),
                            _ => None,
                        });
                        let key_text = strip_ref(self.text(args[1]));
                        match from_listener.or(closure_ty).or(turbofish.clone()) {
                            Some(ty)
                                if key_text.ends_with("EventKey::of()")
                                    || key_text.ends_with("EventKey::default()") =>
                            {
                                (ty, String::new())
                            }
                            // a key from elsewhere: listen under its name
                            // rutis-loader's volatile_key(ctx) is the name itself now
                            Some(ty) if key_text.contains("volatile_key(") => {
                                (ty, format!(".name({key_text})"))
                            }
                            Some(ty) if !self.local_is_of(args[1]) => {
                                (ty, format!(".name({key_text}.name())"))
                            }
                            _ => {
                                self.todo(call, "the event key's type could not be found: use ctx.on::<E>(|event, call| async move { … })");
                                return;
                            }
                        }
                    }
                }
            };
            let Some(listener) = args.get(2) else { return };
            if !matches!(listener, syn::Expr::Closure(_)) {
                if let Some((kind, ev)) = self.listener_of(listener) {
                    let ty = if ty.is_empty() || ty == "?" {
                        ev.clone()
                    } else {
                        ty.clone()
                    };
                    let l = self.text(*listener).to_owned();
                    let new_listener = match kind {
                        ListenerKindOf::AsyncStruct => format!("{{ let listener = std::sync::Arc::new({l}); move |event, call| {{ let listener = listener.clone(); async move {{ let ctx = call.ctx().clone(); listener.on_event(&ctx, &event, call).await }} }} }}"),
                        ListenerKindOf::PatternStruct => format!("{{ let listener = std::sync::Arc::new({l}); move |event, call| {{ let listener = listener.clone(); async move {{ let ctx = call.ctx().clone(); let name = call.name().to_owned(); listener.on_event(&ctx, name, &event, call).await }} }} }}"),
                        ListenerKindOf::SyncStruct => format!("{{ let listener = {l}; move |event, call| {{ let ctx = call.ctx().clone(); listener.on_event_sync(&ctx, &event, call) }} }}"),
                        ListenerKindOf::SyncPatternStruct => format!("{{ let listener = {l}; move |event, call| {{ let ctx = call.ctx().clone(); let name = call.name().to_owned(); listener.on_event_sync(&ctx, name, &event, call) }} }}"),
                        ListenerKindOf::AsyncFn => format!("|event, call| async move {{ let ctx = call.ctx().clone(); {l}(&ctx, &event, call).await }}"),
                        ListenerKindOf::SyncFn => format!("|event, call| {{ let ctx = call.ctx().clone(); {l}(&ctx, &event, call) }}"),
                    };
                    let mut opts = String::new();
                    if method == "once" {
                        opts.push_str(".once()");
                    }
                    if method.ends_with("_opt") {
                        if let Some(o) = args.get(3) {
                            let o = self.text(*o);
                            for (flag, c) in [
                                ("prepend", ".prepend()"),
                                ("once", ".once()"),
                                ("global", ".global()"),
                            ] {
                                if o.contains(&format!(".{flag}(true)"))
                                    || o.contains(&format!("{flag}: true"))
                                {
                                    opts.push_str(c);
                                }
                            }
                        }
                    }
                    let finish = if sync { "on_sync" } else { "on" };
                    let text = if target.is_empty() && opts.is_empty() {
                        format!("{ctx}.{finish}::<{ty}>({new_listener})")
                    } else {
                        format!("{ctx}.listen::<{ty}>(){target}{opts}.{finish}({new_listener})")
                    };
                    self.edit(call, text);
                    return;
                }
            }
            let new_listener = match listener {
                syn::Expr::Closure(c) => {
                    if sync {
                        self.sync_listener(c, pattern)
                    } else {
                        self.async_listener(c, pattern)
                    }
                }
                _ => None,
            };
            let Some(new_listener) = new_listener else {
                self.todo(call, "listeners are closures now: ctx.on::<E>(|event, call| async move { … }) or ctx.on_sync::<E>(|event, call| …)");
                return;
            };
            let mut opts = String::new();
            if method == "once" {
                opts.push_str(".once()");
            }
            if method.ends_with("_opt") {
                if let Some(o) = args.get(3) {
                    let o = self.text(*o);
                    for (flag, call) in [
                        ("prepend", ".prepend()"),
                        ("once", ".once()"),
                        ("global", ".global()"),
                    ] {
                        if o.contains(&format!(".{flag}(true)"))
                            || o.contains(&format!("{flag}: true"))
                        {
                            opts.push_str(call);
                        }
                    }
                    if !o.contains("(true)") && !o.contains(": true") && !o.contains("default()") {
                        self.todo(call, "these EventOptions could not be read: use ctx.listen::<E>().prepend() / .once() / .global()");
                    }
                }
            }
            let finish = if sync { "on_sync" } else { "on" };
            let text = if target.is_empty() && opts.is_empty() {
                format!("{ctx}.{finish}::<{ty}>({new_listener})")
            } else {
                format!("{ctx}.listen::<{ty}>(){target}{opts}.{finish}({new_listener})")
            };
            self.edit(call, text);
            return;
        }
        // dispatch
        let key = self.key_arg(args[1]);
        let on_ctx = if let KeyArg::Dynamic(_, name) | KeyArg::Named(name) = &key {
            if filter.is_some() {
                self.todo(
                    call,
                    "a filtered dispatch under another name: rewrite by hand",
                );
                return;
            }
            self.todo(call, "a typed event sent under another name uses rutis::internal::CtxInternal::named; prefer a type that overrides Event::name");
            format!("{ctx}.named({name})")
        } else {
            on_ctx
        };
        let on_ctx = match key {
            KeyArg::Unknown
                if !strip_ref(self.text(args[1])).contains("::of()")
                    && !self.local_is_of(args[1]) =>
            {
                // a key from elsewhere: send under its name, which is what
                // the old call did whatever the key is
                if filter.is_some() {
                    self.todo(
                        call,
                        "a filtered dispatch under a key's name: rewrite by hand",
                    );
                    return;
                }
                self.todo(call, "a typed event sent under a key's name uses rutis::internal::CtxInternal::named; if the key is EventKey::of(), use ctx.emit(event) and friends");
                format!("{ctx}.named({}.name())", strip_ref(self.text(args[1])))
            }
            _ => on_ctx,
        };
        let event = args.get(2).map(|e| self.event_value(e));
        let Some(event) = event else { return };
        let text = match method.as_str() {
            "emit" | "parallel" | "serial" => format!("{on_ctx}.{method}({event})"),
            "bail_sync" => format!("{on_ctx}.bail({event})"),
            "waterfall" => {
                let term = args.get(3).map(|t| self.text(*t).trim().to_owned());
                match term.as_deref() {
                    None | Some("()") => format!("{on_ctx}.waterfall({event})"),
                    Some(_) => {
                        let Some(t) = args.get(3).and_then(|t| self.async_terminal(t, &ctx)) else {
                            self.todo(call, "this waterfall terminal could not be rewritten: .terminal(|event| async move { … })");
                            return;
                        };
                        format!("{on_ctx}.waterfall({event}).terminal({t})")
                    }
                }
            }
            "waterfall_sync" => {
                let Some(t) = args.get(3).and_then(|t| self.sync_terminal(t, &ctx)) else {
                    self.todo(
                        call,
                        "this waterfall_sync terminal could not be rewritten: |event| …",
                    );
                    return;
                };
                format!("{on_ctx}.waterfall_sync({event}, {t})")
            }
            _ => return,
        };
        self.edit(call, text);
    }

    fn local_is_of(&self, expr: &syn::Expr) -> bool {
        let text = strip_ref(self.text(expr));
        self.local(text)
            .is_some_and(|(init, _)| init.contains("::of()"))
    }

    /// The event value: `Arc::new(v)` → `v`, `&v` → `v`.
    fn event_value(&self, expr: &syn::Expr) -> String {
        if let syn::Expr::Call(c) = expr {
            if let syn::Expr::Path(p) = &*c.func {
                let segs: Vec<_> = p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                if segs.ends_with(&["Arc".to_owned(), "new".to_owned()]) && c.args.len() == 1 {
                    return self.text(&c.args[0]).to_owned();
                }
            }
        }
        strip_ref(self.text(expr)).to_owned()
    }

    fn async_terminal(&self, expr: &syn::Expr, ctx: &str) -> Option<String> {
        if let syn::Expr::Path(p) = expr {
            // a fn item `fn t<'a>(ctx: &'a Ctx, e: &'a E) -> BoxFuture<'a, …>`
            let f = self.text(p);
            return Some(format!("{{ let __ctx = {ctx}.clone(); move |event| async move {{ {f}(&__ctx, &event).await }} }}"));
        }
        let syn::Expr::Closure(c) = expr else {
            return None;
        };
        let params = closure_params(self, c);
        if params.len() != 2 {
            return None;
        }
        let body = unbox_pin(&c.body)?;
        let a = async_block(body)?;
        let inner = self.async_inner(a);
        let mut prelude = String::new();
        if let Some(name) = param_ident(&params[1]) {
            prelude.push_str(&format!("let {name} = &*{name}; "));
        }
        let ev = param_ident(&params[1]).unwrap_or("_").to_owned();
        if let Some(cname) = param_ident(&params[0]) {
            return Some(format!("{{ let __ctx = {ctx}.clone(); move |{ev}| async move {{ let {cname} = &__ctx; {prelude}{inner}}} }}"));
        }
        let mv = if c.capture.is_some() { "move " } else { "" };
        Some(format!("{mv}|{ev}| async move {{ {prelude}{inner}}}"))
    }

    fn sync_terminal(&self, expr: &syn::Expr, ctx: &str) -> Option<String> {
        if let syn::Expr::Path(p) = expr {
            let f = self.text(p);
            return Some(format!(
                "{{ let __ctx = {ctx}.clone(); move |event| {f}(&__ctx, &event) }}"
            ));
        }
        let syn::Expr::Closure(c) = expr else {
            return None;
        };
        let params = closure_params(self, c);
        if params.len() != 2 {
            return None;
        }
        let body = self.text(&*c.body);
        let mut prelude = String::new();
        if let Some(name) = param_ident(&params[1]) {
            prelude.push_str(&format!("let {name} = &*{name}; "));
        }
        let ev = param_ident(&params[1]).unwrap_or("_").to_owned();
        if let Some(cname) = param_ident(&params[0]) {
            return Some(format!("{{ let __ctx = {ctx}.clone(); move |{ev}| {{ let {cname} = &__ctx; {prelude}{body} }} }}"));
        }
        let mv = if c.capture.is_some() { "move " } else { "" };
        Some(format!("{mv}|{ev}| {{ {prelude}{body} }}"))
    }

    /// `fn apply<'a>(&'a self, ctx: &'a Ctx, deps: Self::Deps) -> BoxFuture<…> { Box::pin(async move { B }) }`
    /// in an `impl Plugin` → `async fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> rutis::Result { B }`.
    fn rewrite_typed_apply(&mut self, f: &syn::ImplItemFn) {
        if f.sig.ident != "apply" || f.sig.asyncness.is_some() || f.sig.inputs.len() != 3 {
            return;
        }
        let ret = match &f.sig.output {
            syn::ReturnType::Type(_, t) => self.text(&**t).to_owned(),
            _ => return,
        };
        if !ret.contains("BoxFuture") {
            return;
        }
        let stmts = &f.block.stmts;
        let body_expr = match stmts.as_slice() {
            [syn::Stmt::Expr(e, None)] => e,
            _ => {
                self.todo(f, "write apply as `async fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> rutis::Result`");
                return;
            }
        };
        let Some(inner) = unbox_pin(body_expr).and_then(async_block) else {
            self.todo(f, "write apply as `async fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> rutis::Result`");
            return;
        };
        let inputs: Vec<String> = f
            .sig
            .inputs
            .iter()
            .skip(1)
            .map(|arg| match arg {
                syn::FnArg::Typed(t) => {
                    let pat = self.text(&*t.pat).to_owned();
                    let ty = self.text(&*t.ty).replace("&'a ", "&").replace("&'a", "&");
                    format!("{pat}: {ty}")
                }
                other => self.text(other).to_owned(),
            })
            .collect();
        let body = self.async_inner(inner);
        let mut body = body.replace("Ok(Effect::Done)", "Ok(())");
        if body.contains("Effect::") {
            self.todo(f, "apply returned a cleanup: register it with ctx.effect(|| { …; async move { … } }) instead");
            body = body.clone();
        }
        let vis = match &f.vis {
            syn::Visibility::Inherited => String::new(),
            v => format!("{} ", self.text(v)),
        };
        let text = format!(
            "{vis}async fn apply(&self, {}) -> {} {{{body}}}",
            inputs.join(", "),
            self.result_path()
        );
        // replace from the signature start (after attributes) to the block end
        let (start, _) = self.index.range(self.src, f.sig.span());
        let (_, end) = self.range(&f.block);
        let start = match &f.vis {
            syn::Visibility::Inherited => start,
            v => self.range(v).0,
        };
        self.edit_range(start, end, text);
    }

    /// `fn injects(&self) -> &[TypeKey] { B }` → `-> Vec<TypeKey> { (B).to_vec() }`.
    fn rewrite_injects(&mut self, f: &syn::ImplItemFn) {
        if f.sig.ident != "injects" {
            return;
        }
        let syn::ReturnType::Type(_, ty) = &f.sig.output else {
            return;
        };
        let syn::Type::Reference(r) = &**ty else {
            return;
        };
        let syn::Type::Slice(slice) = &*r.elem else {
            return;
        };
        let elem = self.text(&*slice.elem).to_owned();
        self.edit(&**ty, format!("Vec<{elem}>"));
        let stmts = &f.block.stmts;
        let plain = !self.text(&f.block).contains("return ")
            && !matches!(stmts.last(), Some(syn::Stmt::Expr(tail, None)) if expr_has_attrs(tail));
        if !plain {
            self.todo(f, "injects returns a Vec<TypeKey> now");
            return;
        }
        if let Some(syn::Stmt::Expr(tail, None)) = stmts.last() {
            let t = self.text(tail).trim().to_owned();
            let simple = |e: &syn::Expr| {
                matches!(
                    e,
                    syn::Expr::Path(_) | syn::Expr::Field(_) | syn::Expr::MethodCall(_)
                )
            };
            let replacement = match tail {
                // `self.0.injects()` already returns a `Vec` once that impl is migrated
                syn::Expr::MethodCall(m) if m.method == "injects" => t,
                syn::Expr::Reference(r) if simple(&r.expr) => {
                    format!("{}.to_vec()", self.text(&*r.expr).trim())
                }
                e if simple(e) => format!("{t}.to_vec()"),
                _ => format!("({t}).to_vec()"),
            };
            self.edit(tail, replacement);
        } else {
            self.todo(f, "injects returns a Vec<TypeKey> now");
        }
    }

    /// `ctx.effect(|| { …; Effect::Disposer(Box::new(move || B)) })` →
    /// `ctx.effect(|| { …; async move { B } })`.
    fn rewrite_effect(&mut self, call: &syn::ExprMethodCall) {
        if call.method != "effect" || call.args.len() != 1 {
            return;
        }
        // `ctx.effect({ let x = …; move || … })`
        let mut arg = &call.args[0];
        if let syn::Expr::Block(b) = arg {
            if let Some(syn::Stmt::Expr(tail, None)) = b.block.stmts.last() {
                arg = tail;
            }
        }
        let syn::Expr::Closure(c) = arg else { return };
        if !c.inputs.is_empty() {
            return;
        }
        // the tail expression of the closure body
        let tail: &syn::Expr = match &*c.body {
            syn::Expr::Block(b) => match b.block.stmts.last() {
                Some(syn::Stmt::Expr(e, None)) => e,
                _ => return,
            },
            e => e,
        };
        let Some(replacement) = self.cleanup_future(tail) else {
            if self.text(tail).contains("Effect::") {
                self.todo(call, "this cleanup could not be rewritten: ctx.effect(|| { …; async move { cleanup } })");
            }
            return;
        };
        self.edit(tail, replacement);
    }

    /// `Effect::Done` / `Effect::Disposer(Box::new(move || B))` /
    /// `Effect::AsyncDisposer(Box::new(move || Box::pin(async move { B })))` → a future.
    fn cleanup_future(&self, expr: &syn::Expr) -> Option<String> {
        let expr = strip_block(expr);
        match expr {
            syn::Expr::Path(p) if self.text(p).ends_with("Effect::Done") => {
                Some("async { Ok(()) }".to_owned())
            }
            syn::Expr::Call(call) => {
                let func = self.text(&*call.func);
                let arg = call.args.first()?;
                // Box::new(closure)
                let syn::Expr::Call(boxed) = arg else {
                    return None;
                };
                if !self.text(&*boxed.func).ends_with("Box::new") {
                    return None;
                }
                let syn::Expr::Closure(inner) = boxed.args.first()? else {
                    return None;
                };
                if !inner.inputs.is_empty() {
                    return None;
                }
                if func.ends_with("Effect::Disposer") {
                    let body = self.text(&*inner.body).trim();
                    let body = if body.starts_with('{') && body.ends_with('}') {
                        body[1..body.len() - 1].trim()
                    } else {
                        body
                    };
                    Some(format!("async move {{ {body} }}"))
                } else if func.ends_with("Effect::AsyncDisposer") {
                    let fut = unbox_pin(&inner.body)?;
                    match fut {
                        // `Box::pin(async move { B })` → `async move { B }`
                        syn::Expr::Async(a) if a.capture.is_some() => Some(self.text(a).to_owned()),
                        _ => Some(format!("async move {{ {}.await }}", self.text(fut))),
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// `.map_err(|e| ErrorKind::PluginFailed(Box::new(e)))` and the like → nothing.
    fn rewrite_map_err(&mut self, call: &syn::ExprMethodCall) {
        if call.method != "map_err" || call.args.len() != 1 {
            return;
        }
        let syn::Expr::Closure(c) = &call.args[0] else {
            return;
        };
        if c.inputs.len() != 1 {
            return;
        }
        let param = closure_params(self, c);
        let Some(name) = param_ident(&param[0]) else {
            return;
        };
        let body = self.text(&*c.body).replace(' ', "");
        // only where the error is a `std::error::Error` for sure: then `?`
        // converts it as the closure did (`PluginFailed(e.into())` also
        // takes a `String`, which `?` does not, so it stays)
        let wrap = [
            format!("ErrorKind::PluginFailed(Box::new({name}))"),
            format!("{}::from({name})", self.error_name),
            format!("{name}.into()"),
        ];
        if !wrap.contains(&body) {
            return;
        }
        // keep the receiver, drop `.map_err(…)`
        let (_, recv_end) = self.range(&*call.receiver);
        let (_, end) = self.range(call);
        self.edit_range(recv_end, end, String::new());
    }

    /// Typed service reads and writes by key.
    fn rewrite_service_call(&mut self, call: &syn::ExprMethodCall) {
        let method = call.method.to_string();
        let turbofish = call.turbofish.as_ref().map(|t| self.text(t).to_owned());
        let recv = self.text(&*call.receiver).to_owned();
        let of_type = |rules: &Self, expr: &syn::Expr| -> Option<String> {
            let t = rules.text(expr).trim();
            let rest = t.strip_prefix("TypeKey::of::<").or_else(|| {
                t.split("TypeKey::of::<")
                    .nth(1)
                    .filter(|_| t.contains("::TypeKey::of::<"))
            })?;
            let close = matching_angle(rest)?;
            (rest[close + 1..].trim() == "()").then(|| rest[..close].trim().to_owned())
        };
        match method.as_str() {
            "provide_as" if call.args.len() == 2 => {
                let key = &call.args[0];
                let value = self.text(&call.args[1]).to_owned();
                let text = match (of_type(self, key), typed_key(self.text(key))) {
                    (Some(ty), _) => format!("{recv}.provide_as::<{ty}>({value})"),
                    (None, Some(k)) => format!("{recv}.provide_at({k}, {value})"),
                    (None, None) => {
                        let key = self.text(key).to_owned();
                        // the service type stays named: the value may be a
                        // concrete type provided as a trait object
                        let t = turbofish.clone().unwrap_or_default();
                        self.todo(call, "provide_at takes a rutis::host::Key<T> (Key::named, Key::dynamic, Key::of)");
                        format!("{recv}.provide_at{t}({key}, {value})")
                    }
                };
                self.edit(call, text);
            }
            "get_as" | "require_as" if call.args.len() == 1 => {
                let new = if method == "get_as" { "get" } else { "require" };
                let key = &call.args[0];
                let text = match (of_type(self, key), typed_key(self.text(key))) {
                    (Some(ty), _) => format!("{recv}.{new}::<{ty}>()"),
                    (None, Some(k)) => format!("{recv}.{new}_at(&{k})"),
                    (None, None) => {
                        let k = self.text(key).to_owned();
                        let t = turbofish.clone().unwrap_or_default();
                        self.todo(call, format!("{new}_at takes a &rutis::host::Key<T>"));
                        format!("{recv}.{new}_at{t}(&{k})")
                    }
                };
                self.edit(call, text);
            }
            "provide_as_with_check" | "provide_mut_as" => {
                let new = if method == "provide_as_with_check" {
                    "provide_checked"
                } else {
                    "provide_mut_at"
                };
                let (s, e) = self.index.range(self.src, call.method.span());
                self.edit_range(s, e, new.to_owned());
            }
            "register_with" if call.args.len() == 2 => {
                self.todo(call, "Builtins::register_with takes |build, config| -> Result<P> with P: Plugin now; a per-copy PluginFactory goes through a Resolver returning Resolved::new(..).with_scoped(..)");
            }
            "register_fn" if call.args.len() == 2 => {
                if let syn::Expr::Closure(c) = &call.args[1] {
                    let body = self.text(&*c.body).to_owned();
                    if let Some(inner) = body
                        .trim()
                        .strip_prefix("Ok(Box::new(")
                        .and_then(|b| b.strip_suffix("))"))
                    {
                        let params = self.text(&c.inputs).to_owned();
                        let mv = if c.capture.is_some() { "move " } else { "" };
                        let name = self.text(&call.args[0]).to_owned();
                        // `register_fn::<C, _>` names the config type; `register` has three parameters
                        let config = call
                            .turbofish
                            .as_ref()
                            .and_then(|t| t.args.first())
                            .map(|a| self.text(a).trim().to_owned());
                        let fish = config.map(|c| format!("::<{c}, _, _>")).unwrap_or_default();
                        self.edit(
                            call,
                            format!("{recv}.register{fish}({name}, {mv}|{params}| Ok({inner}))"),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// `ErrorKind::X…` built where an error is expected → `Error::from(ErrorKind::X…)`.
    fn wrap_kind_expr(&mut self, expr: &syn::Expr) {
        let path = match expr {
            syn::Expr::Path(p) => &p.path,
            syn::Expr::Call(c) => match &*c.func {
                syn::Expr::Path(p) => &p.path,
                _ => return,
            },
            syn::Expr::Struct(s) => &s.path,
            _ => return,
        };
        let segs: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if !is_rutis_kind_path(&segs) {
            return;
        }
        let text = self.text(expr).to_owned();
        let (start, _) = self.range(expr);
        let before = &self.src[..start];
        if before
            .trim_end()
            .ends_with(&format!("{}::from(", self.error_path()))
            || before.trim_end().ends_with("::from(")
        {
            return;
        }
        let after = &self.src[self.range(expr).1..];
        if after.starts_with(".into()") {
            return;
        }
        self.edit(expr, format!("{}::from({text})", self.error_path()));
    }

    /// `match e { ErrorKind::X => … }` → `match &*e { … }`: an error
    /// dereferences to the kind the host sees.
    fn kind_scrutinee(&mut self, scrutinee: &syn::Expr, pats: &[&syn::Pat]) {
        let matches_kind = pats.iter().any(|p| pat_is_kind(p));
        if !matches_kind {
            return;
        }
        let t = self.text(scrutinee).trim().to_owned();
        if let Some(new) = kind_of(&t) {
            self.edit(scrutinee, new);
        }
    }

    /// `matches!(e, ErrorKind::X)` inside any macro → `matches!(&*e, …)`.
    fn matches_in_macros(&mut self, file: &syn::File) {
        let stream: TokenStream = match self.src.parse() {
            Ok(s) => s,
            Err(_) => return,
        };
        let _ = file;
        self.scan_matches(stream);
    }

    fn scan_matches(&mut self, stream: TokenStream) {
        let tokens: Vec<TokenTree> = stream.into_iter().collect();
        for (i, tt) in tokens.iter().enumerate() {
            if let TokenTree::Group(g) = tt {
                let is_matches = i >= 2
                    && matches!(&tokens[i - 1], TokenTree::Punct(p) if p.as_char() == '!')
                    && matches!(&tokens[i - 2], TokenTree::Ident(id) if id == "matches")
                    && g.delimiter() == Delimiter::Parenthesis;
                if is_matches {
                    let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                    if let Some(comma) = inner
                        .iter()
                        .position(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == ','))
                    {
                        let pat_is_kind = inner[comma + 1..]
                            .iter()
                            .any(|t| matches!(t, TokenTree::Ident(id) if id == "ErrorKind"));
                        if pat_is_kind && comma > 0 {
                            let (s, _) = self.index.range(self.src, inner[0].span());
                            let (_, e) = self.index.range(self.src, inner[comma - 1].span());
                            let t = self.src[s..e].trim().to_owned();
                            if let Some(new) = kind_of(&t) {
                                self.edit_range(s, e, new);
                            }
                        }
                    }
                }
                self.scan_matches(g.stream());
            }
        }
    }
}

/// `TypeKey::keyed::<T>("x")` and the like → the typed `Key::<T>::named("x")`.
/// The scrutinee matched against `ErrorKind` patterns, as the kind:
/// `e` → `&*e`, `&e` → `&*e`, `*e` and `&*e` stay.
fn kind_of(t: &str) -> Option<String> {
    let t = t.trim();
    if t.starts_with('*')
        || t.starts_with("&*")
        || t.ends_with(".kind()")
        || t.ends_with(".into_kind()")
    {
        return None;
    }
    let inner = t.strip_prefix('&').map(str::trim).unwrap_or(t);
    if inner
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        Some(format!("&*{inner}"))
    } else {
        Some(format!("&*({inner})"))
    }
}

fn typed_key(text: &str) -> Option<String> {
    let t = text.trim();
    let t = t.strip_prefix('&').unwrap_or(t).trim();
    // a trailing `.with_instance(id)`
    if let Some(i) = t.rfind(".with_instance(") {
        let inner = typed_key(&t[..i])?;
        let arg = t[i + ".with_instance(".len()..].strip_suffix(')')?;
        return Some(format!("{inner}.instance({arg})"));
    }
    let after = t.split("TypeKey::").nth(1)?;
    let (ctor, rest) = after.split_once("::<")?;
    let close = matching_angle(rest)?;
    let ty = rest[..close].trim();
    let args = rest[close + 1..]
        .trim()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    Some(match ctor {
        "of" if args.trim().is_empty() => format!("Key::<{ty}>::of()"),
        "keyed" => format!("Key::<{ty}>::named({args})"),
        "keyed_dynamic" => format!("Key::<{ty}>::dynamic({args})"),
        "instance" => format!("Key::<{ty}>::of().instance({args})"),
        _ => return None,
    })
}

fn pat_is_kind(p: &syn::Pat) -> bool {
    let path = match p {
        syn::Pat::Path(p) => &p.path,
        syn::Pat::TupleStruct(t) => &t.path,
        syn::Pat::Struct(s) => &s.path,
        syn::Pat::Or(o) => return o.cases.iter().any(pat_is_kind),
        syn::Pat::Reference(r) => return pat_is_kind(&r.pat),
        syn::Pat::Ident(i) => return i.subpat.as_ref().is_some_and(|(_, p)| pat_is_kind(p)),
        _ => return false,
    };
    let segs: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    is_rutis_kind_path(&segs)
}

impl<'ast, 'a> Visit<'ast> for Rules<'a> {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.rewrite_listener_fn(f);
        self.fn_lets.push(collect_lets(self, &f.block));
        visit::visit_item_fn(self, f);
        self.fn_lets.pop();
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.rewrite_injects(f);
        self.fn_lets.push(collect_lets(self, &f.block));
        visit::visit_impl_item_fn(self, f);
        self.fn_lets.pop();
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let is_plugin = i
            .trait_
            .as_ref()
            .and_then(|(_, p, _)| p.segments.last())
            .is_some_and(|s| s.ident == "Plugin");
        if is_plugin {
            for item in &i.items {
                if let syn::ImplItem::Fn(f) = item {
                    self.rewrite_typed_apply(f);
                }
            }
        }
        self.rewrite_listener_impl(i);
        visit::visit_item_impl(self, i);
    }

    fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
        // lets inside closures (async blocks of tests) resolve keys too
        if let syn::Expr::Block(b) = &*c.body {
            self.fn_lets.push(collect_lets(self, &b.block));
            visit::visit_expr_closure(self, c);
            self.fn_lets.pop();
        } else {
            visit::visit_expr_closure(self, c);
        }
    }

    fn visit_expr_async(&mut self, a: &'ast syn::ExprAsync) {
        self.fn_lets.push(collect_lets(self, &a.block));
        visit::visit_expr_async(self, a);
        self.fn_lets.pop();
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.rewrite_event_call(call);
        self.rewrite_effect(call);
        self.rewrite_map_err(call);
        self.rewrite_service_call(call);
        visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        // Typed::new(p) → p
        if let syn::Expr::Path(p) = &*call.func {
            let segs: Vec<_> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if segs.len() >= 2 && segs[segs.len() - 2] == "Typed" {
                match segs.last().map(String::as_str) {
                    Some("new") if call.args.len() == 1 => {
                        let inner = self.text(&call.args[0]).to_owned();
                        self.edit(call, inner);
                    }
                    Some("with_keys") => self.todo(
                        call,
                        "Typed::with_keys: depend on named services through rutis::host::DynPlugin",
                    ),
                    _ => {}
                }
            }
        }
        // `Arc::new(ErrorKind::X)` is the shared form and stays a kind
        let shared = self.text(&*call.func).trim().ends_with("Arc::new");
        if !shared {
            for arg in &call.args {
                self.wrap_kind_expr(arg);
            }
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr(&mut self, e: &'ast syn::Expr) {
        match e {
            syn::Expr::Return(r) => {
                if let Some(v) = &r.expr {
                    self.wrap_kind_expr(v);
                }
            }
            syn::Expr::Closure(c) => {
                self.wrap_kind_expr(&c.body);
            }
            _ => {}
        }
        visit::visit_expr(self, e);
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        if let Some(init) = &l.init {
            if pat_is_kind(&l.pat) {
                self.kind_scrutinee(&init.expr, &[&l.pat]);
            } else {
                self.wrap_kind_expr(&init.expr);
            }
        }
        visit::visit_local(self, l);
    }

    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        let pats: Vec<&syn::Pat> = m.arms.iter().map(|a| &a.pat).collect();
        self.kind_scrutinee(&m.expr, &pats);
        for arm in &m.arms {
            self.wrap_kind_expr(&arm.body);
        }
        visit::visit_expr_match(self, m);
    }

    fn visit_expr_let(&mut self, l: &'ast syn::ExprLet) {
        self.kind_scrutinee(&l.expr, &[&l.pat]);
        visit::visit_expr_let(self, l);
    }

    fn visit_block(&mut self, b: &'ast syn::Block) {
        if let Some(syn::Stmt::Expr(tail, None)) = b.stmts.last() {
            self.wrap_kind_expr(tail);
        }
        self.drop_unused_keys(b);
        visit::visit_block(self, b);
    }
}

impl Rules<'_> {
    /// A body `{ STMTS; Box::pin(F) }` as the body of an `async fn`:
    /// `{ STMTS; F.await }`, or the async block's contents inlined.
    fn async_fn_body(&self, block: &syn::Block) -> Option<String> {
        let (last, init) = block.stmts.split_last()?;
        let syn::Stmt::Expr(tail, None) = last else {
            return None;
        };
        let pinned = unbox_pin(tail)?;
        let mut out = String::new();
        for stmt in init {
            out.push_str(self.text(stmt));
            out.push('\n');
        }
        match async_block(pinned) {
            Some(a) => out.push_str(&self.async_inner(a)),
            None => out.push_str(&format!("{}.await", self.text(pinned))),
        }
        Some(out)
    }

    /// `impl Listener<E> for S { fn call<'a>(…) -> BoxFuture<…> {…} }` →
    /// `impl S { async fn on_event(&self, ctx: &Ctx, event: &E, call: Call<E>) -> R {…} }`.
    fn rewrite_listener_impl(&mut self, i: &syn::ItemImpl) {
        let Some((_, path, _)) = &i.trait_ else {
            return;
        };
        let Some(last) = path.segments.last() else {
            return;
        };
        let kind = last.ident.to_string();
        if !matches!(
            kind.as_str(),
            "Listener" | "SyncListener" | "PatternListener" | "SyncPatternListener"
        ) {
            return;
        }
        let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
            return;
        };
        let Some(ev) = args.args.first().map(|a| self.text(a).to_owned()) else {
            return;
        };
        let [syn::ImplItem::Fn(f)] = i.items.as_slice() else {
            self.todo(i, "listeners are closures now: turn this into a method and register a closure calling it");
            return;
        };
        let pattern = kind.contains("Pattern");
        let sync = kind.starts_with("Sync");
        let names: Vec<String> = f
            .sig
            .inputs
            .iter()
            .skip(1)
            .map(|a| match a {
                syn::FnArg::Typed(t) => self.text(&*t.pat).to_owned(),
                _ => "_".to_owned(),
            })
            .collect();
        let expected = if pattern { 4 } else { 3 };
        if names.len() != expected {
            return;
        }
        let ret = match &f.sig.output {
            syn::ReturnType::Type(_, t) => self.text(&**t).to_owned(),
            _ => return,
        };
        let krate = self.krate;
        let (params, body, ret, fname, asyncness) = if sync {
            let ps = if pattern {
                format!("{}: &{krate}::Ctx, {}: String, {}: &{ev}, {}: {krate}::events::SyncCall<'_, {ev}>", names[0], names[1], names[2], names[3])
            } else {
                format!(
                    "{}: &{krate}::Ctx, {}: &{ev}, {}: {krate}::events::SyncCall<'_, {ev}>",
                    names[0], names[1], names[2]
                )
            };
            let b = self.text(&f.block);
            (ps, b[1..b.len() - 1].to_owned(), ret, "on_event_sync", "")
        } else {
            let Some(b) = self.async_fn_body(&f.block) else {
                self.todo(i, "listeners are closures now: turn this into an async method and register a closure calling it");
                return;
            };
            let inner = ret
                .trim()
                .strip_prefix("BoxFuture<'a,")
                .or_else(|| ret.trim().strip_prefix("BoxFuture<'a ,"))
                .and_then(|r| r.trim().strip_suffix('>'))
                .map(|r| r.trim().to_owned());
            let Some(inner) = inner else { return };
            let ps = if pattern {
                format!(
                    "{}: &{krate}::Ctx, {}: String, {}: &{ev}, {}: {krate}::events::Call<{ev}>",
                    names[0], names[1], names[2], names[3]
                )
            } else {
                format!(
                    "{}: &{krate}::Ctx, {}: &{ev}, {}: {krate}::events::Call<{ev}>",
                    names[0], names[1], names[2]
                )
            };
            (ps, b, inner, "on_event", "async ")
        };
        let body = if pattern {
            body.replace(
                &format!("{}.name()", names[1]),
                &format!("{}.as_str()", names[1]),
            )
        } else {
            body
        };
        let generics = self.text(&i.generics).to_owned();
        let self_ty = self.text(&*i.self_ty).to_owned();
        let where_clause = i
            .generics
            .where_clause
            .as_ref()
            .map(|w| format!(" {}", self.text(w)))
            .unwrap_or_default();
        let text = format!(
            "impl{generics} {self_ty}{where_clause} {{\n    {asyncness}fn {fname}(&self, {params}) -> {ret} {{{body}}}\n}}"
        );
        // keep the impl's attributes: replace from `impl` to the end
        let (start, _) = self.index.range(self.src, i.impl_token.span);
        let (_, end) = self.range(i);
        self.edit_range(start, end, text);
    }

    /// `fn l<'a>(ctx: &'a Ctx, e: &'a E, call: Call<'a, V>) -> BoxFuture<'a, R> {…}` →
    /// `async fn l(ctx: &Ctx, e: &E, call: Call<E>) -> R {…}`.
    fn rewrite_listener_fn(&mut self, f: &syn::ItemFn) {
        if f.sig.inputs.len() != 3 || f.sig.asyncness.is_some() {
            return;
        }
        let tys: Vec<(String, String)> = f
            .sig
            .inputs
            .iter()
            .map(|a| match a {
                syn::FnArg::Typed(t) => {
                    (self.text(&*t.pat).to_owned(), self.text(&*t.ty).to_owned())
                }
                _ => (String::new(), String::new()),
            })
            .collect();
        if !tys[0].1.contains("Ctx")
            || !(tys[2].1.starts_with("Call<") || tys[2].1.starts_with("SyncCall<"))
        {
            return;
        }
        let ev = tys[1]
            .1
            .trim_start_matches('&')
            .trim_start_matches("'a")
            .trim()
            .to_owned();
        let krate = self.krate;
        let vis = match &f.vis {
            syn::Visibility::Inherited => String::new(),
            v => format!("{} ", self.text(v)),
        };
        let name = f.sig.ident.to_string();
        let ret = match &f.sig.output {
            syn::ReturnType::Type(_, t) => self.text(&**t).to_owned(),
            _ => return,
        };
        let text = if tys[2].1.starts_with("SyncCall<") {
            let b = self.text(&*f.block);
            format!(
                "{vis}fn {name}({}: &{krate}::Ctx, {}: &{ev}, {}: {krate}::events::SyncCall<'_, {ev}>) -> {ret} {b}",
                tys[0].0, tys[1].0, tys[2].0
            )
        } else {
            let Some(inner) = ret
                .trim()
                .strip_prefix("BoxFuture<'a,")
                .and_then(|r| r.trim().strip_suffix('>'))
                .map(|r| r.trim().to_owned())
            else {
                return;
            };
            let Some(b) = self.async_fn_body(&f.block) else {
                return;
            };
            format!(
                "{vis}async fn {name}({}: &{krate}::Ctx, {}: &{ev}, {}: {krate}::events::Call<{ev}>) -> {inner} {{{b}}}",
                tys[0].0, tys[1].0, tys[2].0
            )
        };
        let start = match &f.vis {
            syn::Visibility::Inherited => self.index.range(self.src, f.sig.span()).0,
            v => self.range(v).0,
        };
        let (_, end) = self.range(f);
        self.edit_range(start, end, text);
    }

    /// `let key = EventKey::<E>::of();` (or `EventPattern::…`) that nothing
    /// uses any more → removed, line and all.
    fn drop_unused_keys(&mut self, b: &syn::Block) {
        let block_text = self.text(b);
        for stmt in &b.stmts {
            let syn::Stmt::Local(l) = stmt else { continue };
            let name = match &l.pat {
                syn::Pat::Ident(i) => i.ident.to_string(),
                syn::Pat::Type(t) => match &*t.pat {
                    syn::Pat::Ident(i) => i.ident.to_string(),
                    _ => continue,
                },
                _ => continue,
            };
            let Some(init) = &l.init else { continue };
            // only a key built and nothing else (`let key = EventKey::of();`):
            // anything that does more with it stays
            if init.diverge.is_some() || !is_key_constructor(&init.expr) {
                continue;
            }
            let uses = block_text
                .match_indices(name.as_str())
                .filter(|(i, _)| {
                    let before = block_text[..*i].chars().last();
                    let after = block_text[i + name.len()..].chars().next();
                    !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                        && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
                })
                .count();
            if uses == 1 {
                let (start, end) = self.range(stmt);
                // the whole line when the statement is alone on it
                let line_start = self.src[..start].rfind('\n').map(|n| n + 1).unwrap_or(0);
                let line_end = self.src[end..]
                    .find('\n')
                    .map(|n| end + n + 1)
                    .unwrap_or(end);
                if self.src[line_start..start].trim().is_empty()
                    && self.src[end..line_end].trim().is_empty()
                {
                    self.edit_range(line_start, line_end, String::new());
                } else {
                    self.edit_range(start, end, String::new());
                }
            }
        }
    }
}

fn collect_lets(rules: &Rules<'_>, block: &syn::Block) -> Vec<(String, String, Option<String>)> {
    let mut out = Vec::new();
    for stmt in &block.stmts {
        if let syn::Stmt::Local(l) = stmt {
            let (pat, ty) = match &l.pat {
                syn::Pat::Ident(i) => (i.ident.to_string(), None),
                syn::Pat::Type(t) => match &*t.pat {
                    syn::Pat::Ident(i) => {
                        (i.ident.to_string(), Some(rules.text(&*t.ty).to_owned()))
                    }
                    _ => continue,
                },
                _ => continue,
            };
            if let Some(init) = &l.init {
                out.push((pat, rules.text(&*init.expr).to_owned(), ty));
            }
        }
    }
    out
}

// ── 3. imports ───────────────────────────────────────────────────

/// Where each 0.16 root name lives in 0.17.
fn new_home(name: &str) -> Option<&'static str> {
    Some(match name {
        "Ctx" | "Plugin" | "Deps" | "Event" | "Disposer" | "Error" | "Result" | "service"
        | "serde" => "",
        "TypeKey" | "InstanceId" | "Key" | "DynPlugin" | "PluginFactory" | "Effect"
        | "EffectMeta" | "EffectPhase" | "BoxFuture" | "FiberView" | "FiberState"
        | "FiberStatusChanged" | "PluginId" | "Snapshot" | "DisposeWaitError"
        | "shutdown_deadline" | "ServiceReadError" | "ServiceReadFailure" | "NextMisuse"
        | "CallError" | "ErrorSink" | "default_sink" | "EventSubscription" | "ListenerKind"
        | "ErrorKind" | "CtxHost" => "host",
        "BindingDiagnostics"
        | "DependencyDiagnostics"
        | "DependencyStatus"
        | "EventBacklog"
        | "PluginDiagnostics"
        | "ResolvedDependency"
        | "RuntimeDiagnostics"
        | "ServiceAccess"
        | "ServiceChange"
        | "ServiceChanged" => "host::diagnostics",
        "Mode" | "ListenerInfo" | "Scope" | "Call" | "SyncCall" | "Filtered" | "Listen"
        | "Waterfall" | "Listener" => "events",
        "EventBus"
        | "DispatchAttempt"
        | "EventKey"
        | "EventPattern"
        | "EventOptions"
        | "ServiceIntercept"
        | "ServiceWriter"
        | "ServiceWriteError"
        | "ServiceWriteFailure"
        | "CtxInternal"
        | "RESERVED_PREFIX" => "internal",
        _ => return None,
    })
}

/// Names that no longer exist.
fn removed(name: &str) -> Option<&'static str> {
    Some(match name {
        "Typed" | "TypedFactory" | "TypedPluginFactory" => {
            "typed plugins are the one Plugin trait now"
        }
        "Keyed" | "KeyedGate" => {
            "named dependencies: implement rutis::host::DynPlugin and read with require_at"
        }
        "Gate" => "take the service in Deps and leave it unused",
        "SyncListener" | "PatternListener" | "SyncPatternListener" => {
            "listeners are closures: ctx.on / ctx.on_sync / ctx.listen"
        }
        "Terminal" => "ctx.waterfall(event).terminal(|event| async move { … })",
        _ => return None,
    })
}

/// Imported names that 0.17 code often no longer needs once rewritten.
const DROPPABLE: &[&str] = &[
    "EventKey",
    "EventPattern",
    "Listener",
    "PatternListener",
    "SyncListener",
    "Call",
    "SyncCall",
    "BoxFuture",
    "Effect",
];

const INTERNAL_EVENTS: &[&str] = &[
    "BatchFilter",
    "Candidate",
    "Codec",
    "Dispatch",
    "EntryCall",
    "FilterAnswer",
    "JsonTranscoder",
    "Payload",
    "PayloadRef",
    "RawTerminal",
    "RemoteCall",
    "RemoteEntry",
    "RemoteListener",
    "RemoteNext",
    "RemoteSyncCall",
    "RemoteSyncNext",
    "TableChange",
    "TableEntry",
    "Transcoder",
    "Value",
];

const HOST_METHODS: &[&str] = &[
    "root_with",
    "root_with_sink",
    "root_view",
    "root_ctx",
    "shutdown",
    "shutdown_with_timeout",
    "is_shutting_down",
    "error_sink",
    "take_cleanup_errors",
    "diagnostics",
    "isolate",
    "isolate_label",
    "dispose_self",
    "cancellation_token",
    "plugin_with",
    "plugin_from",
    "require",
    "require_at",
    "get_at",
    "provide_at",
    "waterfall_sync",
    "subscriptions",
];

const INTERNAL_METHODS: &[&str] = &[
    "named",
    "events",
    "mark_scope",
    "mark_row",
    "in_instance_key",
    "require_provided_as",
    "provide_checked",
    "provide_mut_at",
    "effect_named",
    "refresh",
    "get_key",
    "require_key",
    "provide_key",
    "effect_raw",
    "try_filter",
    "try_filter_named",
    "intercept_require_as",
    "intercept_set_as",
];

fn rewrite_imports(text: &str, krate: &str, error_name: &str) -> Option<String> {
    let file = syn::parse_file(text).ok()?;
    let index = LineIndex::new(text);
    let mut edits = Vec::new();
    let krate_segs: Vec<&str> = krate.split("::").collect();
    let mut needs: BTreeSet<(String, String)> = BTreeSet::new(); // (home, name)

    // tokens of the whole file: identifiers used, method calls
    let stream: TokenStream = text.parse().ok()?;
    let mut tokens = Vec::new();
    flatten(stream, &mut tokens);
    let mut idents = BTreeSet::new();
    let mut methods = BTreeSet::new();
    for (i, t) in tokens.iter().enumerate() {
        if let TokenTree::Ident(id) = t {
            idents.insert(id.to_string());
            let dotted =
                i > 0 && matches!(&tokens[i - 1], TokenTree::Punct(p) if p.as_char() == '.');
            let called = matches!(tokens.get(i + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis)
                || matches!(tokens.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == ':');
            if dotted && called {
                methods.insert(id.to_string());
            }
        }
    }
    let ctx_root = text.contains("Ctx::root(");
    let uses_host_methods = ctx_root || HOST_METHODS.iter().any(|m| methods.contains(*m));
    let uses_internal_methods = INTERNAL_METHODS.iter().any(|m| methods.contains(*m));

    if idents.contains("ErrorKind") && !text.contains("::ErrorKind") {
        needs.insert(("host".to_owned(), "ErrorKind".to_owned()));
    }
    if idents.contains("Key")
        && !text.contains("host::Key")
        && !import_lists_name(text, "Key")
        && !has_own_key(text)
    {
        needs.insert(("host".to_owned(), "Key".to_owned()));
    }
    if uses_host_methods && !text.contains("CtxHost") {
        needs.insert(("host".to_owned(), "CtxHost".to_owned()));
    }
    if uses_internal_methods && !text.contains("CtxInternal") {
        needs.insert(("internal".to_owned(), "CtxInternal".to_owned()));
    }
    // Code outside inline modules that needs the imports while no `use` of
    // rutis is there (`rutis::Ctx::root()` written in full): they go to the
    // top of the file, where `use super::*` in the modules sees them too.
    let top_level_uses_rutis = file.items.iter().any(|item| {
        matches!(item, syn::Item::Use(u) if matches!(&u.tree, syn::UseTree::Path(p) if krate_segs.first().is_some_and(|k| p.ident == *k)))
    });
    let top_level_needs = !top_level_uses_rutis && {
        let outside: String = file
            .items
            .iter()
            .filter(|item| !matches!(item, syn::Item::Mod(m) if m.content.is_some()))
            .map(|item| {
                let (s, e) = index.range(text, item.span());
                &text[s..e]
            })
            .collect();
        outside.contains("Ctx::root(")
            || HOST_METHODS
                .iter()
                .chain(INTERNAL_METHODS)
                .any(|m| outside.contains(&format!(".{m}(")))
    };
    // identifiers used outside `use` items
    let mut used_outside_use = BTreeSet::new();
    for item in &file.items {
        if matches!(item, syn::Item::Use(_)) {
            continue;
        }
        let (s, e) = index.range(text, item.span());
        let mut item_tokens = Vec::new();
        if let Ok(stream) = text[s..e].parse::<TokenStream>() {
            flatten(stream, &mut item_tokens);
        }
        for t in &item_tokens {
            if let TokenTree::Ident(id) = t {
                used_outside_use.insert(id.to_string());
            }
        }
    }
    let mut needs_placed = top_level_needs;
    let mut receiver: Option<usize> = None;
    for u in all_uses(&file.items) {
        let mut names = Vec::new(); // (name, rename)
        let mut other = Vec::new(); // unrelated subtrees kept as they are
        if !collect_use(
            &u.tree,
            &krate_segs,
            &mut Vec::new(),
            &mut names,
            &mut other,
        ) {
            continue;
        }
        let mut root: Vec<String> = Vec::new();
        let mut moved: BTreeSet<(String, String)> = BTreeSet::new();
        for (path, name, rename) in names {
            let full = match &rename {
                Some(r) => format!("{name} as {r}"),
                None => name.clone(),
            };
            // names the rewrites above made unused (a key whose dispatch
            // became a `Ctx` method, a listener trait now a closure) go
            if rename.is_none()
                && DROPPABLE.contains(&name.as_str())
                && !used_outside_use.contains(&name)
            {
                continue;
            }
            // events::{X} of 0.16 (remote/wire types) → internal::events
            if path.first().map(String::as_str) == Some("events") && path.len() == 1 {
                if INTERNAL_EVENTS.contains(&name.as_str()) {
                    moved.insert(("internal::events".to_owned(), full));
                } else {
                    match new_home(&name) {
                        Some("") => root.push(full),
                        Some(home) => {
                            moved.insert((home.to_owned(), full));
                        }
                        None => {
                            moved.insert(("events".to_owned(), full));
                        }
                    }
                }
                continue;
            }
            // `rutis::service::…` (the module) is `rutis::internal::service::…`
            if path.first().map(String::as_str) == Some("service") {
                let rest = path[1..]
                    .iter()
                    .map(|s| format!("{s}::"))
                    .collect::<String>();
                let home = format!(
                    "internal::service{}",
                    if rest.is_empty() {
                        String::new()
                    } else {
                        format!("::{}", rest.trim_end_matches("::"))
                    }
                );
                moved.insert((home, full));
                continue;
            }
            if !path.is_empty() {
                // a deeper path (rutis::service::…) is kept
                other.push(format!("{}::{}", path.join("::"), full));
                continue;
            }
            if (name == "Error" || name == error_name) && error_name != "Error" && rename.is_none()
            {
                root.push(format!("Error as {error_name}"));
                continue;
            }
            if let Some(why) = removed(&name) {
                let (s, _) = index.range(text, u.span());
                if let Some(edit) =
                    todo_edit(text, &index, s, &format!("{name} was removed: {why}"))
                {
                    edits.push(edit);
                }
                continue;
            }
            match new_home(&name) {
                Some("") => root.push(full),
                Some(home) => {
                    moved.insert((home.to_owned(), full));
                }
                None => root.push(full),
            }
        }
        let vis = match &u.vis {
            syn::Visibility::Inherited => String::new(),
            v => {
                let (s, e) = index.range(text, v.span());
                format!("{} ", &text[s..e])
            }
        };
        // `use rutis::*;` had what moved: the layers it moved to come along
        if other.iter().any(|o| o == "*") {
            moved.insert(("host".to_owned(), "*".to_owned()));
            let events = [
                "Call",
                "SyncCall",
                "Mode",
                "ListenerInfo",
                "Scope",
                "Listener",
                "Filtered",
            ];
            if events.iter().any(|e| idents.contains(*e)) {
                moved.insert(("events".to_owned(), "*".to_owned()));
            }
        }
        let mut lines = Vec::new();
        let mut root_all = root.clone();
        root_all.extend(other.iter().cloned());
        if !root_all.is_empty() {
            lines.push(format_use(&vis, krate, &root_all));
        }
        if !needs_placed && matches!(u.vis, syn::Visibility::Inherited) {
            for need in std::mem::take(&mut needs) {
                moved.insert(need);
            }
            needs_placed = true;
            receiver = Some(index.range(text, u.span()).0);
        }
        let mut by_home: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for (home, name) in moved {
            let list = by_home.entry(home).or_default();
            if !list.contains(&name) {
                list.push(name);
            }
        }
        for (home, list) in &by_home {
            lines.push(format_use(&vis, &format!("{krate}::{home}"), list));
        }
        // from the visibility (or `use`) to the `;`: attributes stay
        let s = match &u.vis {
            syn::Visibility::Inherited => index.range(text, u.use_token.span).0,
            v => index.range(text, v.span()).0,
        };
        let (_, e) = index.range(text, u.semi_token.span);
        let indent = text[..s].rsplit('\n').next().unwrap_or("").to_owned();
        let indent = if indent.trim().is_empty() {
            indent
        } else {
            String::new()
        };
        edits.push(Edit {
            start: s,
            end: e,
            text: lines.join(&format!("\n{indent}")),
        });
    }
    // An inline module without `use super::*` whose own code calls host or
    // internal methods gets the trait imports itself (`mod tests` beside a
    // `main` that wrote `rutis::Ctx::root()` in full).
    for module in inline_modules(&file.items) {
        let Some((brace, items)) = &module.content else {
            continue;
        };
        let (start, end) = index.range(text, module.span());
        if receiver.is_some_and(|at| at >= start && at < end) {
            continue;
        }
        let body: String = items
            .iter()
            .filter(|item| !matches!(item, syn::Item::Mod(m) if m.content.is_some()))
            .map(|item| {
                let (s, e) = index.range(text, item.span());
                &text[s..e]
            })
            .collect();
        if body.contains("use super::*") {
            continue;
        }
        let mut lines = Vec::new();
        let host = body.contains("Ctx::root(")
            || HOST_METHODS
                .iter()
                .any(|m| body.contains(&format!(".{m}(")));
        if host && !body.contains("CtxHost") {
            lines.push(format!("use {krate}::host::CtxHost;"));
        }
        if INTERNAL_METHODS
            .iter()
            .any(|m| body.contains(&format!(".{m}(")))
            && !body.contains("CtxInternal")
        {
            lines.push(format!("use {krate}::internal::CtxInternal;"));
        }
        if lines.is_empty() {
            continue;
        }
        let (open, _) = index.range(text, brace.span.open());
        let at = open + 1;
        let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
        let indent: String = text[line_start..start]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let inner = format!("{indent}    ");
        edits.push(Edit {
            start: at,
            end: at,
            text: format!("\n{inner}{}", lines.join(&format!("\n{inner}"))),
        });
    }
    // trait imports and ErrorKind not placed in a rutis `use`
    let mut extra = Vec::new();
    for (home, name) in needs {
        let line = format!("use {krate}::{home}::{name};");
        if !text.contains(&line) {
            extra.push(line);
        }
    }
    // qualified paths in code: rutis::X → rutis::host::X
    for (i, t) in tokens.iter().enumerate() {
        let TokenTree::Ident(id) = t else { continue };
        if id != krate_segs.last().copied().unwrap_or("rutis") {
            continue;
        }
        // rutis :: NAME
        let (Some(TokenTree::Punct(c1)), Some(TokenTree::Punct(c2)), Some(TokenTree::Ident(name))) =
            (tokens.get(i + 1), tokens.get(i + 2), tokens.get(i + 3))
        else {
            continue;
        };
        if c1.as_char() != ':' || c2.as_char() != ':' {
            continue;
        }
        let n = name.to_string();
        let (s, e) = index.range(text, name.span());
        if inside_use(&file, s, text, &index) {
            continue;
        }
        if n == "events" {
            // rutis::events::Dispatch → rutis::internal::events::Dispatch
            if let Some(TokenTree::Ident(inner)) = tokens.get(i + 6) {
                let inner = inner.to_string();
                if INTERNAL_EVENTS.contains(&inner.as_str()) {
                    edits.push(Edit {
                        start: s,
                        end: e,
                        text: "internal::events".to_owned(),
                    });
                } else if let Some(home @ ("host" | "internal")) = new_home(&inner) {
                    edits.push(Edit {
                        start: s,
                        end: e,
                        text: home.to_owned(),
                    });
                }
            }
            continue;
        }
        if n == "service" {
            // rutis::service::Entry → rutis::internal::service::Entry; the
            // attribute `#[rutis::service]` stays
            let module =
                matches!(tokens.get(i + 4), Some(TokenTree::Punct(p)) if p.as_char() == ':');
            if module {
                edits.push(Edit {
                    start: s,
                    end: e,
                    text: "internal::service".to_owned(),
                });
            }
            continue;
        }
        if let Some(home) = new_home(&n) {
            if !home.is_empty() {
                edits.push(Edit {
                    start: s,
                    end: e,
                    text: format!("{home}::{n}"),
                });
            }
        }
    }
    let mut out = apply(text, edits);
    if !extra.is_empty() {
        // after the last top-level `use` (or at the top, after inner attributes)
        let file = syn::parse_file(&out).ok();
        let index = LineIndex::new(&out);
        let at = file
            .as_ref()
            .and_then(|f| {
                f.items
                    .iter()
                    .rfind(|i| matches!(i, syn::Item::Use(_)))
                    .map(|u| {
                        let (_, e) = index.range(&out, u.span());
                        let e = if out[e..].starts_with(';') { e + 1 } else { e };
                        out[e..].find('\n').map(|n| e + n + 1).unwrap_or(out.len())
                    })
            })
            .unwrap_or_else(|| first_item_offset(&out));
        let insert = extra.join("\n") + "\n";
        out.insert_str(at, &insert);
    }
    (out != text).then_some(out)
}

/// `ErrorKind::X` naming rutis's kind (`ErrorKind::X`,
/// `rutis::host::ErrorKind::X`), not `std::io::ErrorKind::X`.
fn is_rutis_kind_path(segs: &[String]) -> bool {
    let n = segs.len();
    if n < 2 || segs[n - 2] != "ErrorKind" {
        return false;
    }
    let prefix: Vec<&str> = segs[..n - 2].iter().map(String::as_str).collect();
    matches!(
        prefix.as_slice(),
        [] | ["host"]
            | ["rutis", "host"]
            | ["rutis_sdk", "rutis", "host"]
            | ["crate"]
            | ["crate", "error"]
            | ["rutis"]
    )
}

/// `EventKey::of()`, `EventKey::<E>::dynamic(name)`, `EventPattern::prefix(p)`
/// and the like, possibly borrowed or cloned: a key and nothing more.
fn is_key_constructor(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Reference(r) => is_key_constructor(&r.expr),
        syn::Expr::Paren(p) => is_key_constructor(&p.expr),
        syn::Expr::Call(call) => {
            let syn::Expr::Path(path) = &*call.func else {
                return false;
            };
            let segs: Vec<String> = path
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            let n = segs.len();
            n >= 2
                && matches!(segs[n - 2].as_str(), "EventKey" | "EventPattern")
                && matches!(
                    segs[n - 1].as_str(),
                    "of" | "dynamic" | "named" | "default" | "prefix"
                )
        }
        _ => false,
    }
}

/// An expression with outer attributes (`#[cfg(..)] expr`).
fn expr_has_attrs(expr: &syn::Expr) -> bool {
    let tokens = quote_tokens(expr);
    tokens.trim_start().starts_with('#')
}

fn quote_tokens(expr: &syn::Expr) -> String {
    use quote_like::ToTokensString;
    expr.to_tokens_string()
}

mod quote_like {
    pub trait ToTokensString {
        fn to_tokens_string(&self) -> String;
    }
    impl<T: syn::spanned::Spanned> ToTokensString for T {
        fn to_tokens_string(&self) -> String {
            let span = self.span();
            span.source_text().unwrap_or_default()
        }
    }
}

/// The inline modules of the file, nested ones included.
fn inline_modules(items: &[syn::Item]) -> Vec<&syn::ItemMod> {
    let mut out = Vec::new();
    for item in items {
        if let syn::Item::Mod(m) = item {
            if let Some((_, inner)) = &m.content {
                out.push(m);
                out.extend(inline_modules(inner));
            }
        }
    }
    out
}

/// The `use` items of the file and of its inline modules, in order.
fn all_uses(items: &[syn::Item]) -> Vec<&syn::ItemUse> {
    let mut out = Vec::new();
    for item in items {
        match item {
            syn::Item::Use(u) => out.push(u),
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    out.extend(all_uses(inner));
                }
            }
            _ => {}
        }
    }
    out
}

/// The module of rutis-loader a name moved to from its root (0.17); the
/// eleven names that stay at the root are not listed.
fn loader_home(name: &str) -> Option<&'static str> {
    Some(match name {
        "CheckReport" | "ReconcileReport" | "RowInfo" | "RowStatus" | "RowCheck" | "Isolate"
        | "EntryInfo" | "EntryStatus" | "Failure" | "Unknown" | "Unavailable" | "Supply"
        | "Provider" | "Requirement" | "InstanceInfo" | "LoaderChanged" | "PendingEditDropped"
        | "RowState" => "report",
        "Edit" | "Editable" | "NewEntry" | "CreateInstance" | "Instance" | "InstanceResult"
        | "Persist" | "NoPersist" | "PersistError" | "Version" | "apply_edit"
        | "VolatileUpdate" | "volatile_key" | "volatile_paths" => "edit",
        "Build" | "Resolved" | "Declarations" | "Declared" | "ScopedFactory" | "Expressions"
        | "ExprScope" => "resolve",
        "RuntimeResolver" | "RuntimeRowsPlugin" | "RuntimeRows" | "RowsSource"
        | "RuntimeConnection" | "RuntimeInstances" | "AcceptInstances" => "runtime",
        "Binaries"
        | "BinaryFile"
        | "BinaryLanguage"
        | "BinaryManifest"
        | "BinaryResolver"
        | "BinaryRuntimeInfo"
        | "BinaryRuntimeState"
        | "BinaryRuntimes"
        | "BinaryRuntimesHandle"
        | "RestartPolicy"
        | "RuntimeEnd" => "runtime::binary",
        "Go" | "GoBinaries" | "GoBinary" | "GoManifest" | "GoResolver" | "GoRuntimeInfo"
        | "GoRuntimeState" | "GoRuntimes" | "GoRuntimesHandle" => "runtime::go",
        "Swift"
        | "SwiftBinaries"
        | "SwiftBinary"
        | "SwiftManifest"
        | "SwiftResolver"
        | "SwiftRuntimeInfo"
        | "SwiftRuntimeState"
        | "SwiftRuntimes"
        | "SwiftRuntimesHandle" => "runtime::swift",
        "PeerResolver" | "register_peer_node" | "PeerRows" | "PeerRowsPlugin" | "LoaderCatalog" => {
            "peer"
        }
        "apply_patches"
        | "Composed"
        | "ComposedRow"
        | "Owner"
        | "PatchWarning"
        | "is_runtime_name"
        | "resolve_entry"
        | "PLUGIN_API"
        | "SWIFT_PLUGIN_API"
        | "GO_MARKER"
        | "SWIFT_MARKER"
        | "read_binary_manifest"
        | "read_go_manifest"
        | "read_swift_manifest"
        | "node_schema"
        | "REMOVED_EVENTS" => "internal",
        _ => return None,
    })
}

/// `use rutis_loader::{…}` and `rutis_loader::X` in code: the names that
/// left the root move to their module.
fn rewrite_loader_paths(text: &str) -> Option<String> {
    if !text.contains("rutis_loader") {
        return None;
    }
    let file = syn::parse_file(text).ok()?;
    let index = LineIndex::new(text);
    let mut edits = Vec::new();
    for u in all_uses(&file.items) {
        let syn::UseTree::Path(p) = &u.tree else {
            continue;
        };
        if p.ident != "rutis_loader" {
            continue;
        }
        let mut names = Vec::new();
        let mut other = Vec::new();
        let mut collected = true;
        match &*p.tree {
            syn::UseTree::Name(n) => names.push((n.ident.to_string(), None)),
            syn::UseTree::Rename(r) => {
                names.push((r.ident.to_string(), Some(r.rename.to_string())))
            }
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    match t {
                        syn::UseTree::Name(n) => names.push((n.ident.to_string(), None)),
                        syn::UseTree::Rename(r) => {
                            names.push((r.ident.to_string(), Some(r.rename.to_string())))
                        }
                        t => other.push(quote_use(t)),
                    }
                }
            }
            _ => collected = false,
        }
        if !collected || !names.iter().any(|(n, _)| loader_home(n).is_some()) {
            continue;
        }
        let mut root = Vec::new();
        let mut by_home: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
        for (name, rename) in names {
            let full = match rename {
                Some(r) => format!("{name} as {r}"),
                None => name.clone(),
            };
            match loader_home(&name) {
                Some(home) => by_home.entry(home).or_default().push(full),
                None => root.push(full),
            }
        }
        root.extend(other.into_iter().map(|o| o.replace(',', ", ")));
        let vis = match &u.vis {
            syn::Visibility::Inherited => String::new(),
            v => {
                let (s, e) = index.range(text, v.span());
                format!("{} ", &text[s..e])
            }
        };
        let mut lines = Vec::new();
        if !root.is_empty() {
            lines.push(format_use(&vis, "rutis_loader", &root));
        }
        for (home, list) in &by_home {
            lines.push(format_use(&vis, &format!("rutis_loader::{home}"), list));
        }
        let s = match &u.vis {
            syn::Visibility::Inherited => index.range(text, u.use_token.span).0,
            v => index.range(text, v.span()).0,
        };
        let (_, e) = index.range(text, u.semi_token.span);
        edits.push(Edit {
            start: s,
            end: e,
            text: lines.join("\n"),
        });
    }
    // qualified paths in code
    let stream: TokenStream = text.parse().ok()?;
    let mut tokens = Vec::new();
    flatten(stream, &mut tokens);
    for (i, t) in tokens.iter().enumerate() {
        let TokenTree::Ident(id) = t else { continue };
        if id != "rutis_loader" {
            continue;
        }
        let (Some(TokenTree::Punct(c1)), Some(TokenTree::Punct(c2)), Some(TokenTree::Ident(name))) =
            (tokens.get(i + 1), tokens.get(i + 2), tokens.get(i + 3))
        else {
            continue;
        };
        if c1.as_char() != ':' || c2.as_char() != ':' {
            continue;
        }
        let (s, e) = index.range(text, name.span());
        if inside_use(&file, s, text, &index) {
            continue;
        }
        if let Some(home) = loader_home(&name.to_string()) {
            edits.push(Edit {
                start: s,
                end: e,
                text: format!("{home}::{name}"),
            });
        }
    }
    let out = apply(text, edits);
    (out != text).then_some(out)
}

fn first_item_offset(text: &str) -> usize {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let l = line.trim_start();
        if l.starts_with("//!") || l.starts_with("#![") || l.is_empty() {
            offset += line.len();
            continue;
        }
        break;
    }
    offset
}

fn inside_use(file: &syn::File, offset: usize, text: &str, index: &LineIndex) -> bool {
    all_uses(&file.items).iter().any(|u| {
        let (s, e) = index.range(text, u.span());
        offset >= s && offset <= e + 1
    })
}

fn format_use(vis: &str, path: &str, names: &[String]) -> String {
    if names.len() == 1 && !names[0].contains("::") {
        format!("{vis}use {path}::{};", names[0])
    } else {
        format!("{vis}use {path}::{{{}}};", names.join(", "))
    }
}

/// Collects the names a `use` brings in from the rutis crate (`krate`).
/// Returns false when the tree is not about rutis.
#[allow(clippy::too_many_arguments)]
fn collect_use(
    tree: &syn::UseTree,
    krate: &[&str],
    prefix: &mut Vec<String>,
    names: &mut Vec<(Vec<String>, String, Option<String>)>,
    other: &mut Vec<String>,
) -> bool {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            let ok = collect_use(&p.tree, krate, prefix, names, other);
            prefix.pop();
            ok
        }
        syn::UseTree::Group(g) => {
            let mut any = false;
            for t in &g.items {
                any |= collect_use(t, krate, prefix, names, other);
            }
            any
        }
        syn::UseTree::Name(n) => {
            if !prefix.starts_with(&krate.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                || prefix.len() < krate.len()
            {
                return false;
            }
            let rest = prefix[krate.len()..].to_vec();
            names.push((rest, n.ident.to_string(), None));
            true
        }
        syn::UseTree::Rename(r) => {
            if !prefix.starts_with(&krate.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                || prefix.len() < krate.len()
            {
                return false;
            }
            let rest = prefix[krate.len()..].to_vec();
            names.push((rest, r.ident.to_string(), Some(r.rename.to_string())));
            true
        }
        syn::UseTree::Glob(_) => {
            if prefix.iter().map(String::as_str).eq(krate.iter().copied()) {
                other.push("*".to_owned());
                return true;
            }
            false
        }
    }
}

// ── 4. TODO comments ─────────────────────────────────────────────

/// An edit inserting `// TODO(rutis 0.17): message` above the line of
/// `offset`, unless that comment is already there.
fn todo_edit(text: &str, index: &LineIndex, offset: usize, message: &str) -> Option<Edit> {
    let line = index.line_of(offset);
    let start = index.starts[line];
    let indent: String = text[start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let comment = format!("{indent}// TODO(rutis 0.17): {message}");
    if line > 0 {
        let prev_start = index.starts[line - 1];
        let prev = &text[prev_start..start];
        if prev.trim() == comment.trim() {
            return None;
        }
    }
    Some(Edit {
        start,
        end: start,
        text: format!("{comment}\n"),
    })
}

fn todos_in(text: &str) -> Vec<Todo> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            l.trim()
                .strip_prefix("// TODO(rutis 0.17): ")
                .map(|m| Todo {
                    line: i + 1,
                    message: m.to_owned(),
                })
        })
        .collect()
}

/// A unified diff of `old` and `new`, three lines of context.
pub fn diff(path: &str, old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    // each op: (tag, line text, old line number, new line number)
    let mut ops: Vec<(char, &str, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', a[i], i, j));
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push(('+', b[j], i, j));
            j += 1;
        } else {
            ops.push(('-', a[i], i, j));
            i += 1;
        }
    }
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| o.0 != ' ')
        .map(|(k, _)| k)
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut out = format!("--- {path}\n+++ {path}\n");
    let mut k = 0;
    while k < changed.len() {
        let first = changed[k];
        let mut last = first;
        while k + 1 < changed.len() && changed[k + 1] <= last + 7 {
            k += 1;
            last = changed[k];
        }
        k += 1;
        let start = first.saturating_sub(3);
        let end = (last + 4).min(ops.len());
        let hunk = &ops[start..end];
        let old_start = hunk.first().map(|o| o.2).unwrap_or(0);
        let new_start = hunk.first().map(|o| o.3).unwrap_or(0);
        let old_len = hunk.iter().filter(|o| o.0 != '+').count();
        let new_len = hunk.iter().filter(|o| o.0 != '-').count();
        out.push_str(&format!(
            "@@ -{},{old_len} +{},{new_len} @@\n",
            old_start + 1,
            new_start + 1
        ));
        for (tag, line, _, _) in hunk {
            out.push(*tag);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}
