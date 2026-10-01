//! Secret redaction in the prompt-assembly path (sidecar 458).
//!
//! Every outbound prompt is built from session content a user may have pasted a
//! key into, so the assembly path must strip credential shapes before the
//! vendor ever sees them. The shapes are treated as secrets on sight: the
//! redactor never asks whether a match is a real credential, because that
//! judgement is the one that fails open.
//!
//! Every value in this file is an obviously fake placeholder, not a
//! well-shaped key. Nothing here is or resembles a live credential.

use total_recall::{
    RolloutMessage, build_goals_prompt, build_state_prompt, build_structured_prompt, redact_secrets,
};

/// A message with the given role and content, otherwise empty.
fn msg(role: &str, content: &str) -> RolloutMessage {
    RolloutMessage {
        role: role.to_string(),
        content: content.to_string(),
        thinking: None,
        tool_calls_summary: Vec::new(),
        timestamp: None,
        injected: false,
    }
}

/// Assert `needle` is gone and `marker` is present, naming the case on failure.
#[track_caller]
fn assert_redacted(haystack: &str, needle: &str, marker: &str) {
    assert!(
        !haystack.contains(needle),
        "secret survived redaction: {needle:?}\n--- output ---\n{haystack}"
    );
    assert!(
        haystack.contains(marker),
        "replacement marker {marker:?} missing:\n--- output ---\n{haystack}"
    );
}

// --- Vendor key shapes this tool itself uses ---

#[test]
fn inception_sk_underscore_key_is_redacted() {
    let key = "sk_test0000000000000000000000000000";
    let out = redact_secrets(&format!("export INCEPTION_API_KEY={key}"));
    assert_redacted(&out, key, "[REDACTED:vendor-key]");
}

#[test]
fn openai_sk_hyphen_key_is_redacted() {
    let key = "sk-test00000000000000000000000000000000";
    let out = redact_secrets(&format!("openai says {key} is wrong"));
    assert_redacted(&out, key, "[REDACTED:vendor-key]");
}

#[test]
fn anthropic_sk_ant_key_is_redacted() {
    // Must beat the plain `sk-` rule; the same shape matched by the shorter
    // prefix would still be redacted but would be classed wrong.
    let key = "sk-ant-test0000000000000000000000000";
    let out = redact_secrets(&format!("ANTHROPIC_API_KEY={key}"));
    assert_redacted(&out, key, "[REDACTED:vendor-key]");
}

#[test]
fn tavily_tvly_key_is_redacted() {
    let key = "tvly-test0000000000000000000000";
    let out = redact_secrets(&format!("tavily: {key}"));
    assert_redacted(&out, key, "[REDACTED:vendor-key]");
}

#[test]
fn context7_ctx7sk_key_is_redacted() {
    // `ctx7sk-` contains `sk-`; the longer prefix must win or the tail of the
    // key would be left behind as bare text.
    let key = "ctx7sk-test0000000000000000000000";
    let out = redact_secrets(&format!("ctx7 key {key}"));
    assert_redacted(&out, key, "[REDACTED:vendor-key]");
}

#[test]
fn github_tokens_are_redacted() {
    for key in [
        "ghp_test0000000000000000000000000000",
        "gho_test0000000000000000000000000000",
        "ghu_test0000000000000000000000000000",
        "ghs_test0000000000000000000000000000",
        "ghr_test0000000000000000000000000000",
        "github_pat_test00000000000000000000000000000",
    ] {
        let out = redact_secrets(&format!("git remote uses {key}"));
        assert_redacted(&out, key, "[REDACTED:github-token]");
    }
}

#[test]
fn other_credential_shapes_are_redacted() {
    for key in [
        "AIzaTest0000000000000000000000000",
        "AKIATEST00000000TEST",
        "glpat-test0000000000000000",
        "xoxb-test000000000000000000000",
    ] {
        let out = redact_secrets(&format!("leaked {key} in a thread"));
        assert_redacted(&out, key, "[REDACTED:vendor-key]");
    }
}

#[test]
fn jwt_is_redacted() {
    let jwt = "eyJhbGciOiTest0000.eyJzdWIiOiJ0ZXN0In0.abcdefghijklmnop";
    let out = redact_secrets(&format!("the session carried {jwt}"));
    assert_redacted(&out, jwt, "[REDACTED:jwt]");
}

// --- Header and assignment forms ---

#[test]
fn bearer_header_value_is_redacted() {
    let token = "test0000000000000000000000000000";
    let out = redact_secrets(&format!("curl -H 'Authorization: Bearer {token}'"));
    assert_redacted(&out, token, "[REDACTED:bearer]");
}

#[test]
fn lowercase_bearer_is_redacted() {
    let token = "test0000000000000000000000000000";
    let out = redact_secrets(&format!("authorization: bearer {token}"));
    assert_redacted(&out, token, "[REDACTED:bearer]");
}

