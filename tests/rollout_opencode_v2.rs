mod common;

use rusqlite::Connection;
use std::path::Path;
use total_recall::RolloutAdapter;
use total_recall::rollout::opencode::OpenCodeAdapter;

use common::scratch::{ScratchRoot, scratch};

/// A session id in the v2-only set (2.2 writes new sessions to session_v2 +
/// session_message; the legacy session/message/part tables never see them).
const V2_SESSION: &str = "ses_e7ad8c698ffeh3kWIl6wrgGpVS20";
const OLD_SESSION: &str = "ses_legacy000000000000x1";
const PARENT: &str = "ses_parent0000000000zz22";

fn ms(hours_ago: i64) -> i64 {
    // Live wall clock minus the shift; hour shifts control the --hours
    // windows: 0 = recent, 30 = 30 hours old.
    let now: i64 = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    now - hours_ago * 3_600_000
}

fn create_v2_fixture(path: &Path) {
    let conn = Connection::open(path).unwrap();
    // The 2.2 schema, verbatim subset: the columns the adapter reads.
    conn.execute_batch(
        "CREATE TABLE `session_v2` (
            `id` text PRIMARY KEY,
            `project_id` text NOT NULL,
            `parent_id` text,
            `slug` text NOT NULL,
            `directory` text NOT NULL,
            `title` text,
            `version` text NOT NULL,
            `time_created` integer NOT NULL,
            `time_updated` integer NOT NULL
        );
        CREATE TABLE `session_message` (
            `id` text PRIMARY KEY,
            `session_id` text NOT NULL,
            `type` text NOT NULL,
            `seq` integer NOT NULL,
            `time_created` integer NOT NULL,
            `time_updated` integer NOT NULL,
            `data` text NOT NULL
        );
        CREATE TABLE `session` (
            id text PRIMARY KEY,
            parent_id text,
            directory text,
            title text,
            time_created integer NOT NULL,
            time_updated integer NOT NULL
        );
        CREATE TABLE `message` (
            id text PRIMARY KEY,
            session_id text NOT NULL,
            time_created integer NOT NULL,
            time_updated integer NOT NULL,
            data text NOT NULL
        );
        CREATE TABLE `part` (
            id text PRIMARY KEY,
            message_id text NOT NULL,
            session_id text NOT NULL,
            time_created integer NOT NULL,
            time_updated integer NOT NULL,
            data text NOT NULL
        );
        CREATE TABLE todo (
            session_id text NOT NULL,
            data text NOT NULL
        );",
    )
    .unwrap();

    let project = "prj_fixture";
    // The v1 legacy row for a pre-2.2 session, migrated into the v2
    // registry exactly as 2.2 does (the registry is the superset).
    conn.execute(
        "INSERT INTO session (id, parent_id, directory, title, time_created, time_updated)
         VALUES (?1, NULL, ?2, ?3, 1000, 4000)",
        rusqlite::params![OLD_SESSION, "/Users/dev/legacy", "legacy session"],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, title, version, time_created, time_updated)
         VALUES (?1, ?2, NULL, 'legacy-slug', '/Users/dev/legacy', 'legacy session', '2.2.0', 1000, 4000)",
        rusqlite::params![OLD_SESSION, project],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
         VALUES (?1, ?2, 'user', 1, 4000, 4000, ?3)",
        rusqlite::params![
            "sml_1",
            OLD_SESSION,
            r#"{"text":"legacy turn","time":{"created":4000},"files":[],"agents":[]}"#
        ],
    )
    .unwrap();

    // v2 registry: a recent session (updated 1h ago) and a 30h-old parent.
    conn.execute(
        "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, title, version, time_created, time_updated)
         VALUES (?1, ?2, ?3, 'v2-session', '/Users/dev/fixture', 'v2 fixture session', '2.2.0', ?4, ?5)",
        rusqlite::params![V2_SESSION, project, PARENT, ms(2), ms(1)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, title, version, time_created, time_updated)
         VALUES (?1, ?2, NULL, 'parent-slug', '/Users/dev/fixture', 'parent session', '2.2.0', ?3, ?4)",
        rusqlite::params![PARENT, project, ms(40), ms(30)],
    )
    .unwrap();

    // session_message rows for the recent v2 session, ordered by seq.
    // user: {text, time.created}; assistant: {content:[...], time.created}.
    let now = ms(0);
    let user_json =
        r#"{"text":"please summarise me","time":{"created":@NOW@},"files":[],"agents":[]}"#;
    let assistant_one = r#"{"agent":"plan","model":{"id":"m","providerID":"p"},"time":{"created":@NOW@},"content":[
        {"type":"reasoning","text":"thinking about it","time":{"created":@NOW@}},
        {"type":"tool","name":"todowrite","state":{"status":"completed","input":{"todos":[{"content":"item one","priority":"high","status":"pending"}]},"metadata":{},"output":"todo"},"time":{"created":@NOW@}},
        {"type":"text","text":"here is the answer","time":{"created":@NOW@}}
    ]}"#;
    let assistant_two = r#"{"agent":"plan","model":{"id":"m","providerID":"p"},"time":{"created":@NOW@},"content":[
        {"type":"tool","name":"shell","state":{"status":"completed","input":{"command":"cargo test"},"metadata":{},"output":"test result: ok"},"time":{"created":@NOW@}},
        {"type":"text","text":"cargo checklist done","time":{"created":@NOW@}}
    ]}"#;
    let compaction = r#"{"status":"completed","reason":"manual","summary":"compressed"}"#;
    let user_two = r#"{"text":"after compaction","time":{"created":@NOW@},"files":[],"agents":[]}"#;
    let qa = r#"{"agent":"plan","model":{"id":"m","providerID":"p"},"time":{"created":@NOW@},"content":[
        {"type":"tool","name":"question","state":{"status":"completed",
            "metadata":{"answers":[["dual"]]},
            "input":{"questions":[{"question":"Which layout?","header":"Plan","options":[{"label":"v2 only"},{"label":"dual"}]}]}},
            "time":{"created":@NOW@}}
    ]}"#;

    let rows: Vec<(&str, &str, &str)> = vec![
        ("1", "user", user_json),
        ("2", "assistant", assistant_one),
        ("3", "assistant", assistant_two),
        ("4", "compaction", compaction),
        ("5", "user", user_two),
        ("6", "assistant", qa),
    ];
    for (seq, r#type, data) in rows {
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
            rusqlite::params![
                format!("smo_{seq}"),
                V2_SESSION,
                r#type,
                seq.parse::<i64>().unwrap(),
                now,
                data.replace("@NOW@", &now.to_string())
            ],
        )
        .unwrap();
    }
}

