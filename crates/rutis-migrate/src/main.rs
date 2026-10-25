//! `rutis-migrate 0.17 PATH [--write] [--no-check]`
//!
//! Migrates the Rust files under PATH from rutis 0.16 to 0.17. Without
//! `--write` it prints a diff and changes nothing. With `--write` it writes
//! the files, then runs `cargo check --all-targets` in PATH (unless
//! `--no-check`) and lists the errors left, which the `TODO(rutis 0.17)`
//! comments it added point at.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut version = None;
    let mut path = None;
    let mut write = false;
    let mut check = true;
    for arg in &args {
        match arg.as_str() {
            "--write" => write = true,
            "--no-check" => check = false,
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            other if version.is_none() => version = Some(other.to_owned()),
            other if path.is_none() => path = Some(PathBuf::from(other)),
            other => {
                eprintln!("rutis-migrate: unexpected argument {other}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(version) = version else {
        print_help();
        return ExitCode::from(2);
    };
    if version != "0.17" {
        eprintln!("rutis-migrate: only `0.17` (from 0.16) is supported, not {version}");
        return ExitCode::from(2);
    }
    let root = path.unwrap_or_else(|| PathBuf::from("."));
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    let mut changed = 0;
    let mut todos = 0;
    for file in &files {
        let Ok(old) = std::fs::read_to_string(file) else {
            continue;
        };
        let migrated = rutis_migrate::migrate(&old);
        if migrated.source == old {
            continue;
        }
        changed += 1;
        todos += migrated.todos.len();
        let shown = file.display().to_string();
        if write {
            if let Err(error) = std::fs::write(file, &migrated.source) {
                eprintln!("rutis-migrate: {shown}: {error}");
                return ExitCode::FAILURE;
            }
            println!("migrated {shown}");
        } else {
            print!("{}", rutis_migrate::diff(&shown, &old, &migrated.source));
        }
        for todo in &migrated.todos {
            println!("{shown}:{}: TODO {}", todo.line, todo.message);
        }
    }
    println!(
        "{changed} file(s) {}, {todos} TODO(s)",
        if write {
            "changed"
        } else {
            "would change (run with --write)"
        }
    );
    if write && check && root.join("Cargo.toml").exists() {
        println!("running cargo check --all-targets in {}", root.display());
        let output = Command::new("cargo")
            .args(["check", "--all-targets", "--message-format=short"])
            .current_dir(&root)
            .output();
        match output {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let errors: Vec<&str> = stderr.lines().filter(|l| l.contains("error")).collect();
                for line in &errors {
                    println!("{line}");
                }
                if errors.is_empty() {
                    println!("cargo check: no errors");
                }
            }
            Err(error) => eprintln!("rutis-migrate: cargo check: {error}"),
        }
    }
    ExitCode::SUCCESS
}

fn print_help() {
    println!("usage: rutis-migrate 0.17 [PATH] [--write] [--no-check]");
    println!("  migrates the Rust files under PATH (default .) from rutis 0.16 to 0.17;");
    println!("  without --write, prints the diff only");
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().is_some_and(|e| e == "rs") {
            out.push(path.to_owned());
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name.starts_with('.') || name == "node_modules" {
            continue;
        }
        collect(&p, out);
    }
}
