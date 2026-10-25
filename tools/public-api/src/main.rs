//! `rutis-public-api <toolchain> <manifest-path> <package>`: the public API
//! of `package`, one item a line, as `cargo +<toolchain> public-api -p
//! <package> --simplified` prints it (blanket impls left out). The rustdoc
//! JSON is built with `toolchain`, a nightly.

use std::io::Write;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(toolchain), Some(manifest), Some(package), None) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        return Err("usage: rutis-public-api <toolchain> <manifest-path> <package>".into());
    };
    let json = rustdoc_json::Builder::default()
        .toolchain(toolchain)
        .manifest_path(manifest)
        .package(package)
        .build()?;
    let api = public_api::Builder::from_rustdoc_json(json)
        .omit_blanket_impls(true)
        .build()?;
    let mut out = std::io::stdout().lock();
    for item in api.items() {
        writeln!(out, "{item}")?;
    }
    Ok(())
}
