//! Plugin definitions another language hands over: a `plugin`, function or
//! object reference a Rust service takes as a [`rutis::Definition`]
//! (`docs/design-plugin-values-2026-10-25.md` §5, §6).
//!
//! The definition stays in the runtime that defined it. Each copy is a row
//! of that runtime whose entry is the definition itself; rutis-loader mounts
//! it ([`PluginMounter`]), with the keys of the instance it is mounted in.

use std::any::Any;
use std::sync::Arc;

use rutis::host::{BoxFuture, DynPlugin, ErrorKind, Key};
use rutis::internal::definitions::{self, Source};
use rutis::internal::service::erased_serde;
use rutis::{Ctx, Definition};
use serde_json::Value;

use crate::runtime::{Error, PluginSchema, Process, RowSchema, RowService};
use crate::session::{CallerRow, HostDispatch, Marked, Reference, Value as RpcValue};

/// Mounts one copy of a plugin definition another language handed over:
/// rutis-loader provides it under [`mounter_key`], from the root of its
/// tree, and builds the copy as a row of the definition's runtime at `at`.
pub trait PluginMounter: Send + Sync + 'static {
    fn mount(
        &self,
        at: &Ctx,
        plugin: &ForeignPlugin,
        config: Value,
    ) -> Result<Box<dyn DynPlugin>, rutis::Error>;
}

/// Where the [`PluginMounter`] is provided.
pub fn mounter_key() -> Key<dyn PluginMounter> {
    Key::of()
}

/// A plugin definition of another language's runtime, as its copies are
/// loaded: the process, the definition (a reference that only goes back
/// there), and what it declares.
pub struct ForeignPlugin {
    process: Arc<Process>,
    reference: Reference,
    schema: RowSchema,
    name: String,
    row: Option<String>,
}

impl ForeignPlugin {
    /// The runtime that defined it, where its copies run.
    pub fn process(&self) -> &Arc<Process> {
        &self.process
    }

    /// The definition, to send back as a row's entry
    /// ([`Process::load_definition_exporting`]).
    pub fn reference(&self) -> &Reference {
        &self.reference
    }

    /// What it declares: config schema, injected and provided services.
    pub fn schema(&self) -> &RowSchema {
        &self.schema
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The id of the row that handed it over, which its copies are named
    /// by; `None` for one handed over outside any row's call.
    pub fn row(&self) -> Option<&str> {
        self.row.as_deref()
    }
}

struct Foreign {
    plugin: ForeignPlugin,
    encoded: Marked,
}

impl Source for Foreign {
    fn build(&self, at: &Ctx, config: Value) -> Result<Box<dyn DynPlugin>, rutis::Error> {
        let mounter = at.get::<dyn PluginMounter>().ok_or_else(|| {
            failed(format!(
                "mounting plugin definition {} needs rutis-loader in the tree",
                self.plugin.name
            ))
        })?;
        mounter.mount(at, &self.plugin, config)
    }

    fn encode(&self) -> Result<&dyn erased_serde::Serialize, String> {
        Ok(&self.encoded)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The plugin definition `reference`, handed over by `process` in a call of
/// `row` (`None`: outside any row's call). Its declarations are read from
/// the runtime once; the same value handed over again is the same
/// definition while that one is held.
pub(crate) async fn read(
    process: Arc<Process>,
    reference: Reference,
    row: Option<CallerRow>,
) -> Result<Definition, Error> {
    let not_one = || Error::Value("expected a plugin definition of the runtime that calls".into());
    if !(reference.is_plugin() || reference.is_function() || reference.is_object()) {
        return Err(not_one());
    }
    if !reference.is_from(process.connection()) {
        return Err(Error::Value(
            "a plugin definition can only be handed over by the runtime that defined it".into(),
        ));
    }
    let (identity, home) = reference
        .identity()
        .zip(reference.home_only())
        .ok_or_else(not_one)?;
    if let Some(known) = process.known_definition(identity) {
        return Ok(known);
    }
    let PluginSchema { name, schema } = process.read_plugin(&reference).await?;
    let row_id = row.as_ref().map(|row| row.id().to_owned());
    let name = name
        .or_else(|| row_id.clone())
        .unwrap_or_else(|| "plugin".to_owned());
    let (origin, scope) = match &row {
        Some(row) => (Some(row.ctx().clone()), None),
        None => (None, Some(process.scope().to_vec())),
    };
    let source = Foreign {
        plugin: ForeignPlugin {
            process: process.clone(),
            reference: home.clone(),
            schema,
            name: name.clone(),
            row: row_id,
        },
        encoded: Marked(home),
    };
    let definition = definitions::from_source(origin.as_ref(), scope, name, Box::new(source));
    process.remember_definition(identity, &definition, row.is_none());
    Ok(definition)
}

/// The plugin definition `value` a service of another language returned:
/// `dispatch` is that service (a row's, [`RowService`]). The definition
/// belongs to the row providing the service; its declarations are read
/// before this resolves (design R2).
pub fn returned(
    dispatch: &dyn HostDispatch,
    value: RpcValue,
) -> BoxFuture<'static, Result<Definition, Error>> {
    let service = dispatch
        .as_any()
        .and_then(|any| any.downcast_ref::<RowService>())
        .map(|service| (service.process().clone(), service.owner().cloned()));
    Box::pin(async move {
        let (process, owner) = service.ok_or_else(|| {
            Error::Value("only a service of a language runtime returns plugin definitions".into())
        })?;
        let RpcValue::Reference(reference) = value else {
            return Err(Error::Value(
                "the service did not return a plugin definition".into(),
            ));
        };
        read(process, reference, owner).await
    })
}

fn failed(message: String) -> rutis::Error {
    ErrorKind::PluginFailed(message.into()).into()
}