fn v2_fixture(tag: &str) -> (ScratchRoot, OpenCodeAdapter) {
    let root = scratch(tag);
    let db = root.join("opencode.db");
    create_v2_fixture(&db);
    let adapter = OpenCodeAdapter::with_root(&db);
    (root, adapter)
}

#[test]
fn v2_partial_id_resolves_and_reads() {
    let (_r, ad) = v2_fixture("ocv2_resolve");
    // Issue evidence: profile --session <real 2.2 session> said "no session
    // matching". Resolve by a distinctive partial id and read the messages.
    let full = ad
        .resolve_canonical_id("kWIl6wrgGpVS")
        .expect("v2 session must resolve via partial id");
    assert_eq!(full, V2_SESSION);

    let messages = ad.read_session(V2_SESSION).expect("v2 read");
    // Each inlined part renders as its own message, exactly like the legacy
    // layout's part rows: user text, reasoning, tool call + output pairs,
    // prose, the compaction marker and the post-compaction user turn.
    assert_eq!(messages.len(), 12, "messages: {messages:?}");
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "please summarise me");
    assert_eq!(messages[1].thinking.as_deref(), Some("thinking about it"));
    assert_eq!(messages[2].tool_calls_summary.len(), 1);
    assert!(messages[2].tool_calls_summary[0].contains("todowrite"));
    assert_eq!(messages[3].role, "tool");
    assert_eq!(messages[3].content, "todo");
    assert_eq!(messages[4].content, "here is the answer");
    assert_eq!(messages[5].role, "assistant");
    assert!(messages[5].tool_calls_summary[0].contains("shell"));
    assert_eq!(messages[6].role, "tool");
    assert_eq!(messages[6].content, "test result: ok");
    // The typed compaction row renders the injected marker at index 8, after
    // the cargo text part: shell call (5), its output (6), prose (7).
    assert_eq!(messages[7].content, "cargo checklist done");
    assert!(
        messages[8].injected,
        "the compaction part must render the injected marker: {messages:?}"
    );
    assert_eq!(messages[8].content, "context compaction");
    assert_eq!(messages[9].content, "after compaction");
    assert!(messages[10].tool_calls_summary[0].contains("question"));
}

