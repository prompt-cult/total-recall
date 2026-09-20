use std::path::PathBuf;

use total_recall::RolloutAdapter;
use total_recall::index;
use total_recall::rollout::mock::MockAdapter;

fn fixture_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sheep_{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_fixture(name: &str, lines: &[String]) -> (PathBuf, MockAdapter) {
    let dir = fixture_dir(name);
    let data_path = dir.join("data.jsonl");
    std::fs::write(&data_path, lines.join("\n") + "\n").unwrap();
    let adapter = MockAdapter::new(&data_path);
    (dir, adapter)
}

fn msg(role: &str, content: &str, timestamp: &str, thinking: Option<&str>) -> String {
    serde_json::json!({
        "role": role,
        "content": content,
        "thinking": thinking,
        "tool_calls_summary": [],
        "injected": false,
        "timestamp": timestamp,
    })
    .to_string()
}

fn w(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn fixture_lines() -> Vec<String> {
    vec![
        msg(
            "user",
            "please review the alpha query engine",
            "2026-09-19T10:00:00Z",
            None,
        ),
        msg(
            "assistant",
            "the alpha engine handles queries well",
            "2026-09-19T10:01:00Z",
            None,
        ),
        msg(
            "assistant",
            "",
            "2026-09-19T10:02:00Z",
            Some("contemplating the electric sheep dream"),
        ),
    ]
}

#[test]
fn test_index_session_writes_meta_and_doc_count() {
    let (_dir, adapter) = write_fixture("meta", &fixture_lines());
    let stats = index::index_session(&adapter, "").unwrap();
    assert_eq!(stats.session_id, "mock");
    assert!(stats.doc_count > 0);
    let meta = adapter
        .shadow_index_root()
        .join("mock")
        .join("total-recall-meta.json");
    assert!(meta.is_file(), "total-recall-meta.json must be written");
    assert!(index::index_exists(&adapter, "mock"));
}

#[test]
fn test_reindex_replaces_without_duplicate_growth() {
    let (_dir, adapter) = write_fixture("reindex", &fixture_lines());
    let first = index::index_session(&adapter, "").unwrap();
    let second = index::index_session(&adapter, "").unwrap();
    assert_eq!(
        first.doc_count, second.doc_count,
        "re-index must not grow docs"
    );
}

#[test]
fn test_search_finds_content_term() {
    let (_dir, adapter) = write_fixture("content", &fixture_lines());
    index::index_session(&adapter, "").unwrap();
    let report = index::search(&adapter, &[], "alpha", 0, None).unwrap();
    assert!(report.contains("mock |"), "{}", report);
    assert!(report.contains("USER"), "{}", report);
    assert!(report.contains("2026-09-19T10:00:00Z"), "{}", report);
    assert!(
        report.contains("please review the alpha query engine"),
        "{}",
        report
    );
}

#[test]
fn test_search_finds_thinking_only_term_and_marks_it() {
    let (_dir, adapter) = write_fixture("thinking", &fixture_lines());
    index::index_session(&adapter, "").unwrap();
    let report = index::search(&adapter, &[], "contemplating", 0, None).unwrap();
    assert!(
        report.contains("contemplating the electric sheep dream"),
        "{}",
        report
    );
    assert!(
        report.contains("ASSISTANT (thinking)"),
        "thinking hit must carry the thinking marker: {}",
        report
    );
}

#[test]
fn test_search_reports_unindexed_sessions() {
    let (_dir, adapter) = write_fixture("unindexed", &fixture_lines());
    let report = index::search(&adapter, &[], "alpha", 0, None).unwrap();
    assert!(
        report.contains("not indexed (run index first)"),
        "{}",
        report
    );
    assert!(report.contains("1 not indexed"), "{}", report);
}

#[test]
fn test_search_empty_query_is_error() {
    let (_dir, adapter) = write_fixture("empty_query", &fixture_lines());
    assert!(index::search(&adapter, &[], "", 0, None).is_err());
    assert!(index::search(&adapter, &[], "  ", 0, None).is_err());
}

#[test]
fn test_has_tantivy_index_flips_after_indexing() {
    let (_dir, adapter) = write_fixture("flag_flip", &fixture_lines());
    let mut sessions = adapter.list_sessions();
    assert!(!sessions[0].has_tantivy_index);
    let mut profile = adapter.profile_session("mock");
    assert!(!profile.has_tantivy_index);

    index::index_session(&adapter, "").unwrap();
    index::annotate_sessions(&mut sessions, &adapter);
    assert!(sessions[0].has_tantivy_index);
    assert!(index::index_exists(&adapter, &sessions[0].session_id));
    profile.has_tantivy_index = index::index_exists(&adapter, &profile.session_id);
    assert!(profile.has_tantivy_index);
}

#[test]
fn test_explicit_partial_id_resolves_and_unmatched_reported() {
    let (_dir, adapter) = write_fixture("partial_id", &fixture_lines());
    index::index_session(&adapter, "mock").unwrap();
    let report = index::search(&adapter, &w(&["mock", "nope"]), "alpha", 0, None).unwrap();
    assert!(
        report.contains("1 unmatched session ids: nope"),
        "{}",
        report
    );
    assert!(report.contains("alpha"), "{}", report);
}
