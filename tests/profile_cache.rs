use std::path::{Path, PathBuf};

use total_recall::RolloutAdapter;
use total_recall::rollout::mock::MockAdapter;

#[allow(dead_code, unused_imports)]
mod cli {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"));
}

use clap::Parser;
use cli::Cli;

fn fixture_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("profile_cache_{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn msg(role: &str, content: &str, timestamp: &str) -> String {
    serde_json::json!({
        "role": role,
        "content": content,
        "thinking": Option::<&str>::None,
        "tool_calls_summary": [],
        "injected": false,
        "timestamp": timestamp,
    })
    .to_string()
}

fn fixture_lines() -> Vec<String> {
    vec![
        msg("user", "cache me if you can", "2026-09-24T10:00:00Z"),
        msg("assistant", "cached", "2026-09-24T10:00:01Z"),
        msg("tool", "result", "2026-09-24T10:00:02Z"),
    ]
}

fn write_fixture(name: &str, lines: &[String]) -> (PathBuf, MockAdapter, PathBuf) {
    let dir = fixture_dir(name);
    let data_path = dir.join("data.jsonl");
    std::fs::write(&data_path, lines.join("\n") + "\n").unwrap();
    let adapter = MockAdapter::new(&data_path);
    let cache_path = adapter.shadow_index_root().join("tr_mock_meta.json");
    (dir, adapter, cache_path)
}

fn set_mtime_epoch_secs(path: &Path, epoch_secs: i64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap();
    file.set_times(
        std::fs::FileTimes::new().set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(epoch_secs.max(0) as u64),
        ),
    )
    .unwrap();
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn test_first_call_writes_cache_second_call_serves_it() {
    let (_dir, adapter, cache_path) = write_fixture("hit", &fixture_lines());
    let first = adapter.profile_session_opts("mock", true);
    assert!(cache_path.is_file(), "cache file must be written");

    // Push the source file's mtime into the past: still inside the 15 s
    // tolerance relative to the younger cache file, so the second call must
    // be served from the cache and return the identical profile.
    set_mtime_epoch_secs(&_dir.join("data.jsonl"), now_epoch_secs() - 3_600);
    let second = adapter.profile_session_opts("mock", true);
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap(),
        "cache hit must return the identical profile"
    );
    assert!(cache_path.is_file());
}

#[test]
fn test_stale_cache_recomputes_and_rewrites() {
    let (dir, adapter, cache_path) = write_fixture("stale", &fixture_lines());
    let first = adapter.profile_session_opts("mock", true);
    assert!(cache_path.is_file());

    // Change the payload AND push its mtime beyond the staleness tolerance:
    // the cache must be treated as stale, the profile recomputed from the new
    // content, and the cache rewritten.
    let mut grown = fixture_lines();
    grown.push(msg("user", "after the bump", "2026-09-24T10:00:03Z"));
    std::fs::write(dir.join("data.jsonl"), grown.join("\n") + "\n").unwrap();
    set_mtime_epoch_secs(&dir.join("data.jsonl"), now_epoch_secs() + 120);

    let second = adapter.profile_session_opts("mock", true);
    assert_eq!(
        second.line_count, first.line_count + 1,
        "stale cache must recompute from the changed fixture"
    );
    assert!(
        second.file_size > first.file_size,
        "recomputed profile must reflect the grown fixture"
    );
    assert_eq!(
        serde_json::to_value(&second).unwrap(),
        serde_json::to_value(adapter.profile_session_opts("mock", false)).unwrap(),
        "rewritten cache must match the fresh compute"
    );
}

