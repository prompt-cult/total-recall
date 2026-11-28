//! Unit tests for the law 1/2 additive trait extensions (ticket 805): the
//! five new `RolloutAdapter` methods and their defaults. These are PURE
//! ADDITIVE seams — no existing method is touched, and no live code path uses
//! them yet. The mock adapter is the fixture carrier; a test-local wrapper
//! overrides the two law-2 seams (`payload_stamp`, `resolve_location`) to
//! exercise known-vs-unknown behavior without touching the mock itself.

mod common;

use std::path::{Path, PathBuf};

use total_recall::rollout::ReadResult;
use total_recall::rollout::{BackendKind, SessionMapScope};
use total_recall::{MockAdapter, RolloutAdapter, SessionListing, SessionProfile, SessionSummary};

use common::scratch::scratch;

fn test_data_path() -> String {
    env!("CARGO_MANIFEST_DIR").to_string() + "/rollouts/mock_sample.jsonl"
}

/// The mock's first message timestamp is `2026-09-10T10:00:00Z` — 1,789,034,400
/// seconds after the Unix epoch (verified against the civil-days calendar).
const FIRST_MSG_EPOCH_SECS: i64 = 1_789_034_400;

/// A test-local wrapper that delegates everything to [`MockAdapter`] and
/// overrides the two law-2 seams: `payload_stamp` reports a known stamp for
/// the mock session and `None` otherwise; `resolve_location` resolves the
/// mock session (partial match, empty = most recent) and errors on anything
/// else — standing in for what a native harness implementation will do.
struct StampAndLocationAdapter {
    inner: MockAdapter,
    store_path: PathBuf,
}

impl StampAndLocationAdapter {
    fn new<P: Into<PathBuf>>(data_path: P) -> Self {
        let store_path = data_path.into();
        Self {
            inner: MockAdapter::new(store_path.clone()),
            store_path,
        }
    }
}

impl RolloutAdapter for StampAndLocationAdapter {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn list_sessions(&self) -> Vec<SessionSummary> {
        self.inner.list_sessions()
    }

    fn list_sessions_scoped(&self, hours_back: u64, directory: Option<&str>) -> SessionListing {
        self.inner.list_sessions_scoped(hours_back, directory)
    }

    fn read_session(&self, session_id: &str) -> ReadResult<Vec<total_recall::RolloutMessage>> {
        self.inner.read_session(session_id)
    }

    fn read_session_mmap(&self, session_id: &str) -> ReadResult<Vec<total_recall::RolloutMessage>> {
        self.inner.read_session_mmap(session_id)
    }

    fn read_session_from_compaction(
        &self,
        session_id: &str,
    ) -> ReadResult<Vec<total_recall::RolloutMessage>> {
        self.inner.read_session_from_compaction(session_id)
    }

    fn profile_session_opts(&self, session_id: &str, cache: bool) -> ReadResult<SessionProfile> {
        self.inner.profile_session_opts(session_id, cache)
    }

    fn extract_user_messages(&self, session_id: &str) -> ReadResult<Vec<String>> {
        self.inner.extract_user_messages(session_id)
    }

    fn resolve_canonical_id(&self, session_id: &str) -> Option<String> {
        self.inner.resolve_canonical_id(session_id)
    }

    fn shadow_index_root(&self) -> PathBuf {
        self.inner.shadow_index_root()
    }

    fn payload_stamp(&self, session_id: &str) -> Option<i64> {
        match session_id {
            "mock" | "test" => Some(FIRST_MSG_EPOCH_SECS),
            _ => None,
        }
    }

    fn resolve_location(&self, session_id: &str) -> ReadResult<(String, PathBuf)> {
        if session_id.is_empty() || "mock".contains(session_id) {
            Ok(("mock".to_string(), self.store_path.clone()))
        } else {
            Err(format!(
                "no session matching '{}' under {}",
                session_id,
                self.store_path.display()
            ))
        }
    }
}

/// `session_map` renders the live store as lean rows: canonical id, folder,
/// seconds-since-epoch timestamps and backend kind — no cache-derived
/// marker fields, no presentation weight. The default bridges the scoped
/// listing, so the mock's single session appears exactly once, uncapped in
/// shape.
#[test]
fn session_map_shape_is_lean_and_live() {
    let adapter = MockAdapter::new(test_data_path());
    let map = adapter.session_map(&SessionMapScope::default());

    assert_eq!(map.len(), 1, "the mock store holds one session");
    let entry = &map[0];
    assert_eq!(entry.canonical_id, "mock");
    assert!(
        entry.folder.is_none(),
        "the mock listing carries no directory — folder stays None (unknown ⇒ absent)"
    );
    assert!(
        entry.repo.is_none(),
        "no repo notion exists at the bridge layer"
    );
    assert!(
        entry.start.is_none() && entry.end.is_none(),
        "the mock summary carries no start/end strings — unknown ⇒ absent"
    );
    assert!(
        entry.last_modified.is_none(),
        "the mock listing carries no ctime/mtime — last_modified stays None (unknown ⇒ absent)"
    );
    assert_eq!(entry.backend_kind, BackendKind::File);
}

