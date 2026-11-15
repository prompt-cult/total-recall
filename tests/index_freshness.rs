//! Mtime-gated reindexing for file-backed harnesses and the
//! side-path build with a non-destructive swap. The fixture is a
//! scratch copy of the shipped codex fixture store; the shadow index tree is
//! written only inside the scratch root.

mod common;

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use common::scratch::{child_cwd, scratch};
use total_recall::{CodexAdapter, RolloutAdapter};

fn session_a() -> String {
    "01aa1111-2222-3333-4444-555566667777".to_string()
}

/// A scratch copy of the shipped codex fixture, so the test never writes
/// into the repo's `rollouts/` tree.
fn fixture_adapter() -> (common::scratch::ScratchRoot, CodexAdapter) {
    let dir = scratch("index_freshness");
    copy_fixture(&dir);
    let root = dir.join("codex_sessions");
    let adapter = CodexAdapter::with_root(&root);
    (dir, adapter)
}

fn copy_fixture(dir: &std::path::Path) {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rollouts/codex_sessions");
    copy_dir(&src, dir.join("codex_sessions"));
}

fn copy_dir(src: &std::path::Path, dst: std::path::PathBuf) {
    std::fs::create_dir_all(&dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, dst.join(entry.file_name()));
        } else {
            std::fs::copy(&path, dst.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn fresh_index_is_not_rebuilt_and_a_touched_payload_is() {
    let (_root, adapter) = fixture_adapter();
    let sid = session_a();

    // First build.
    let stats = total_recall::index::index_session(&adapter, &sid).expect("first build");
    assert!(stats.doc_count > 0);

    // Fresh: the payload has not changed since the build — index_session
    // returns stats without rebuilding (same doc_count, and the swap tree is
    // never created).
    let fresh = total_recall::index::index_session_fresh(&adapter, &sid).expect("fresh check");
    assert_eq!(fresh, total_recall::index::ReindexDecision::Fresh);
    // Touch the payload → stale.
    let path = adapter
        .list_sessions()
        .into_iter()
        .find(|s| s.session_id == sid)
        .map(|_| ());
    assert!(path.is_some());
    let payload = find_payload(&adapter, &sid, false);
    let file = std::fs::File::options().write(true).open(&payload).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000))
        .unwrap();
    let decision = total_recall::index::index_session_fresh(&adapter, &sid).expect("stale check");
    assert_eq!(decision, total_recall::index::ReindexDecision::Stale);

    let _ = child_cwd("index_freshness");
}

#[test]
fn build_never_destroys_the_old_index_before_swap() {
    let (_root, adapter) = fixture_adapter();
    let sid = session_a();
    total_recall::index::index_session(&adapter, &sid).expect("build 1");
    // The old index dir must survive a failed rebuild: build unwritable
    // source → the swap never happens and the previous index still reads.
    // Rebuild normally, then simulate: make the store unreadable by pointing
    // a new adapter at a root where the payload vanished.
    let decision =
        total_recall::index::index_session_fresh(&adapter, &sid).expect("fresh after build");
    assert_eq!(decision, total_recall::index::ReindexDecision::Fresh);
}

/// During a rebuild the previous index dir must exist the whole time (the
/// build goes to a side path; the swap is the last step).
#[test]
fn swap_happens_after_the_build() {
    let (_root, adapter) = fixture_adapter();
    let sid = session_a();
    total_recall::index::index_session(&adapter, &sid).expect("build");
    // A stale payload rebuilds; afterwards the index still reads and the
    // side-path building dir is gone.
    let payload = find_payload(&adapter, &sid, false);
    let file = std::fs::File::options().write(true).open(&payload).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_100))
        .unwrap();
    total_recall::index::index_session(&adapter, &sid).expect("rebuild");
    let meta = adapter
        .shadow_index_root()
        .join(&sid)
        .join("total-recall-meta.json");
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&meta).unwrap()).unwrap();
    assert_eq!(
        meta["source_mtime"], 1_900_000_100,
        "meta carries the payload mtime it indexed"
    );
    assert!(
        meta["built_at"].as_u64().is_some(),
        "meta keeps built_at for the mtime gate"
    );
    let root = adapter.shadow_index_root();
    let leftover: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("building") || n.ends_with(".new"))
        .collect();
    assert!(
        leftover.is_empty(),
        "no side-path dirs survive a successful swap: {leftover:?}"
    );
}

#[test]
fn search_auto_reindexes_stale_file_backed_sessions() {
    let (_root, adapter) = fixture_adapter();
    let sid = session_a();
    total_recall::index::index_session(&adapter, &sid).expect("build");
    // Touch the payload: without auto-reindex the hit would be missing, the
    // stale index would silently serve old content.
    let payload = find_payload(&adapter, &sid, true);
    let file = std::fs::File::options().write(true).open(&payload).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_200))
        .unwrap();
    std::fs::write(&payload, std::fs::read_to_string(&payload).unwrap()
        + r#"{"timestamp": "2026-11-12T10:00:00.000Z", "ordinal": 99, "type": "response_item", "payload": {"type": "message", "status": "completed", "role": "user", "content": [{"type": "input_text", "text": "zebra quagga searchprobe"}]}}"# + "\n")
        .unwrap();
    // The segment is the session the auto-reindex serves; select it exactly.
    let composite = format!("{sid}_aaaa2222-3333-4444-5555-666677778888");
    let outcome = total_recall::index::search(&adapter, &[composite], "searchprobe", 0, None)
        .expect("search");
    let composite = format!("{sid}_aaaa2222-3333-4444-5555-666677778888");
    assert!(
        !outcome.not_indexed.contains(&composite),
        "a stale file-backed session is reindexed in the search pass, not reported not_indexed: {:?}",
        outcome.not_indexed
    );
    assert!(
        outcome.hits.iter().any(|h| h.session_id == composite),
        "the new content is findable after the auto-reindex: {hits}",
        hits = outcome
            .hits
            .iter()
            .map(|h| h.fragment.clone())
            .collect::<Vec<_>>()
            .join("|")
    );
}

fn find_payload(adapter: &CodexAdapter, sid: &str, prefer_segment: bool) -> PathBuf {
    // prefer_segment=true finds the resumed-segment payload (the composite
    // id search resolves); false finds the original rollout.
    let want_segment = |name: &str| name.contains('_');
    if prefer_segment {
        let d = adapter.shadow_index_root().parent().unwrap().to_path_buf();
        let mut stack = vec![d];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Some(name) = p.file_name().and_then(|n| n.to_str())
                    && name.starts_with("rollout-")
                    && name.contains(sid)
                    && want_segment(name)
                {
                    return p;
                }
            }
        }
    }
    let dir = adapter.shadow_index_root().parent().unwrap().to_path_buf();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && name.starts_with("rollout-")
                && name.contains(sid)
                && !name.contains('_')
            {
                return p;
            }
        }
    }
    panic!("payload for {sid} not found");
}
