//! The composed tree as rows the loader can act on.

use std::collections::{BTreeMap, HashMap};

use rutis::{Ctx, TypeKey};
use serde_json::Value;

use crate::catalog::{ExprScope, Expressions, NameKey, ServiceCatalog};
use crate::patch::{truthy, Composed, Owner, PatchWarning};
use crate::resolver::Build;
use crate::LoaderError;

#[derive(Default)]
pub(super) struct Desired {
    pub(super) rows: Vec<Row>,
    pub(super) by_id: HashMap<String, usize>,
    pub(super) warnings: Vec<PatchWarning>,
    pub(super) issues: Vec<String>,
}

pub(super) struct Row {
    pub(super) id: String,
    pub(super) parent: Option<String>,
    pub(super) value: Value,
    pub(super) name: Option<String>,
    pub(super) group: bool,
    /// An instanced group: not spawned by reconcile, only as instances.
    pub(super) instanced: bool,
    pub(super) owner: Owner,
    pub(super) overridden: BTreeMap<String, usize>,
    /// Evaluated with the loader's root context.
    pub(super) disabled: Result<bool, LoaderError>,
    /// Raw; evaluated per spawn or update in the row's own context.
    pub(super) config: Value,
    pub(super) raw_scope: RawScope,
    /// The scope resolved through the catalog. Rows whose resolver handles
    /// scope itself (`Resolved::foreign_scope`) do without it.
    pub(super) scope: Result<RowScope, LoaderError>,
    pub(super) invalid: Option<LoaderError>,
}

/// The row's `isolate` and `inject`, resolved through the catalog. Names
/// inside instances get their keys per copy ([`RowScope::bind`]).
#[derive(Clone, Default)]
pub(super) struct RowScope {
    /// (service name, key, label), sorted by name.
    pub(super) isolate: Vec<(String, NameKey, String)>,
    /// (service name, key), sorted by name.
    pub(super) inject: Vec<(String, NameKey)>,
}

impl RowScope {
    /// The keys for a copy running in `build`'s instances.
    pub(super) fn bind(&self, build: &Build) -> Result<BoundScope, LoaderError> {
        Ok(BoundScope {
            isolate: self
                .isolate
                .iter()
                .map(|(name, key, label)| Ok((name.clone(), key.key(name, build)?, label.clone())))
                .collect::<Result<_, LoaderError>>()?,
            inject: self
                .inject
                .iter()
                .map(|(name, key)| Ok((name.clone(), key.key(name, build)?)))
                .collect::<Result<_, LoaderError>>()?,
        })
    }

    /// The instanced groups whose services the row names.
    fn groups(&self) -> impl Iterator<Item = (&str, &str)> {
        self.isolate
            .iter()
            .map(|(name, key, _)| (name, key))
            .chain(self.inject.iter().map(|(name, key)| (name, key)))
            .filter_map(|(name, key)| key.group().map(|group| (name.as_str(), group)))
    }
}

/// A copy's `isolate` and `inject` keys.
#[derive(Clone, Default)]
pub(super) struct BoundScope {
    /// (service name, key, label), sorted by name.
    pub(super) isolate: Vec<(String, TypeKey, String)>,
    /// (service name, key), sorted by name.
    pub(super) inject: Vec<(String, TypeKey)>,
}

/// The scope label a copy uses: inside an instance (`copy` is its number),
/// a label is the instance's own, so a private scope is one per copy and a
/// named one is shared only within the instance.
pub(super) fn copy_label(label: &str, copy: Option<u64>) -> String {
    match copy {
        None => label.to_owned(),
        Some(number) => format!("{label}@{number}"),
    }
}

impl BoundScope {
    /// `parent` with every isolate applied, for the copy in instance `copy`.
    pub(super) fn context(&self, parent: &Ctx, copy: Option<u64>) -> Ctx {
        self.isolate
            .iter()
            .fold(parent.clone(), |ctx, (_, key, label)| {
                ctx.isolate(key.clone(), &copy_label(label, copy))
            })
    }

    pub(super) fn inject_keys(&self) -> impl Iterator<Item = &TypeKey> {
        self.inject.iter().map(|(_, key)| key)
    }
}

pub(super) fn is_expression(value: &Value) -> bool {
    matches!(value, Value::Object(map) if map.len() == 1 && map.get("__jsExpr").is_some_and(Value::is_string))
}

pub(super) fn contains_expression(value: &Value) -> bool {
    match value {
        _ if is_expression(value) => true,
        Value::Array(items) => items.iter().any(contains_expression),
        Value::Object(map) => map.values().any(contains_expression),
        _ => false,
    }
}