#[test]
fn key_equals_value_forms_are_redacted() {
    for (text, needle) in [
        ("api_key=abcdefghijklmnop", "abcdefghijklmnop"),
        ("apikey = abcdefghijklmnop", "abcdefghijklmnop"),
        ("api-key: abcdefghijklmnop", "abcdefghijklmnop"),
        ("INCEPTION_API_KEY=abcdefghijklmnop", "abcdefghijklmnop"),
        ("token: abcdefghijklmnop", "abcdefghijklmnop"),
        ("secret=abcdefghijklmnop", "abcdefghijklmnop"),
        ("password=abcdefghijklmnop", "abcdefghijklmnop"),
        ("passwd=abcdefghijklmnop", "abcdefghijklmnop"),
        ("access_token=abcdefghijklmnop", "abcdefghijklmnop"),
    ] {
        let out = redact_secrets(text);
        assert_redacted(&out, needle, "[REDACTED:secret]");
    }
}

#[test]
fn json_key_value_is_redacted_and_stays_balanced() {
    let json = r#"{"api_key":"abcdefghijklmnop","model":"mercury-2.5"}"#;
    let out = redact_secrets(json);
    assert_redacted(&out, "abcdefghijklmnop", "[REDACTED:secret]");
    assert_eq!(
        out.matches('"').count(),
        json.matches('"').count(),
        "redaction must not disturb the JSON quoting: {out}"
    );
    assert!(
        out.contains(r#""model":"mercury-2.5""#),
        "siblings survive: {out}"
    );
}

#[test]
fn query_string_form_is_redacted() {
    let out = redact_secrets("https://example.test/v1?api_key=abcdefghijklmnop&model=x");
    assert_redacted(&out, "abcdefghijklmnop", "[REDACTED:secret]");
    assert!(
        out.contains("model=x"),
        "sibling params must survive: {out}"
    );
}

// --- PEM blocks ---

#[test]
fn pem_private_key_block_is_redacted() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\n\
               TESTKEYTESTKEYTESTKEYTESTKEYTESTKEYTESTKEYTESTKEYTESTKEY\n\
               -----END RSA PRIVATE KEY-----";
    let out = redact_secrets(&format!("here is the deploy key\n{pem}\ndone"));
    assert_redacted(&out, "TESTKEYTESTKEY", "[REDACTED:pem]");
    assert!(!out.contains("PRIVATE KEY"), "no header may survive: {out}");
    assert!(
        out.contains("here is the deploy key") && out.contains("done"),
        "surrounding prose must survive: {out}"
    );
}

#[test]
fn pem_block_snipped_midway_still_has_its_body_removed() {
    // Tool results are snipped to 500 chars, so a truncated PEM is the common
    // case, not the exotic one: the BEGIN header survives with no END.
    let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
               b3BlbnNzaC1rZXktdjEAAAAA\n\
               TESTKEYTESTKEYTESTKEYTESTK";
    let out = redact_secrets(&format!("cat id_rsa:\n{pem}"));
    assert_redacted(&out, "b3BlbnNzaC1rZXktdjEAAAAA", "[REDACTED:pem]");
    assert!(
        !out.contains("TESTKEYTESTKEY"),
        "the snipped body must be removed too: {out}"
    );
}

// --- Over-redaction is fine; under-redaction is not ---

#[test]
fn ordinary_prose_is_left_alone() {
    let prose = "I refactored the tokenizer so the password field on the login \
                 form reuses the same secret store. The token stream is \
                 append-only and the api_key is read from the environment at \
                 startup, never from a file. Next step: document the secrets \
                 rotation runbook.";
    let out = redact_secrets(prose);
    assert_eq!(
        out, prose,
        "prose mentioning credential words must survive untouched"
    );
}

#[test]
fn redacting_an_already_redacted_string_changes_nothing() {
    let once = redact_secrets("api_key=abcdefghijklmnop and sk_test0000000000000000000000000000");
    let twice = redact_secrets(&once);
    assert_eq!(once, twice, "redaction must be idempotent");
}

#[test]
fn empty_and_marker_only_input_is_stable() {
    for text in [
        "",
        "[REDACTED:secret]",
        "[REDACTED:vendor-key]",
        "no keys here",
    ] {
        assert_eq!(redact_secrets(text), text, "input {text:?} must be stable");
    }
}

// --- The assembly paths, not just the function ---

#[test]
fn structured_prompt_redacts_every_role() {
    let messages = vec![
        msg("user", "my key is sk_test0000000000000000000000000000"),
        msg(
            "assistant",
            "and mine was ghp_test0000000000000000000000000000",
        ),
        msg("tool", "the config had password=abcdefghijklmnop in it"),
    ];
    let prompt = build_structured_prompt(&messages);
    assert!(!prompt.contains("sk_test0000"), "user content leaked");
    assert!(!prompt.contains("ghp_test0000"), "assistant content leaked");
    assert!(!prompt.contains("abcdefghijklmnop"), "tool result leaked");
    assert!(prompt.contains("[REDACTED:"));
    assert!(
        prompt.contains("## Accomplished"),
        "the prompt body must survive"
    );
}

