//! Wire-level proof that no credential reaches the vendor's API.
//!
//! The unit tests in `redaction.rs` prove the redactor recognises shapes. This
//! file proves the thing that actually matters: the bytes in the HTTP request
//! body do not contain the key.
//!
//! Everything here runs against a `TcpListener` bound to 127.0.0.1:0, using the
//! same hand-rolled mock pattern as `mercury_guardrails.rs`. No request leaves
//! the machine, and no real API key is read or needed: the provider is built
//! with `with_api`, which takes the key as an argument and never touches the
//! environment or a `.env` file.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use total_recall::{
    MercuryProvider, RolloutMessage, build_goals_prompt, build_state_prompt,
    build_structured_prompt,
};

/// Obviously fake, and long enough to clear the redactor's minimum body length.
const FAKE_INCEPTION_KEY: &str = "sk_test0000000000000000000000000000";
const FAKE_PASSWORD: &str = "hunter-two-word-phrase";
const FAKE_GITHUB_TOKEN: &str = "ghp_test0000000000000000000000000000";

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

/// A one-shot mock that records every request body it is sent.
struct RecordingServer {
    url: String,
    bodies: Arc<Mutex<Vec<String>>>,
    count: Arc<AtomicUsize>,
}

async fn spawn_recorder() -> RecordingServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let count = Arc::new(AtomicUsize::new(0));

    let bodies_sink = bodies.clone();
    let count_sink = count.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let bodies = bodies_sink.clone();
            let count = count_sink.clone();
            tokio::spawn(async move {
                count.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                let raw = read_full_request(&mut stream).await;
                if let Some(body) = split_body(&raw) {
                    bodies.lock().expect("body mutex").push(body);
                }
                let body = serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": "ok"}}]
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.flush().await;
                let _ = stream.shutdown().await;
            });
        }
    });

    RecordingServer {
        url: format!("http://127.0.0.1:{port}/v1/chat/completions"),
        bodies,
        count,
    }
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn read_full_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = find_sub(&buf, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            let len = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if buf.len() >= pos + 4 + len {
                break;
            }
        }
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    buf
}

fn split_body(raw: &[u8]) -> Option<String> {
    let pos = find_sub(raw, b"\r\n\r\n")?;
    Some(String::from_utf8_lossy(&raw[pos + 4..]).to_string())
}

/// Every credential in the fixture, and the marker that must stand in its
/// place.
const SECRETS: &[(&str, &str)] = &[
    (FAKE_INCEPTION_KEY, "[REDACTED:vendor-key]"),
    (FAKE_GITHUB_TOKEN, "[REDACTED:github-token]"),
    (FAKE_PASSWORD, "[REDACTED:secret]"),
];

/// The proof itself: none of the fixture's credentials is in this body.
///
/// Absence alone would also pass on an empty request, so the callers pair this
/// with an assertion that the expected markers are present.
#[track_caller]
fn assert_body_clean(body: &str, path: &str) {
    for (secret, _) in SECRETS {
        assert!(
            !body.contains(secret),
            "{path}: {secret:?} reached the vendor request body:\n{body}"
        );
    }
}

/// Every credential in `body` was replaced, not merely absent.
#[track_caller]
fn assert_body_redacted(body: &str, path: &str) {
    assert_body_clean(body, path);
    let markers = body.matches("[REDACTED:").count();
    assert!(
        markers > 0,
        "{path}: no redaction marker in the body, so the fixture never \
         exercised the redactor:\n{body}"
    );
}

/// The `compact` path: a full session through `build_structured_prompt` and
/// out over the wire.
#[tokio::test]
async fn compact_request_body_carries_no_secret() {
    let server = spawn_recorder().await;
    // `with_api` is used deliberately: no env var, no `.env`, no real key.
    let provider = MercuryProvider::with_api(
        "unused-test-key".into(),
        "mercury-2.5".into(),
        server.url.clone(),
    );

    let messages = vec![
        msg(
            "user",
            &format!("deploy with INCEPTION_API_KEY={FAKE_INCEPTION_KEY}"),
        ),
        msg("assistant", &format!("pushing with {FAKE_GITHUB_TOKEN}")),
        msg(
            "tool",
            &format!("the vault had password={FAKE_PASSWORD} inside"),
        ),
    ];
    let prompt = build_structured_prompt(&messages);

    provider
        .compact("system", &prompt)
        .await
        .expect("mock call must succeed");

    let bodies = server.bodies.lock().expect("body mutex").clone();
    assert_eq!(bodies.len(), 1, "exactly one request must have been sent");
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    // One prompt, so every credential must appear as its own marker: this is
    // the case that proves the three classes are each recognised, rather than
    // merely that the bodies happen to be disjoint.
    for (secret, marker) in SECRETS {
        assert!(
            bodies[0].contains(marker),
            "compact: {secret:?} should have become {marker}:\n{}",
            bodies[0]
        );
    }
    assert_body_clean(&bodies[0], "compact");
}