/// Evaluates expression nodes in a raw value.
pub(super) struct Eval<'a> {
    pub(super) expressions: Option<&'a dyn Expressions>,
    pub(super) catalog: &'a ServiceCatalog,
}

impl Eval<'_> {
    /// `raw` with every expression node replaced by its value, outside
    /// instances.
    pub(super) fn value(&self, raw: &Value, ctx: Option<&Ctx>) -> Result<Value, LoaderError> {
        self.value_in(raw, ctx, &Build::default())
    }

    /// [`Eval::value`] for a copy running in `build`'s instances.
    pub(super) fn value_in(
        &self,
        raw: &Value,
        ctx: Option<&Ctx>,
        build: &Build,
    ) -> Result<Value, LoaderError> {
        if !contains_expression(raw) {
            return Ok(raw.clone());
        }
        let Some(expressions) = self.expressions else {
            return Err(LoaderError::Expression(
                "no expression evaluator is installed".into(),
            ));
        };
        let scope = ExprScope::in_instances(ctx, self.catalog, build);
        interpolate(raw, &|source| expressions.evaluate(source, &scope))
    }
}

fn interpolate(
    value: &Value,
    evaluate: &dyn Fn(&str) -> Result<Value, LoaderError>,
) -> Result<Value, LoaderError> {
    if is_expression(value) {
        return evaluate(value["__jsExpr"].as_str().unwrap_or_default());
    }
    Ok(match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| interpolate(item, evaluate))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), interpolate(v, evaluate)?)))
                .collect::<Result<_, LoaderError>>()?,
        ),
        other => other.clone(),
    })
}

/// A row's `isolate` and `inject` as written: service names, before the
/// catalog maps them to keys.
#[derive(Clone, Default, PartialEq)]
pub(super) struct RawScope {
    /// (service name, label), sorted by name.
    pub(super) isolate: Vec<(String, String)>,
    /// Service names, sorted.
    pub(super) inject: Vec<String>,
}

impl RawScope {
    /// What identifies the scope: a change means respawning.
    pub(super) fn signature(&self) -> (Vec<(String, String)>, Vec<String>) {
        (self.isolate.clone(), self.inject.clone())
    }

    /// Map the names to keys through the catalog.
    pub(super) fn resolve(&self, catalog: &ServiceCatalog) -> Result<RowScope, LoaderError> {
        let keys = catalog.name_keys(
            self.isolate
                .iter()
                .map(|(name, _)| name.as_str())
                .chain(self.inject.iter().map(String::as_str)),
        )?;
        let (isolate_keys, inject_keys) = keys.split_at(self.isolate.len());
        Ok(RowScope {
            isolate: self
                .isolate
                .iter()
                .cloned()
                .zip(isolate_keys)
                .map(|((name, label), (_, key))| (name, key.clone(), label))
                .collect(),
            inject: inject_keys.to_vec(),
        })
    }
}

fn parse_scope(id: &str, value: &Value) -> Result<RawScope, LoaderError> {
    let mut isolate_names = Vec::new();
    match value.get("isolate") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (name, spec) in map {
                let label = match spec {
                    Value::Bool(true) => format!("rutis-loader/entry/{id}"),
                    Value::String(label) => format!("rutis-loader/shared/{label}"),
                    Value::Bool(false) | Value::Null => continue,
                    other => {
                        return Err(LoaderError::InvalidEntry(format!(
                            "isolate.{name} of {id:?} must be true or a label, not {other}"
                        )))
                    }
                };
                isolate_names.push((name.clone(), label));
            }
        }
        Some(other) => {
            return Err(LoaderError::InvalidEntry(format!(
                "isolate of {id:?} must be an object, not {other}"
            )))
        }
    }
    let inject_names: Vec<String> = match value.get("inject") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str().map(str::to_owned).ok_or_else(|| {
                    LoaderError::InvalidEntry(format!("inject of {id:?} must list service names"))
                })
            })
            .collect::<Result<_, _>>()?,
        Some(Value::Object(map)) => {
            // cordis's object form maps a name to intercept config, which
            // rutis does not have; only a bare declaration is accepted.
            for (name, config) in map {
                let bare = matches!(config, Value::Null | Value::Bool(true))
                    || config.as_object().is_some_and(|c| c.is_empty());
                if !bare {
                    return Err(LoaderError::Unsupported(format!(
                        "intercept config for {name:?} in inject of {id:?}"
                    )));
                }
            }
            map.keys().cloned().collect()
        }
        Some(other) => {
            return Err(LoaderError::InvalidEntry(format!(
                "inject of {id:?} must be a list or an object, not {other}"
            )))
        }
    };
    isolate_names.sort();
    let mut inject_names = inject_names;
    inject_names.sort();
    inject_names.dedup();
    Ok(RawScope {
        isolate: isolate_names,
        inject: inject_names,
    })
}

