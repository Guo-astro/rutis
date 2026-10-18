fn main() {
    // The services the rows use, by name. The types are in a dependency
    // crate; the generator reads their source and writes the dispatchers
    // and the TypeScript declarations (copied to rows/services.d.ts).
    rutis_bridge::cordis::build::Exports::new()
        .service("calendar", "rust_services_calendar::Calendar")
        .service_in("notes", "rust_services_calendar::Notes", "session")
        .typescript("rows/services.d.ts")
        .generate()
        .expect("generate the exported services");
}
