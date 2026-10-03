//! #18: every MCP params struct's `session_id` promises "Empty = most
//! recent", so omitting the field must deserialize to the empty string, not
//! fail with "missing field `session_id`". Written RED.

use total_recall::mcp::{
    CompactParams, ExtractByTypeParams, ExtractParams, ProfileParams, TotalRecallParams,
    UserMessagesParams,
};

fn de<T: serde::de::DeserializeOwned>(body: &str) -> T {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("deserialize failed: {e}"))
}

#[test]
fn an_omitted_session_id_defaults_to_empty_everywhere() {
    let p: ProfileParams = de("{}");
    assert_eq!(p.session_id, "");
    let p: ExtractParams = de("{}");
    assert_eq!(p.session_id, "");
    let p: UserMessagesParams = de("{}");
    assert_eq!(p.session_id, "");
    let p: ExtractByTypeParams = de("{}");
    assert_eq!(p.session_id, "");
    let p: CompactParams = de("{}");
    assert_eq!(p.session_id, "");
    // The #18 reproduction: hours_back alone, session_id omitted.
    let p: TotalRecallParams = de(r#"{"hours_back": 72}"#);
    assert_eq!(p.session_id, "");
    assert_eq!(p.hours_back, 72);
}

#[test]
fn an_explicit_empty_session_id_still_selects_most_recent() {
    let p: TotalRecallParams = de(r#"{"session_id": ""}"#);
    assert_eq!(p.session_id, "");
}
