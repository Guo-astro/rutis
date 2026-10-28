fn main() {
    // The TypeScript declarations of the marked services in src/lib.rs,
    // read with the dependency crate's types they use, rewritten into
    // rows/services.d.ts when they change. The services need no build step;
    // `rutis-host types` writes the same file.
    rutis_types::Exports::new()
        .typescript("rows/services.d.ts")
        .generate()
        .expect("generate the service declarations");
}