impl Desired {
    /// Read the composed rows. `disabled` expressions are evaluated with
    /// `root`; config expressions are left for spawn time.
    pub(super) fn from_composed(composed: Composed, eval: &Eval<'_>, root: Option<&Ctx>) -> Self {
        let mut desired = Desired {
            warnings: composed.warnings,
            ..Desired::default()
        };
        for flat in composed.flat {
            let Some(id) = flat.id.clone() else {
                desired
                    .issues
                    .push(format!("row without an id skipped: {}", flat.value));
                continue;
            };
            if desired.by_id.contains_key(&id) {
                desired
                    .issues
                    .push(format!("duplicate id {id:?}: the later row is skipped"));
                continue;
            }
            let value = flat.value;
            let group = value.get("group").is_some_and(truthy);
            let instanced = value.get("instanced").is_some_and(truthy);
            let name = value.get("name").and_then(Value::as_str).map(str::to_owned);
            let disabled = match value.get("disabled") {
                Some(d) => eval.value(d, root).map(|d| truthy(&d)),
                None => Ok(false),
            };
            let config = if group {
                Value::Null
            } else {
                value.get("config").cloned().unwrap_or(Value::Null)
            };
            let (raw_scope, invalid) = if !group && name.is_none() {
                (
                    RawScope::default(),
                    Some(LoaderError::InvalidEntry(format!("{id:?} has no name"))),
                )
            } else if instanced && !group {
                (
                    RawScope::default(),
                    Some(LoaderError::InvalidEntry(format!(
                        "{id:?} is instanced but not a group"
                    ))),
                )
            } else {
                match parse_scope(&id, &value) {
                    Ok(raw) => (raw, None),
                    Err(error) => (RawScope::default(), Some(error)),
                }
            };
            let scope = raw_scope.resolve(eval.catalog);
            desired.by_id.insert(id.clone(), desired.rows.len());
            desired.rows.push(Row {
                id,
                parent: flat.parent,
                value,
                name,
                group,
                instanced,
                owner: flat.owner,
                overridden: flat.overridden,
                disabled,
                config,
                raw_scope,
                scope,
                invalid,
            });
        }
        desired.check_instance_names();
        desired
    }

    /// A row naming a service inside instances of a group must be in that
    /// group (or be it): elsewhere the name has no key.
    fn check_instance_names(&mut self) {
        let mut misplaced = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            let Ok(scope) = &row.scope else {
                continue;
            };
            let enclosing = self.enclosing_instances(row);
            if let Some((name, group)) = scope
                .groups()
                .find(|(_, group)| !enclosing.iter().any(|g| g == group))
            {
                misplaced.push((
                    index,
                    LoaderError::OutsideInstance {
                        name: name.to_owned(),
                        group: group.to_owned(),
                    },
                ));
            }
        }
        for (index, error) in misplaced {
            self.rows[index].scope = Err(error);
        }
    }

    /// The instanced groups enclosing `row`, itself included.
    fn enclosing_instances(&self, row: &Row) -> Vec<String> {
        let mut groups = Vec::new();
        let mut current = Some(row);
        while let Some(row) = current {
            if row.instanced {
                groups.push(row.id.clone());
            }
            current = row.parent.as_deref().and_then(|id| self.row(id));
        }
        groups
    }

    pub(super) fn row(&self, id: &str) -> Option<&Row> {
        self.by_id.get(id).map(|&i| &self.rows[i])
    }

    /// The nearest instanced group enclosing `row`, or `row` itself when it
    /// is one: its copies run once per instance of that group.
    pub(super) fn instanced_group<'a>(&'a self, row: &'a Row) -> Option<&'a Row> {
        let mut current = Some(row);
        while let Some(row) = current {
            if row.instanced {
                return Some(row);
            }
            current = row.parent.as_deref().and_then(|id| self.row(id));
        }
        None
    }

    /// The row and every enclosing group are enabled and valid.
    pub(super) fn wanted(&self, row: &Row) -> bool {
        if row.invalid.is_some() || !matches!(row.disabled, Ok(false)) {
            return false;
        }
        match &row.parent {
            None => true,
            Some(parent) => self.row(parent).is_some_and(|p| self.wanted(p)),
        }
    }
}
