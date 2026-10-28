//! `rutis-host <command> --help` and `-h`: each command prints its own lines
//! of the usage and exits 0, doing nothing else (a change here is a change
//! users see, to be explained in review).

use std::path::Path;
use std::process::Command;

const COMMANDS: [&str; 10] = [
    "run", "dev", "check", "status", "call", "new", "add", "remove", "upgrade", "types",
];

fn host(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_rutis-host"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn every_command_prints_its_usage_for_help() {
    let dir = tempfile::tempdir().unwrap();
    for command in COMMANDS {
        for flag in ["--help", "-h"] {
            let (status, stdout, stderr) = host(dir.path(), &[command, flag]);
            assert_eq!(status, 0, "{command} {flag}: {stderr}");
            assert_eq!(stderr, "", "{command} {flag}");
            let mut lines = stdout.lines();
            assert_eq!(lines.next(), Some("usage:"), "{command} {flag}");
            let entry = lines.next().unwrap_or_default();
            assert!(
                entry.starts_with(&format!("  rutis-host {command} ")),
                "{command} {flag}: {stdout}"
            );
        }
    }
}

/// The options of `run` are listed with it, those of `dev` alone are not.
#[test]
fn run_help_word_for_word() {
    let dir = tempfile::tempdir().unwrap();
    let (status, stdout, _) = host(dir.path(), &["run", "--help"]);
    assert_eq!(status, 0);
    assert_eq!(
        stdout,
        "\
usage:
  rutis-host run [rutis.json]                 run what the configuration names
    run and dev: --shutdown-timeout <seconds> how long cleanups may take after
                                              Ctrl-C or SIGTERM (default 10, or
                                              RUTIS_SHUTDOWN_TIMEOUT)
    run and dev: --prefix-output, --no-prefix-output
                                              start each line a plugin writes with its
                                              row's name ([weather]); on in dev, off in run
    run: --watch-config                       read rutis.json again when it changes
                                              (SIGUSR1 does too, on Unix)
"
    );
}

/// `--help` anywhere after the command wins: nothing is created, installed
/// or read.
#[test]
fn help_among_other_arguments_does_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        ["new", "jot", "-h"].as_slice(),
        ["add", "npm:left-pad", "--help"].as_slice(),
        ["run", "missing.json", "--help"].as_slice(),
    ] {
        let (status, stdout, stderr) = host(dir.path(), args);
        assert_eq!(status, 0, "{args:?}: {stderr}");
        assert!(stdout.starts_with("usage:\n"), "{args:?}: {stdout}");
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