#[test]
fn test_corrupt_cache_is_repaired() {
    let (_dir, adapter, cache_path) = write_fixture("corrupt", &fixture_lines());
    adapter.profile_session_opts("mock", true);
    assert!(cache_path.is_file());
    std::fs::write(&cache_path, b"\x00not json {{{").unwrap();

    let profile = adapter.profile_session_opts("mock", true);
    assert_eq!(profile.session_id, "mock");
    assert_eq!(profile.line_count, 3);

    let data = std::fs::read(&cache_path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&data)
        .expect("corrupt cache must have been rewritten as valid JSON");
    assert!(value["written_at_epoch_ms"].is_u64());
    assert!(value["profile"]["role_counts"].is_object());
}

#[test]
fn test_structurally_wrong_cache_fails_jtd_gate_and_recomputes() {
    let (_dir, adapter, cache_path) = write_fixture("jtd_gate", &fixture_lines());
    adapter.profile_session_opts("mock", true);

    let data = std::fs::read(&cache_path).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&data).unwrap();
    value["profile"]["role_counts"] = serde_json::json!([]);
    std::fs::write(&cache_path, serde_json::to_vec(&value).unwrap()).unwrap();

    let profile = adapter.profile_session_opts("mock", true);
    assert_eq!(profile.line_count, 3, "wrong-shape cache must recompute");

    let repaired: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache_path).unwrap()).unwrap();
    assert!(
        repaired["profile"]["role_counts"].is_object(),
        "cache must be repaired with the correct shape"
    );
}

#[test]
fn test_flag_off_writes_no_cache() {
    let (_dir, adapter, cache_path) = write_fixture("flag_off", &fixture_lines());
    let _ = adapter.profile_session_opts("mock", false);
    assert!(
        !cache_path.exists(),
        "cache=false must not create a cache file"
    );
    let _ = adapter.profile_session("mock");
    assert!(!cache_path.exists(), "trait default must not cache either");
}

#[test]
fn test_cli_cache_flag_parses_on_profile() {
    let cli = Cli::try_parse_from(["bin", "profile", "--cache"])
        .unwrap_or_else(|e| panic!("clap rejected --cache on profile: {e}"));
    assert!(cli.cache);
    assert!(matches!(cli.command, cli::Command::Profile));

    let cli =
        Cli::try_parse_from(["bin", "profile"]).expect("profile must parse without --cache");
    assert!(!cli.cache, "cache must default to false");
}

#[test]
fn test_freshness_rule_is_enforced_in_profile_cache_module() {
    use total_recall::profile_cache;

    let dir = fixture_dir("module_freshness");
    let cache_path = dir.join("tr_mock_meta.json");

    // Missing cache file → miss.
    assert!(profile_cache::read_fresh(&cache_path, 0).is_none());

    // Write a cache whose mtime is now; a source time far in the past is
    // fresh, a source time beyond the tolerance is stale.
    let profile = total_recall::SessionProfile {
        session_id: "mock".to_string(),
        file_size: 1,
        line_count: 1,
        first_ts: None,
        last_ts: None,
        role_counts: Default::default(),
        has_tantivy_index: false,
        interesting_events: Vec::new(),
    };
    profile_cache::write(&cache_path, &profile);
    let cache_mtime = profile_cache::mtime_ms(&cache_path).unwrap();
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime).is_some());
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime + 15_000).is_some());
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime + 15_001).is_none());

    // Corrupt bytes → miss. Valid JSON of the wrong variant name → JTD
    // passes (event_type is a plain string) but the unknown variant must fail
    // the conversion into SessionProfile, i.e. treated as corrupt.
    std::fs::write(&cache_path, b"{{{garbage").unwrap();
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime).is_none());

    profile_cache::write(&cache_path, &profile);
    let mut valid: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cache_path).unwrap()).unwrap();
    valid["profile"]["interesting_events"] = serde_json::json!([{
        "line_number": 1,
        "event_type": "NotARealEvent",
        "summary": "s",
        "gap_lines": 1
    }]);
    std::fs::write(&cache_path, serde_json::to_vec(&valid).unwrap()).unwrap();
    assert!(profile_cache::read_fresh(&cache_path, cache_mtime).is_none());
}