/// The default `session_map` bridges `list_sessions_scoped`, so the scope
/// (recency window) is honored: a payload aged out of the window disappears
/// from the map while the unbounded form still sees it — live, never cached.
#[test]
fn session_map_scope_bridges_to_scoped_listing() {
    let dir = scratch("session_map_scope");
    let path = dir.join("mock.jsonl");
    std::fs::write(
        &path,
        concat!(
            "{\"role\":\"user\",\"content\":\"hello\",\"tool_calls_summary\":[],\"timestamp\":null,\"injected\":false}\n",
            "{\"role\":\"assistant\",\"content\":\"hi back\",\"tool_calls_summary\":[],\"timestamp\":null,\"injected\":false}\n",
        ),
    )
    .unwrap();
    let adapter = MockAdapter::new(&path);

    // A fresh payload is inside a 24-hour window, and the unbounded form
    // sees it regardless.
    assert_eq!(adapter.session_map(&SessionMapScope::default()).len(), 1);
    assert_eq!(
        adapter
            .session_map(&SessionMapScope {
                hours_back: Some(24),
                directory: None,
            })
            .len(),
        1
    );

    // Age the payload three days: the window hides it, the unbounded form
    // still sees it.
    let three_days_ago =
        std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(three_days_ago)
        .unwrap();
    assert!(
        adapter
            .session_map(&SessionMapScope {
                hours_back: Some(24),
                directory: None,
            })
            .is_empty(),
        "the window hides an aged payload"
    );
    assert_eq!(adapter.session_map(&SessionMapScope::default()).len(), 1);
}

/// Law 2: unknown ⇒ treat as absent. The default returns `None` for every
/// session — a harness that has not implemented `payload_stamp` yet never
/// fabricates a stamp.
#[test]
fn payload_stamp_default_is_absent_for_unknown() {
    let adapter = MockAdapter::new(test_data_path());
    assert_eq!(adapter.payload_stamp("test"), None);
    assert_eq!(adapter.payload_stamp("nope"), None);
}

/// A harness that implements `payload_stamp` reports a known stamp for a
/// known session and `None` for an unknown one.
#[test]
fn payload_stamp_known_vs_unknown() {
    let adapter = StampAndLocationAdapter::new(test_data_path());
    assert_eq!(adapter.payload_stamp("test"), Some(FIRST_MSG_EPOCH_SECS));
    assert_eq!(adapter.payload_stamp("nope"), None);
}

/// The default `resolve_location` refuses cleanly: not implemented on this
/// harness, never a silent guess.
#[test]
fn resolve_location_default_errors_not_implemented() {
    let adapter = MockAdapter::new(test_data_path());
    let err = adapter.resolve_location("anything").unwrap_err();
    assert!(err.contains("not implemented"), "unexpected error: {err}");
}

/// A harness that implements `resolve_location` resolves a known (even
/// partial) session to its canonical id + store path, and errors on an
/// unknown session.
#[test]
fn resolve_location_resolves_known_and_errors_unknown() {
    let adapter = StampAndLocationAdapter::new(test_data_path());

    let (canonical, path) = adapter.resolve_location("moc").unwrap();
    assert_eq!(canonical, "mock");
    assert_eq!(path, PathBuf::from(test_data_path()));

    let err = adapter.resolve_location("missing").unwrap_err();
    assert!(
        err.contains("no session matching"),
        "unexpected error: {err}"
    );
}

/// `record_stream` keys are stable: the same input always yields the same
/// records, with `record_key` = the index string and `seq` dense from 0. The
/// default derives one record per normalized message, kind mirroring role,
/// and parses ISO8601 timestamps to epoch seconds.
#[test]
fn record_stream_record_keys_are_stable() {
    let adapter = MockAdapter::new(test_data_path());
    let messages = adapter.read_session("test").unwrap();

    let first = adapter.record_stream("test").unwrap();
    let second = adapter.record_stream("test").unwrap();
    assert_eq!(first, second, "same input ⇒ identical record stream");

    assert_eq!(
        first.len(),
        messages.len(),
        "one record per normalized message"
    );
    for (i, rec) in first.iter().enumerate() {
        assert_eq!(rec.seq, i as u64);
        assert_eq!(rec.record_key, i.to_string(), "record_key = index string");
        assert_eq!(rec.role, messages[i].role);
        assert_eq!(
            rec.kind, messages[i].role,
            "kind mirrors the normalized role"
        );
    }

    assert_eq!(first[0].ts, Some(FIRST_MSG_EPOCH_SECS));
}

/// `intermediate_root` mirrors the `shadow_index_root` pattern: a sibling of
/// the store root's parent — same parent directory as the shadow root, a
/// distinct name, never the payload itself.
#[test]
fn intermediate_root_sibling_placement() {
    let adapter = MockAdapter::new(test_data_path());
    let shadow = adapter.shadow_index_root();
    let intermediate = adapter.intermediate_root();

    assert_ne!(
        intermediate, shadow,
        "distinct from the tantivy shadow root"
    );
    assert_eq!(
        intermediate.parent(),
        shadow.parent(),
        "siblings under the same parent (mirrors shadow_index_root)"
    );
    assert_eq!(
        intermediate.file_name().and_then(|s| s.to_str()),
        Some(".tr_intermediates")
    );
    assert!(
        !intermediate.starts_with(Path::new(&test_data_path())),
        "never the payload path itself"
    );
}