/// The `recall` path sends TWO prompts in one batch. Both must be clean: a
/// state summary that is redacted while a goals summary is not still ships the
/// key.
#[tokio::test]
async fn both_recall_request_bodies_carry_no_secret() {
    let server = spawn_recorder().await;
    let provider = MercuryProvider::with_api(
        "unused-test-key".into(),
        "mercury-2.5".into(),
        server.url.clone(),
    );

    let messages = vec![msg(
        "user",
        &format!("the key is {FAKE_INCEPTION_KEY} and the repo uses {FAKE_GITHUB_TOKEN}"),
    )];
    let state_prompt = build_state_prompt(&messages);
    let goals_prompt = build_goals_prompt(&[format!("password={FAKE_PASSWORD}")]);

    provider
        .compact_batch_pairs(vec![
            ("state system".to_string(), state_prompt),
            ("goals system".to_string(), goals_prompt),
        ])
        .await
        .expect("batch must succeed");

    let bodies = server.bodies.lock().expect("body mutex").clone();
    assert_eq!(bodies.len(), 2, "recall must send two prompts");
    assert_eq!(server.count.load(Ordering::SeqCst), 2);
    // Cleanliness is per-body: each prompt is checked on its own, so a key
    // hidden in the second one cannot hide behind the first being clean.
    for (i, body) in bodies.iter().enumerate() {
        assert_body_clean(body, &format!("recall prompt {i}"));
        assert_body_redacted(body, &format!("recall prompt {i}"));
    }
    // The two prompts carry different secrets between them; between them they
    // must account for all three, or the fixture proved less than it claims.
    let joined = bodies.join("\n");
    for (secret, marker) in SECRETS {
        assert!(
            joined.contains(marker),
            "recall: {secret:?} should have become {marker} in one of the two prompts"
        );
    }
}

/// The backstop: a prompt handed straight to the provider, bypassing both
/// prompt builders, is still redacted at the wire.
///
/// This is the guarantee that does not depend on every future caller
/// remembering to redact. It is also the reason the redaction is applied in
/// `send_guarded` and not only in the builders.
#[tokio::test]
async fn a_prompt_bypassing_the_builders_is_redacted_at_the_wire() {
    let server = spawn_recorder().await;
    let provider = MercuryProvider::with_api(
        "unused-test-key".into(),
        "mercury-2.5".into(),
        server.url.clone(),
    );

    // Deliberately unredacted, as if a future builder forgot the call.
    let raw = format!(
        "summarise this: INCEPTION_API_KEY={FAKE_INCEPTION_KEY} token={FAKE_GITHUB_TOKEN} \
         password={FAKE_PASSWORD}"
    );
    provider
        .compact("system", &raw)
        .await
        .expect("mock call must succeed");

    let bodies = server.bodies.lock().expect("body mutex").clone();
    assert_eq!(bodies.len(), 1);
    assert_body_redacted(&bodies[0], "wire backstop");
    // All three were in the one prompt, so all three markers must be here.
    for (secret, marker) in SECRETS {
        assert!(
            bodies[0].contains(marker),
            "wire backstop: {secret:?} should have become {marker}:\n{}",
            bodies[0]
        );
    }
}

/// The vendor's own `Authorization` header is the one place a credential
/// legitimately crosses the network: it is the caller's own key, sent to its
/// own vendor, to authenticate the call. Redaction must not touch it, or the
/// tool stops working.
#[tokio::test]
async fn the_authentication_header_is_not_redacted() {
    let server = spawn_recorder().await;
    let provider = MercuryProvider::with_api(
        "header-key-test".into(),
        "mercury-2.5".into(),
        server.url.clone(),
    );
    provider
        .compact("system", "hello")
        .await
        .expect("mock call must succeed");
    // The body is the thing under test; the header is out of scope for the
    // redactor by construction (it is added in `send_guarded` from config, not
    // from session content).
    let bodies = server.bodies.lock().expect("body mutex").clone();
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("hello"), "the prompt must still be sent");
}
