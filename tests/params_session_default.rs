//! every MCP params struct's `session_id` promises "Empty = most
//! recent", so omitting the field must deserialize to the empty string, not
//! fail with "missing field `session_id`". Written RED.

use total_recall::mcp::{
    CacheDeleteLatestParams, ExtractByTypeParams, ExtractParams, PriorPeriodSummaryParams,
    ProfileParams, SummariseFromParams, SummaryCacheParams, UserMessagesParams,
};

fn de<T: serde::de::DeserializeOwned>(body: &str) -> T {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("deserialize failed: {e}"))
}

#[test]
fn an_omitted_session_id_defaults_to_empty_everywhere() {
    let p: ProfileParams = de("{}");
    assert_eq!(p.session_id, "");
    assert!(p.cache, "the profile cache is on by default");
    let p: ExtractParams = de("{}");
    assert_eq!(p.session_id, "");
    let p: UserMessagesParams = de("{}");
    assert_eq!(p.session_id, "");
    let p: ExtractByTypeParams = de("{}");
    assert_eq!(p.session_id, "");
    // todo_history retired into extract-by-type; its params are gone.
    // hours_back alone, session_id omitted.
    let p: PriorPeriodSummaryParams = de(r#"{"hours_back": 72}"#);
    assert_eq!(p.session_id, "");
    assert_eq!(p.hours_back, 72);
    // the summary cache tools are session tools too.
    let p: SummaryCacheParams = de("{}");
    assert_eq!(p.session_id, "");
    let p: CacheDeleteLatestParams = de("{}");
    assert_eq!(p.session_id, "");
    let p: SummariseFromParams = de(r#"{"from_id": 3}"#);
    assert_eq!(p.session_id, "");
    assert_eq!(p.from_id, 3, "from_id travels with the call");
}

#[test]
fn an_explicit_empty_session_id_still_selects_most_recent() {
    let p: PriorPeriodSummaryParams = de(r#"{"session_id": ""}"#);
    assert_eq!(p.session_id, "");
}
