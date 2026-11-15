//! Issue #45: vibe 3.0.3 replaced its rollout layout — sessions moved from
//! `session_<ts>/` dirs to `unified/<uuid>/` stores with a hash-chained
//! journal (`CURRENT` pointer, `journal/NNN.jsonl` segments,
//! `chunks/<sha256>.json` payloads). The adapter must read both layouts.

mod common;

use total_recall::RolloutAdapter;
use total_recall::rollout::vibe::VibeAdapter;

const UUID: &str = "01670e46-2e0a-192a-44dd-6e4f5f43dc9b";

/// A vibe 3.0.3 unified-session store under the adapter root: `CURRENT`
/// points at the session id, `meta.json` carries title/times, the
/// hash-chained `journal/*.jsonl` segments carry the dialogue (`core_input`
/// = user turn, `action_result` = assistant output; verified against a live
/// 3.0.3 store).
fn create_unified_store(root: &std::path::Path, uuid: &str) {
    let dir = root.join("unified").join(uuid);
    std::fs::create_dir_all(dir.join("journal")).unwrap();

    std::fs::write(
        dir.join("CURRENT"),
        r#"{"generation":"0000000000000520","manifest_sha256":"606b121e","session_id":"<uuid>","snapshot_sequence":520}"#,
    )
    .unwrap();

    std::fs::write(
        dir.join("meta.json"),
        r#"{"title":"unified fixture","start_time":"2026-09-28T15:55:59.040000+00:00","end_time":"2026-09-28T15:59:59.535000+00:00","environment":{"working_directory":"/fixture/cwd"},"parent_session_id":null}"#,
    )
    .unwrap();

    // Journal segment: one user turn (core_input, with the live record shape)
    // and one assistant output (action_result with text + reasoning + a
    // tool_call part, as observed live). Record types the dialogue has no
    // read for (command_reserved, action_intent) are included on purpose:
    // they must contribute nothing.
    let seg = concat!(
        r#"{"type":"command_reserved","payload":{"method":"turn/start","params":{"message":[{"text":"please summarize me","type":"text"}]}}}"#,
        "\n",
        r#"{"type":"action_intent","payload":{"request":{"model_input":{"messages":{"messages":[{"role":"user","content":[{"text":"please summarize me","type":"text"}]}]}}}}}"#,
        "\n",
        r#"{"type":"core_input","payload":{"input":{"command":{"type":"user_message","mode":"queue","content":[{"text":"please summarize me","type":"text"}]},"determinism":{"random_seed":7,"time_unix_ms":1759072559040},"input_id":1,"protocol_version":1}}}"#,
        "\n",
        r#"{"type":"action_result","payload":{"action_id":"a1","state":"succeeded","result":{"result":{"finish_reason":"tool_call","parts":[{"_meta":{"closed":true},"content":[{"text":"thinking about it","type":"text"}],"type":"reasoning"},{"type":"tool_call","id":"chatcmpl-to-t1","name":"todowrite","arguments_json":"{\"todos\":[{\"content\":\"item one\"}]}"}],"usage":{}},"type":"assistant"},"sequence":9}}"#,
        "\n",
        r#"{"type":"action_result","payload":{"action_id":"a2","state":"succeeded","result":{"result":{"finish_reason":"stop","parts":[{"type":"text","text":"here is the answer"}],"usage":{}},"type":"assistant"},"sequence":10}}"#,
        "\n",
        r#"{"type":"receipt_succeeded","payload":{"client_command_id":"x","response":{}}}"#,
        "\n",
        r#"{"type":"projection_delta","payload":{"delta":{},"watermark":1}}"#,
        "\n",
    );
    std::fs::write(dir.join("journal").join("0000000000000183.jsonl"), seg).unwrap();
}

#[test]
fn unified_session_is_listed_with_meta() {
    let root = common::scratch::scratch("vibe_unified_list");
    create_unified_store(&root, UUID);
    let ad = VibeAdapter::with_root(&*root);
    let sessions = ad.list_sessions();
    assert!(
        sessions
            .iter()
            .any(|s| s.session_id == UUID && s.title == "unified fixture"),
        "unified session must appear with meta title: {sessions:?}"
    );
    // meta.json timestamps surface on the listing, same as legacy sessions.
    let s = sessions.iter().find(|s| s.session_id == UUID).unwrap();
    assert_eq!(s.start_time, "2026-09-28T15:55:59.040000+00:00");
    assert_eq!(s.end_time, "2026-09-28T15:59:59.535000+00:00");
    assert_eq!(s.directory.as_deref(), Some("/fixture/cwd"));

    // Scoped listing (mtime of the journal segments) sees it too.
    let scoped = ad.list_sessions_scoped(24, None);
    assert!(
        scoped.sessions.iter().any(|s| s.session_id == UUID),
        "scoped listing must include the unified session: {:?}",
        scoped.sessions
    );
}

#[test]
fn unified_read_returns_dialogue() {
    let root = common::scratch::scratch("vibe_unified_read");
    create_unified_store(&root, UUID);
    let ad = VibeAdapter::with_root(&*root);

    let messages = ad.read_session(UUID).expect("unified read");
    // user turn + assistant tool_call action + assistant text action; the
    // bookkeeping record types (command_reserved, action_intent, receipt,
    // projection_delta) carry nothing.
    assert_eq!(messages.len(), 3, "messages: {messages:?}");
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "please summarize me");
    assert_eq!(
        messages[0].timestamp.as_deref(),
        Some("2025-09-28T15:15:59Z"),
        "core_input determinism time to ISO"
    );

    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "");
    assert_eq!(messages[1].thinking.as_deref(), Some("thinking about it"));
    assert_eq!(messages[1].tool_calls_summary.len(), 1);
    assert!(messages[1].tool_calls_summary[0].contains("todowrite"));

    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[2].content, "here is the answer");

    // extract = the user turns.
    let extracted = ad.extract_user_messages(UUID).expect("extract");
    assert_eq!(extracted, vec!["please summarize me".to_string()]);
}

#[test]
fn unified_profile_reports_real_counters() {
    let root = common::scratch::scratch("vibe_unified_profile");
    create_unified_store(&root, UUID);
    let ad = VibeAdapter::with_root(&*root);

    let profile = ad.profile_session_opts(UUID, false).expect("profile");
    assert_eq!(profile.session_id, UUID);
    assert!(profile.file_size > 0, "size from the journal segments");
    assert_eq!(profile.line_count, 3);
    assert_eq!(profile.role_counts.get("user"), Some(&1));
    assert_eq!(profile.role_counts.get("assistant"), Some(&2));
    assert!(
        profile.first_ts.is_some(),
        "timestamps from core_input determinism"
    );
    assert!(
        profile
            .interesting_events
            .iter()
            .any(|e| e.summary.contains("please summarize me")),
        "user message surfaced as an interesting event"
    );

    // Direct partial-id access works too.
    assert_eq!(
        ad.read_session("2e0a-192a").expect("partial id read").len(),
        3
    );
}