#[test]
fn state_prompt_redacts_session_content() {
    let messages = vec![msg(
        "user",
        "deploy with api_key=abcdefghijklmnop and sk_test0000000000000000000000000000",
    )];
    let prompt = build_state_prompt(&messages);
    assert_redacted(&prompt, "abcdefghijklmnop", "[REDACTED:");
    assert_redacted(&prompt, "sk_test0000", "[REDACTED:");
}

#[test]
fn goals_prompt_redacts_user_messages() {
    let users = vec![
        "use tvly-test0000000000000000000000 for the search".to_string(),
        "the deploy password=hunter-two-word".to_string(),
    ];
    let prompt = build_goals_prompt(&users);
    assert_redacted(&prompt, "tvly-test0000", "[REDACTED:");
    assert_redacted(&prompt, "hunter-two-word", "[REDACTED:");
    assert!(prompt.contains("### Goals"), "the prompt body must survive");
}

/// The deliberate limit of the assignment rules, pinned so it cannot drift
/// silently.
///
/// `password = value` is redacted. `the password is hunter-two` is prose, not
/// an assignment: no separator, no way to tell the value from the rest of the
/// sentence. Matching a copula instead would swallow ordinary sentences like
/// "the password field on the login form", which is a far more common thing to
/// read than a bare password in prose. A user who pastes a bare password into
/// a sentence without a separator is outside what shape-matching can promise,
/// and SECURITY.md says so.
#[test]
fn prose_password_without_a_separator_is_not_matched() {
    let prose = "the password is hunter-two";
    assert_eq!(redact_secrets(prose), prose);
}

#[test]
fn tool_call_summary_is_redacted() {
    let mut tool = msg("assistant", "");
    tool.tool_calls_summary = vec![
        "bash(export INCEPTION_API_KEY=abcdefghijklmnop)".to_string(),
        "read_file(src/mercury.rs)".to_string(),
    ];
    let prompt = build_structured_prompt(&[tool]);
    assert_redacted(&prompt, "abcdefghijklmnop", "[REDACTED:");
    assert!(
        prompt.contains("read_file(src/mercury.rs)"),
        "harmless tool calls must survive: {prompt}"
    );
}

#[test]
fn redaction_happens_before_truncation_not_after() {
    // The snip is 1500 chars for prose and 500 for tool results. A key that
    // starts at 400 chars in a tool result is inside the snipped window, so a
    // snip-first implementation would ship it intact. Redact-first shrinks
    // the text, and the marker is what gets snipped instead.
    let padding = "x".repeat(400);
    let secret = "sk_test0000000000000000000000000000";
    let content = format!("{padding} {secret}");
    let prompt = build_structured_prompt(&[msg("tool", &content)]);
    assert!(
        !prompt.contains(secret),
        "a secret inside the snip window must be redacted:\n{prompt}"
    );
    assert!(prompt.contains("[REDACTED:"), "the marker must be present");
}

/// A key whose body is cut in half by the snip must still be gone.
///
/// This is the failure mode that sets the order: truncate-then-redact ships the
/// surviving prefix of a key, and a prefix is a real thing to paste into a
/// login form.
#[test]
fn a_secret_straddling_the_snip_boundary_does_not_survive_as_a_prefix() {
    let secret = "sk_test0000000000000000000000000000";
    // Push the key's start to 480 in a tool result, so the 500-char snip cuts
    // it roughly in half.
    let content = format!("{} {secret}", "x".repeat(480));
    assert!(content.len() > 500, "the fixture must actually straddle");
    let prompt = build_structured_prompt(&[msg("tool", &content)]);
    let leaked: String = prompt
        .chars()
        .filter(|c| *c != 'x' && !c.is_whitespace() && !"TOOLRESULT:.()".contains(*c))
        .collect();
    assert!(
        !leaked.contains("sk_test"),
        "no prefix of the key may survive the snip:\n{prompt}"
    );
}

/// A prefix glued to the tail of a word is a word, not a key.
///
/// `flask-migrations` is not a `sk-` credential, and redacting it would bury
/// the prompt in markers. The boundary rule is what keeps ordinary hyphenated
/// identifiers readable; a real key arrives after a space, a quote or a
/// bracket, all of which are non-alphanumeric.
#[test]
fn a_key_glued_to_word_characters_is_not_matched() {
    let glued = format!("xxxx{}", "sk_test0000000000000000000000000000");
    assert_eq!(redact_secrets(&glued), glued);
}
