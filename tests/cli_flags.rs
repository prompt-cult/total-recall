mod common;

#[allow(dead_code, unused_imports)]
mod cli {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"));
}

use clap::Parser;
use clap::error::ErrorKind;
use cli::Cli;
use std::path::Path;
use std::process::Command;

use common::scratch::{child_cwd, scratch};
use total_recall::LISTING_ROW_CAP;

type FlagCheck = (Vec<&'static str>, fn(&Cli) -> bool);

/// The binary, pinned to an empty working directory outside the repo tree,
/// with the vendor keys removed: `dotenvy` walks up parent directories, so a
/// child left at the crate-root CWD would read the developer's real `.env`.
/// The caller owns the directory and clears it once the child has run.
fn bin(cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_total-recall"));
    cmd.current_dir(cwd)
        .env_remove("INCEPTION_API_KEY")
        .env_remove("MISTRAL_API_KEY");
    cmd
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
    let cwd = child_cwd("sheep_flag");
    let output = bin(&cwd)
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
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn index_and_sheep_appear_in_help() {
    let cwd = child_cwd("help_flag");
    let output = bin(&cwd)
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
    let _ = std::fs::remove_dir_all(&cwd);
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

#[test]
fn list_hours_parses_and_defaults_to_the_unbounded_stream() {
    let cli = Cli::try_parse_from(["bin", "list"]).expect("`list` parses with no flags");
    assert!(
        matches!(cli.command, cli::Command::List { hours: 0 }),
        "`list` with no --hours is the unbounded stream (hours 0)"
    );
    let cli = Cli::try_parse_from(["bin", "list", "--hours", "24"]).expect("--hours 24 must parse");
    assert!(matches!(cli.command, cli::Command::List { hours: 24 }));

    let err = match Cli::try_parse_from(["bin", "list", "--hours", "abc"]) {
        Ok(_) => panic!("--hours abc must be rejected"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("invalid value") || err.kind() == ErrorKind::ValueValidation,
        "expected value validation error for --hours abc, got: {err}"
    );
}

/// An opencode fixture of `count` sessions all updated now, with strictly
/// descending `time_updated` so ordering is deterministic, and one tiny user
/// message + part each so the aggregates are non-zero.
fn build_listing_fixture(root: &std::path::Path, count: usize) {
    use rusqlite::Connection;
    let conn = Connection::open(root.join("fixture.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
            id text PRIMARY KEY, parent_id text, directory text, title text,
            time_created integer NOT NULL, time_updated integer NOT NULL);
        CREATE TABLE message (
            id text PRIMARY KEY, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);
        CREATE TABLE part (
            id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL,
            time_created integer NOT NULL, time_updated integer NOT NULL,
            data text NOT NULL);",
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    for i in 0..count {
        let sid = format!("ses_clilist{i:03}000000000000000000000aa");
        let updated = now - (i as i64) * 1_000;
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                &sid,
                format!("/Users/dev/clilist{i:03}"),
                format!("cli listing session {i}"),
                updated,
                updated
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![
                format!("cmsg{i:03}"),
                &sid,
                updated,
                "{\"role\":\"user\",\"time\":{\"created\":1}}"
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![
                format!("cpart{i:03}"),
                &sid,
                format!("cmsg{i:03}"),
                updated,
                serde_json::json!({"type":"text","text":format!("cli listing body {i}")})
                    .to_string()
            ],
        )
        .unwrap();
    }
}

#[test]
fn list_hours_scopes_the_listing_and_names_the_held_back_rows_on_stderr() {
    let fixture = scratch("clilist_fixture");
    build_listing_fixture(&fixture, 250);
    let cwd = child_cwd("clilist_run");
    let scoped = bin(&cwd)
        .arg("--harness")
        .arg("opencode")
        .arg("list")
        .arg("--hours")
        .arg("1")
        .arg("--json")
        .env("TOTAL_RECALL_OPENCODE_ROOT", fixture.join("fixture.db"))
        .output()
        .expect("spawn total-recall list --hours 1 --json");
    assert!(
        scoped.status.success(),
        "list --hours 1 must succeed: {}",
        String::from_utf8_lossy(&scoped.stderr)
    );

    // stdout is the scoped listing as machine-parseable JSON: the 200 most
    // recent rows of the window plus the window count.
    let stdout: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&scoped.stdout))
        .expect("scoped list --json prints one JSON object");
    let sessions = stdout["sessions"]
        .as_array()
        .expect("the scoped listing renders its rows under `sessions`");
    assert_eq!(
        sessions.len(),
        250_usize.min(LISTING_ROW_CAP),
        "the scoped listing prints at most the cap rows of the window"
    );
    assert_eq!(
        stdout["window_count"].as_u64(),
        Some(250),
        "the scoped listing states how many rows the window holds"
    );

    // The held-back count is a stderr notice, so stdout stays
    // machine-parseable.
    let stderr = String::from_utf8_lossy(&scoped.stderr);
    assert!(
        stderr.contains("TRUNCATED: 250 sessions in the 1h window, 200 printed"),
        "the held-back notice must name the window and the printed rows: {stderr}"
    );

    // --hours 0 (the default) keeps the unbounded stream: every session on
    // stdout as a bare array, and no truncation notice.
    let unbounded = bin(&cwd)
        .arg("--harness")
        .arg("opencode")
        .arg("list")
        .arg("--json")
        .env("TOTAL_RECALL_OPENCODE_ROOT", fixture.join("fixture.db"))
        .output()
        .expect("spawn total-recall list --json");
    assert!(unbounded.status.success());
    let rows: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&unbounded.stdout))
        .expect("the unbounded list prints one JSON array");
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(250),
        "the unbounded stream lists every session, no cap"
    );
    assert!(
        !String::from_utf8_lossy(&unbounded.stderr).contains("TRUNCATED"),
        "the unbounded stream never signals truncation"
    );

    let _ = std::fs::remove_dir_all(&cwd);
}
