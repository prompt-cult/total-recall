#[allow(dead_code, unused_imports)]
mod cli {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"));
}

use clap::Parser;
use clap::error::ErrorKind;
use cli::Cli;
use std::process::Command;

type FlagCheck = (Vec<&'static str>, fn(&Cli) -> bool);

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_total-recall"))
}

#[test]
fn version_flag_is_wired() {
    let err = match Cli::try_parse_from(["bin", "--version"]) {
        Ok(_) => panic!("--version must exit with the version display, not parse as flags"),
        Err(e) => e,
    };
    assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    let rendered = err.render().to_string();
    assert!(
        rendered.contains(env!("CARGO_PKG_VERSION")),
        "version output must carry the package version, got: {rendered}"
    );
}

#[test]
fn global_flags_accepted_before_subcommand() {
    let cli = Cli::try_parse_from(["bin", "--verbose", "compact"]).unwrap();
    assert!(cli.verbose);
    assert!(matches!(cli.command, cli::Command::Compact));
}

#[test]
fn global_flags_accepted_after_subcommand() {
    let cli = Cli::try_parse_from(["bin", "compact", "--verbose"])
        .inspect_err(|e| panic!("clap rejected flag after subcommand: {e}"));
    assert!(cli.is_ok());
    let cli = cli.unwrap();
    assert!(cli.verbose);
    assert!(matches!(cli.command, cli::Command::Compact));
}

#[test]
fn each_documented_global_flag_works_after_subcommand() {
    let cases: [FlagCheck; 6] = [
        (vec!["bin", "list", "--json"], |c| c.json),
        (vec!["bin", "list", "--markdown"], |c| c.markdown),
        (vec!["bin", "compact", "--full"], |c| c.full),
        (vec!["bin", "compact", "--verbose"], |c| c.verbose),
        (vec!["bin", "compact", "--session", "abc"], |c| {
            c.session.as_deref() == Some("abc")
        }),
        (vec!["bin", "list", "--harness", "vibe"], |c| {
            c.harness.as_deref() == Some("vibe")
        }),
    ];
    for (args, check) in cases {
        let cli = Cli::try_parse_from(args)
            .unwrap_or_else(|e| panic!("clap rejected flag after subcommand: {e}"));
        assert!(check(&cli));
    }
}

#[test]
fn sheep_without_query_exits_nonzero_with_useful_stderr() {
    let output = bin()
        .arg("do-android-dream-of-electric-sheep")
        .output()
        .expect("failed to spawn total-recall binary");
    assert!(
        !output.status.success(),
        "missing --query must exit non-zero, got stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--query") || stderr.contains("required"),
        "stderr must tell the user --query is required, got: {stderr}"
    );
}

#[test]
fn index_and_sheep_appear_in_help() {
    let output = bin()
        .arg("--help")
        .output()
        .expect("failed to spawn total-recall binary");
    assert!(output.status.success(), "--help must succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("index"),
        "--help must mention the index subcommand, got: {stdout}"
    );
    assert!(
        stdout.contains("do-android-dream-of-electric-sheep"),
        "--help must mention the do-android-dream-of-electric-sheep subcommand, got: {stdout}"
    );
}

#[test]
fn harness_global_flag_works_after_index_and_sheep() {
    let index = Cli::try_parse_from(["bin", "index", "--harness", "vibe"])
        .unwrap_or_else(|e| panic!("clap rejected --harness after index: {e}"));
    assert!(matches!(index.command, cli::Command::Index { .. }));
    assert_eq!(index.harness.as_deref(), Some("vibe"));

    let sheep = Cli::try_parse_from([
        "bin",
        "do-android-dream-of-electric-sheep",
        "--harness",
        "vibe",
        "--query",
        "test",
    ])
    .unwrap_or_else(|e| panic!("clap rejected --harness after sheep: {e}"));
    assert!(matches!(sheep.command, cli::Command::Sheep { .. }));
    assert_eq!(sheep.harness.as_deref(), Some("vibe"));
}

#[test]
fn index_hours_accepts_numbers_and_rejects_garbage() {
    Cli::try_parse_from(["bin", "index", "--hours", "0"]).expect("--hours 0 must parse");
    Cli::try_parse_from(["bin", "index", "--hours", "999"]).expect("--hours 999 must parse");

    let err = match Cli::try_parse_from(["bin", "index", "--hours", "abc"]) {
        Ok(_) => panic!("--hours abc must be rejected"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("invalid value") || err.kind() == ErrorKind::ValueValidation,
        "expected value validation error for --hours abc, got: {err}"
    );
}
