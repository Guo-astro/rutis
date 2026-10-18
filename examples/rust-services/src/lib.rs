//! An application whose rows use its existing Rust services by name: a
//! [`Calendar`](rust_services_calendar::Calendar) for every row and a
//! [`Notes`](rust_services_calendar::Notes) per `session` instance, both
//! from a dependency crate. `build.rs` names them; nothing here implements
//! `HostDispatch`.
//!
//! ```ignore
//! let mut catalog = rutis_loader::ServiceCatalog::new();
//! catalog.export_all(rust_services_example::exports::all());
//! root.provide(Calendar::new())?;   // the native service, as usual
//! ```

rutis_bridge::include_exports!();
