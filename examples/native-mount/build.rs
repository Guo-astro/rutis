fn main() {
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() != Ok("unix") {
        return;
    }
    // Cordis plugins mounted by this crate are listed in Cargo.toml.
    rutis_bridge::cordis::build::from_manifest()
        .expect("generate Cordis bindings during the normal Cargo build");
}