#[test]
fn v2_hours_window_sees_recent_sessions() {
    let (_r, ad) = v2_fixture("ocv2_window");
    // Issue evidence: list --hours 24 showed nothing after the 2.2 restart.
    // A 24h window over the v2 registry must include the recent session and
    // exclude the 30h-old parent.
    let listing = ad.list_sessions_scoped(24, None);
    assert!(
        listing.sessions.iter().any(|s| s.session_id == V2_SESSION),
        "recent v2 session must appear: {:?}",
        listing.sessions
    );
    assert!(
        listing.sessions.iter().all(|s| s.session_id != PARENT),
        "30h-old session must be outside the 24h window"
    );
    // And the unscoped listing carries both v2 ids.
    let all = ad.list_sessions();
    assert!(all.iter().any(|s| s.session_id == V2_SESSION));
    assert!(all.iter().any(|s| s.session_id == PARENT));
}

#[test]
fn v2_most_recent_session_is_a_v2_row() {
    let (_r, ad) = v2_fixture("ocv2_recent");
    // The v2 session (updated 1h ago) must beat the legacy table's frozen
    // copy as most_recent_session_id.
    let id = ad.most_recent_session_id().expect("recent session");
    assert_eq!(id, V2_SESSION);
}

#[test]
fn v2_legacy_session_still_resolves() {
    let (_r, ad) = v2_fixture("ocv2_legacy");
    // Older sessions migrate into session_v2 too — a legacy id resolves
    // through the v2 registry in this store.
    let full = ad.resolve_canonical_id(OLD_SESSION).expect("legacy id");
    assert_eq!(full, OLD_SESSION);
}

#[test]
fn v2_profile_and_extract() {
    let (_r, ad) = v2_fixture("ocv2_profile");
    let profile = ad.profile_session_opts(V2_SESSION, false).expect("profile");
    assert_eq!(profile.session_id, V2_SESSION);
    assert_eq!(profile.role_counts.get("user"), Some(&3)); // 2 real + the injected compaction marker
    assert!(
        profile.file_size > 0,
        "size must come from the v2 log, not the frozen legacy copy"
    );
    assert!(
        profile.line_count > 0,
        "lines must come from the v2 content arrays"
    );
    assert!(
        profile.first_ts.is_some(),
        "profile timestamps from v2 rows"
    );
    assert!(
        profile
            .interesting_events
            .iter()
            .any(|e| e.summary.contains("please summarise me"))
    );

    let extracted = ad.extract_user_messages(V2_SESSION).expect("extract");
    assert_eq!(
        extracted,
        vec![
            "please summarise me".to_string(),
            "after compaction".to_string()
        ]
    );
}

#[test]
fn v2_todo_writes_and_compaction_slicing() {
    let (_r, ad) = v2_fixture("ocv2_todo");
    let writes = ad.read_todo_writes(V2_SESSION, true).expect("todos full");
    assert_eq!(writes.len(), 1, "one todowrite flush: {writes:?}");
    assert_eq!(writes[0].todos.len(), 1);
    assert_eq!(writes[0].todos[0].content, "item one");

    let sliced = ad
        .read_todo_writes(V2_SESSION, false)
        .expect("todos post-compaction");
    assert!(
        sliced.is_empty(),
        "the flush predates the compaction marker"
    );

    let post = ad
        .read_session_from_compaction(V2_SESSION)
        .expect("sliced session");
    assert!(post.iter().any(|m| m.content == "after compaction"));
    assert!(!post.iter().any(|m| m.content == "here is the answer"));
}

#[test]
fn v2_qa_pairs_parse_question_tool() {
    let (_r, ad) = v2_fixture("ocv2_qa");
    let pairs = ad.read_qa_pairs(V2_SESSION).expect("qa");
    assert_eq!(pairs.len(), 1, "pairs: {pairs:?}");
    assert_eq!(pairs[0].question, "Which layout?");
    assert_eq!(pairs[0].header.as_deref(), Some("Plan"));
    assert_eq!(pairs[0].options.len(), 2);
}

#[test]
fn v2_store_she_said_finds_v2_hits() {
    let (_r, ad) = v2_fixture("ocv2_shesaid");
    let report = ad
        .she_said_he_said_action(
            &[],
            &["summarise".to_string(), "cargo".to_string()],
            24,
            None,
        )
        .expect("report");
    assert!(report.contains("HE SAID"), "user hit: {report}");
    assert!(report.contains("SHE SAID"), "assistant hit: {report}");
    assert!(report.contains("THEY DID"), "tool hit: {report}");
}
