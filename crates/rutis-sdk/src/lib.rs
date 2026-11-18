//! Shared Rust ABI for trusted first-party plugins. The host and every plugin
//! must link this exact binary with the same compiler and build configuration.

#[cfg(panic = "abort")]
compile_error!("rutis-sdk requires panic = unwind");

/// The rutis kernel, as plugins reach it.
///
/// Everything of `rutis`, but `service`, the `#[rutis::service]` marker:
/// here its generated code names `::rutis_sdk::rutis`, since a plugin depends
/// on this crate only (or, built against an sdk-bundle, on nothing: the
/// packer injects `rutis_sdk` with `--extern`).
pub mod rutis {
    pub use ::rutis::internal::__sdk_service as service;
    pub use ::rutis::*;
}
pub use serde_json;
pub use tokio;

pub type ConfigValue = serde_json::Value;

// Must stay `System` on every platform. With a dynamic libstd, libstd's own
// allocations do not go through this allocator on Mach-O and Windows, and a
// custom allocator crashes on Linux too (rust-lang/rust#100781, #114518).
// See docs/design-dylib-macos-windows-2026-10-03.md §3.5.
#[global_allocator]
static SDK_ALLOCATOR: std::alloc::System = std::alloc::System;

include!(concat!(env!("OUT_DIR"), "/identity.rs"));
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

#[inline(never)]
pub fn loaded_sdk_id() -> &'static str {
    SDK_ID
}

/// The boot blob's const ABI: its layout and the SHA-256 text check,
/// defined in `rutis-abi`. That crate has no dependency and nothing to
/// build, so `cargo kani` builds it and proves the code that ships (#1271);
/// this crate re-exports it, so plugin and host builds see no change.
pub use rutis_abi::{boot_meta, is_sha256, BOOT_MAGIC, BOOT_SIZE};

/// A C ABI diagnostic query. The independent launcher performs the actual
/// pre-execution file verification before any SDK code can run.
///
/// # Safety
///
/// `buf` is null or points to at least `cap` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn rutis_sdk_boot_id(buf: *mut u8, cap: usize) -> usize {
    // SAFETY: the contract for `buf` and `cap` is the same; the body is
    // `rutis-abi`'s so that Kani proves it.
    unsafe { rutis_abi::write_boot_id(SDK_ID, buf, cap) }
}

#[derive(Clone, Debug)]
pub struct PluginMeta {
    pub sdk_id: &'static str,
    pub sdk_artifact_sha256: &'static str,
    pub id: &'static str,
    pub version: &'static str,
}

/// The factory `export_plugin!(build: …)` exports: `build` makes the plugin
/// from the config, decoded from JSON as rutis-loader's `Builtins::register`
/// decodes it (a `null` config as `null` first, then as `{}`); the
/// dependencies come from `P::Deps`. The factory is named by the exported
/// `id`, as a `factory:` export names its own. `schema`: the export has a
/// schema, which the loader validates the config against first (a decoder
/// error is then one sentence, config schema design §3.3).
#[doc(hidden)]
pub fn build_factory<C, P, B>(
    id: &'static str,
    build: B,
    schema: bool,
) -> impl rutis::host::PluginFactory<ConfigValue>
where
    C: ::rutis::serde::de::DeserializeOwned + Send + Sync + 'static,
    P: rutis::Plugin,
    B: Fn(&C) -> rutis::Result<P> + Send + Sync + 'static,
{
    Exported {
        id,
        factory: ::rutis::internal::build_fn(move |config: &ConfigValue| {
            build(&decode::<C>(config, schema)?)
        }),
    }
}

struct Exported<F> {
    id: &'static str,
    factory: F,
}

impl<F: rutis::host::PluginFactory<ConfigValue>> rutis::host::PluginFactory<ConfigValue>
    for Exported<F>
{
    fn name(&self) -> &str {
        self.id
    }

    fn injects(&self) -> Vec<rutis::host::TypeKey> {
        self.factory.injects()
    }

    fn validate_config(&self, config: &ConfigValue) -> rutis::Result {
        self.factory.validate_config(config)
    }

    fn build(&self, config: &ConfigValue) -> rutis::Result<Box<dyn rutis::host::DynPlugin>> {
        self.factory.build(config)
    }
}

