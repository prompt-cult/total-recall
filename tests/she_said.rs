use rusqlite::Connection;
use std::path::{Path, PathBuf};
use total_recall::RolloutAdapter;
use total_recall::rollout::opencode::OpenCodeAdapter;
use total_recall::rollout::vibe::VibeAdapter;

fn tmp_db(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("she_said_{}.db", name));
    let _ = std::fs::remove_file(&path);
    path
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn create_fixture(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
            id text PRIMARY KEY,
            parent_id text,
            directory text,
            title text,
            time_created integer NOT NULL,
            time_updated integer NOT NULL
        );
        CREATE TABLE message (
            id text PRIMARY KEY,
            session_id text NOT NULL,
            time_created integer NOT NULL,
            time_updated integer NOT NULL,
            data text NOT NULL
        );
        CREATE TABLE part (
            id text PRIMARY KEY,
            message_id text NOT NULL,
            session_id text NOT NULL,
            time_created integer NOT NULL,
            time_updated integer NOT NULL,
            data text NOT NULL
        );",
    )
    .unwrap();

    let now = now_ms();
    let hour = 3_600_000i64;
    let day = 86_400_000i64;

    let sessions: Vec<(&str, &str, &str, &str, i64, i64)> = vec![
        (
            "ses_alpha0001",
            "",
            "/Users/dev/alpha",
            "alpha session",
            now - hour,
            now,
        ),
        (
            "ses_beta0002",
            "",
            "/Users/dev/beta",
            "beta session",
            now - 2 * hour,
            now - hour,
        ),
        (
            "ses_gamma0003",
            "",
            "/Users/dev/alpha",
            "gamma session",
            now - 100 * day,
            now - 100 * day,
        ),
    ];
    for (id, parent, directory, title, created, updated) in sessions {
        conn.execute(
            "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                id,
                if parent.is_empty() {
                    None
                } else {
                    Some(parent)
                },
                directory,
                title,
                created,
                updated
            ],
        )
        .unwrap();
    }

    // alpha: role-bearing messages with matched and unmatched parts
    let long_text = format!("git {}", "é".repeat(1000));
    let msgs: Vec<(&str, &str, i64, &str)> = vec![
        (
            "ma1",
            "ses_alpha0001",
            now - 300_000,
            r#"{"role":"user","time":{"created":1}}"#,
        ),
        (
            "ma2",
            "ses_alpha0001",
            now - 200_000,
            r#"{"role":"assistant","time":{"created":2}}"#,
        ),
        (
            "mb1",
            "ses_beta0002",
            now - hour,
            r#"{"role":"user","time":{"created":3}}"#,
        ),
        (
            "mg1",
            "ses_gamma0003",
            now - 100 * day,
            r#"{"role":"user","time":{"created":4}}"#,
        ),
    ];
    for (id, sid, t, data) in msgs {
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![id, sid, t, data],
        )
        .unwrap();
    }

    let parts: Vec<(&str, &str, &str, i64, String)> = vec![
        ("pa1", "ma1", "ses_alpha0001", now - 300_000, r#"{"type":"text","text":"Please run Git status for me"}"#.into()),
        ("pa2", "ma1", "ses_alpha0001", now - 290_000, r#"{"type":"text","synthetic":true,"text":"git hint injected"}"#.into()),
        ("pa3", "ma1", "ses_alpha0001", now - 280_000, serde_json::json!({"type":"text","text":long_text}).to_string()),
        ("pa4", "ma2", "ses_alpha0001", now - 200_000, r#"{"type":"text","text":"Created a BRANCH for you"}"#.into()),
        ("pa5", "ma2", "ses_alpha0001", now - 190_000, r#"{"type":"text","text":"merged the branch now"}"#.into()),
        ("pa6", "ma2", "ses_alpha0001", now - 180_000, r#"{"type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"git commit -m x"},"output":"ok"}}"#.into()),
        ("pa7", "ma2", "ses_alpha0001", now - 170_000, r#"{"type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"git commit -m x"},"output":"ok"}}"#.into()),
        ("pa8", "ma2", "ses_alpha0001", now - 160_000, r#"{"type":"tool","tool":"read","state":{"status":"completed","input":{"file_path":"/tmp/alpha.rs"},"output":"git git git"}}"#.into()),
        ("pa9", "ma2", "ses_alpha0001", now - 150_000, r#"{"type":"tool","tool":"branchtool","state":{"status":"pending"}}"#.into()),
        ("pb1", "mb1", "ses_beta0002", now - hour, r#"{"type":"text","text":"git in beta"}"#.into()),
        ("pg1", "mg1", "ses_gamma0003", now - 100 * day, r#"{"type":"text","text":"git in gamma"}"#.into()),
    ];
    for (id, mid, sid, t, data) in parts {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![id, mid, sid, t, data],
        )
        .unwrap();
    }
}

fn fixture_adapter(name: &str) -> OpenCodeAdapter {
    let path = tmp_db(name);
    create_fixture(&path);
    OpenCodeAdapter::with_root(&path)
}

fn w(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_he_said_matches_user_text_case_insensitive() {
    let adapter = fixture_adapter("he_said");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha"]), &w(&["git"]), 48, None)
        .unwrap();
    assert!(
        report.contains("Please run Git status for me"),
        "{}",
        report
    );
    assert!(report.contains("--- HE SAID:"), "{}", report);
}

#[test]
fn test_she_said_matches_assistant_text() {
    let adapter = fixture_adapter("she_said");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha"]), &w(&["branch"]), 48, None)
        .unwrap();
    assert!(report.contains("Created a BRANCH for you"), "{}", report);
    assert!(report.contains("--- SHE SAID:"), "{}", report);
}

#[test]
fn test_case_insensitivity_both_directions() {
    let adapter = fixture_adapter("both_dirs");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha"]), &w(&["GIT", "BRANCH"]), 48, None)
        .unwrap();
    assert!(
        report.contains("Please run Git status for me"),
        "{}",
        report
    );
    assert!(report.contains("merged the branch now"), "{}", report);
}

#[test]
fn test_they_did_tool_input_matches_and_output_does_not() {
    let adapter = fixture_adapter("they_did");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha"]), &w(&["git"]), 48, None)
        .unwrap();
    assert!(
        report.contains("bash: git commit -m x"),
        "tool input match: {}",
        report
    );
    assert!(
        !report.contains("read_file(/tmp/alpha.rs)"),
        "output-only match must not appear: {}",
        report
    );
    assert!(
        !report.contains("branchtool("),
        "non-matching tool name must not appear: {}",
        report
    );
    assert!(report.contains("--- THEY DID (tool calls):"), "{}", report);
}

#[test]
fn test_they_did_tool_name_only_match() {
    let adapter = fixture_adapter("tool_name_only");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha"]), &w(&["branchtool"]), 48, None)
        .unwrap();
    assert!(report.contains("branchtool("), "{}", report);
}

#[test]
fn test_synthetic_text_parts_are_skipped() {
    let adapter = fixture_adapter("synthetic");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha"]), &w(&["git"]), 48, None)
        .unwrap();
    assert!(
        !report.contains("git hint injected"),
        "synthetic part must be skipped: {}",
        report
    );
}

#[test]
fn test_empty_session_list_hours_bound_excludes_old_session() {
    let adapter = fixture_adapter("hours_bound");
    let report = adapter
        .she_said_he_said_action(&[], &w(&["git"]), 48, None)
        .unwrap();
    assert!(report.contains("=== alpha session"), "{}", report);
    assert!(report.contains("=== beta session"), "{}", report);
    assert!(
        !report.contains("gamma"),
        "100-day-old session must be excluded by the 48h bound: {}",
        report
    );
}

#[test]
fn test_empty_session_list_hours_zero_means_no_bound() {
    let adapter = fixture_adapter("no_bound");
    let report = adapter
        .she_said_he_said_action(&[], &w(&["git"]), 0, None)
        .unwrap();
    assert!(report.contains("=== gamma session"), "{}", report);
}

#[test]
fn test_directory_filter_excludes_other_directory() {
    let adapter = fixture_adapter("dir_filter");
    let report = adapter
        .she_said_he_said_action(&[], &w(&["git"]), 48, Some("/Users/dev/alpha"))
        .unwrap();
    assert!(report.contains("=== alpha session"), "{}", report);
    assert!(
        !report.contains("=== beta session"),
        "beta lives in another directory: {}",
        report
    );
}

#[test]
fn test_explicit_partial_ids_resolve_most_recent_first() {
    let adapter = fixture_adapter("explicit_order");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_gamma", "ses_beta"]), &w(&["git"]), 48, None)
        .unwrap();
    let beta = report.find("=== beta session").expect(&report);
    let gamma = report.find("=== gamma session").expect(&report);
    assert!(
        beta < gamma,
        "sessions must be most-recent first: {}",
        report
    );
}

#[test]
fn test_unmatched_partial_id_reported_in_header_not_fatal() {
    let adapter = fixture_adapter("unmatched");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha", "ses_nope"]), &w(&["git"]), 48, None)
        .unwrap();
    assert!(
        report.starts_with("# she-said-he-said-action — terms: git\n"),
        "{}",
        report
    );
    assert!(
        report.contains("1 sessions scanned, 1 with hits, 1 unmatched session ids: ses_nope"),
        "{}",
        report
    );
    assert!(report.contains("=== alpha session"), "{}", report);
}

#[test]
fn test_consecutive_identical_they_did_lines_deduped() {
    let adapter = fixture_adapter("dedup");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha"]), &w(&["git"]), 48, None)
        .unwrap();
    let count = report.matches("bash: git commit -m x").count();
    assert_eq!(
        count, 1,
        "identical consecutive THEY DID lines deduped: {}",
        report
    );
}

#[test]
fn test_truncation_at_1500_bytes_lands_on_utf8_boundary() {
    let adapter = fixture_adapter("truncation");
    let report = adapter
        .she_said_he_said_action(&w(&["ses_alpha"]), &w(&["git"]), 48, None)
        .unwrap();
    let truncated = format!("git {}", "é".repeat(748));
    assert_eq!(truncated.len(), 1500);
    assert!(report.contains(&truncated), "{}", report);
    let over = format!("git {}", "é".repeat(749));
    assert!(!report.contains(&over), "{}", report);
}

#[test]
fn test_default_trait_impl_not_implemented_error() {
    let vibe = VibeAdapter::new();
    let err = vibe
        .she_said_he_said_action(&w(&["ses_x"]), &w(&["git"]), 48, None)
        .unwrap_err();
    assert_eq!(
        err,
        "she_said_he_said_action is not implemented for harness 'vibe'; supported: opencode"
    );
}

#[test]
fn test_empty_words_is_an_error() {
    let adapter = fixture_adapter("empty_words");
    let err = adapter
        .she_said_he_said_action(&[], &[], 48, None)
        .unwrap_err();
    assert!(!err.is_empty(), "empty words must be a caller error");
    assert!(
        err.to_lowercase().contains("word") || err.to_lowercase().contains("term"),
        "error must be clear about the words/terms problem: {}",
        err
    );
}

#[test]
fn test_list_sessions_directory_populated() {
    let adapter = fixture_adapter("list_dir");
    let sessions = adapter.list_sessions();
    let alpha = sessions
        .iter()
        .find(|s| s.session_id == "ses_alpha0001")
        .expect("alpha session listed");
    assert_eq!(alpha.directory.as_deref(), Some("/Users/dev/alpha"));
    assert_eq!(alpha.title, "alpha session");
    assert_eq!(alpha.user_count, 1);
    assert_eq!(alpha.assistant_count, 1);
    assert_eq!(alpha.tool_count, 4);
    assert_eq!(alpha.line_count, 9);
    assert!(!alpha.has_compaction);
    assert!(alpha.file_size > 0);
}