fn decode<C: ::rutis::serde::de::DeserializeOwned>(
    value: &ConfigValue,
    schema: bool,
) -> rutis::Result<C> {
    rutis::config::decode(value, schema).map_err(rutis::ConfigError::into_error)
}

/// Export a plugin and a static, pre-dlopen identity blob from a plugin
/// dylib. `RUTIS_SDK_ARTIFACT_SHA256` must be set by the two-stage build.
///
/// `build:` takes the constructor `Builtins::register` takes,
/// `Fn(&C) -> rutis::Result<P>` with `P: rutis::Plugin` and `C` decoded
/// from the JSON config (`serde_json::Value` for the raw config):
///
/// ```ignore
/// rutis_sdk::export_plugin!(id: "weather", build: Weather::new);
/// ```
///
/// `factory:` takes a `PluginFactory<serde_json::Value>` instead, for a
/// plugin whose dependencies are known only at run time.
///
/// `schema:` optionally gives the JSON Schema of the config, as a
/// `serde_json::Value`, for hosts that show config forms (rutis-loader).
#[macro_export]
macro_rules! export_plugin {
    (id: $id:literal, build: $build:expr $(,)?) => {
        $crate::export_plugin!(@export $id, $crate::build_factory($id, $build, false), ::core::option::Option::None);
    };
    (id: $id:literal, build: $build:expr, schema: $schema:expr $(,)?) => {
        $crate::export_plugin!(@export $id, $crate::build_factory($id, $build, true), ::core::option::Option::Some($schema));
    };
    (id: $id:literal, factory: $factory:expr $(,)?) => {
        $crate::export_plugin!(@export $id, $factory, ::core::option::Option::None);
    };
    (id: $id:literal, factory: $factory:expr, schema: $schema:expr $(,)?) => {
        $crate::export_plugin!(@export $id, $factory, ::core::option::Option::Some($schema));
    };
    (@export $id:literal, $factory:expr, $schema:expr) => {
        #[cfg(panic = "abort")]
        compile_error!("rutis dylib plugins require panic = unwind");
        const RUTIS_PLUGIN_ARTIFACT_SHA256: &str = env!("RUTIS_SDK_ARTIFACT_SHA256");
        const _: () = assert!(
            $crate::is_sha256(RUTIS_PLUGIN_ARTIFACT_SHA256),
            "invalid SDK artifact SHA-256"
        );
        // Section names per object format; rutis-dylib-meta reads them back.
        // PE image section names are limited to 8 bytes.
        #[used]
        #[cfg_attr(target_os = "linux", link_section = ".note.rutis.meta")]
        #[cfg_attr(target_vendor = "apple", link_section = "__DATA,__rutis_meta")]
        #[cfg_attr(windows, link_section = ".rutism")]
        static RUTIS_BOOT_META: [u8; $crate::BOOT_SIZE] = $crate::boot_meta(
            $crate::SDK_ID,
            RUTIS_PLUGIN_ARTIFACT_SHA256,
            $id,
            env!("CARGO_PKG_VERSION"),
        );
        #[no_mangle]
        pub fn rutis_plugin_meta() -> $crate::PluginMeta {
            $crate::PluginMeta {
                sdk_id: $crate::SDK_ID,
                sdk_artifact_sha256: RUTIS_PLUGIN_ARTIFACT_SHA256,
                id: $id,
                version: env!("CARGO_PKG_VERSION"),
            }
        }
        /// The config's JSON Schema as text, or `None`. Optional for hosts:
        /// plugins built before it existed simply lack the symbol.
        #[no_mangle]
        pub fn rutis_plugin_config_schema() -> Option<String> {
            let schema: Option<$crate::serde_json::Value> = $schema;
            schema.map(|schema| schema.to_string())
        }
        #[no_mangle]
        pub fn rutis_plugin_entry() -> Result<
            Box<dyn $crate::rutis::host::PluginFactory<$crate::ConfigValue>>,
            $crate::rutis::Error,
        > {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Box::new($factory) as Box<dyn $crate::rutis::host::PluginFactory<$crate::ConfigValue>>
            }))
            .map_err(|_| $crate::rutis::Error::msg("plugin factory construction panicked"))
        }
    };
}
