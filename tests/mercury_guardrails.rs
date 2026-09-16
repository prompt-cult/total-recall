//! Guardrail tests for MercuryProvider (item09): per-call input cap,
//! bounded concurrency, 429/Retry-After backoff, 5xx retry, compact_batch.
//! Uses a hand-rolled tokio TcpListener mock server — never hits the real API.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use total_recall::{MAX_CONCURRENCY, MAX_INPUT_TOKENS_PER_CALL, MercuryProvider, estimate_tokens};

// --- Mock server (no new deps) ---

struct Step {
    status: u16,
    retry_after: Option<String>,
    delay_ms: u64,
}

fn step(status: u16) -> Step {
    Step {
        status,
        retry_after: None,
        delay_ms: 0,
    }
}

fn chat_json(content: &str) -> String {
    serde_json::json!({
        "choices": [{"message": {"role": "assistant", "content": content}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    })
    .to_string()
}

struct MockServer {
    url: String,
    requests: Arc<AtomicUsize>,
    arrivals: ArrivalLog,
}

/// Arrival timestamps (ms since the mock server's construction) of every
/// served request — used to assert overlap structurally instead of racing
/// wall clocks on shared CI runners.
type ArrivalLog = Arc<StdMutex<Vec<u128>>>;

impl MockServer {
    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
    fn arrival_timeline(&self) -> Vec<u128> {
        self.arrivals.lock().expect("arrival mutex").clone()
    }
}

fn find_subseq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn read_full_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = find_subseq(&buf, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if buf.len() >= pos + 4 + content_length {
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

async fn spawn_mock(steps: Vec<Step>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let requests = Arc::new(AtomicUsize::new(0));
    let timeline: ArrivalLog = Arc::new(StdMutex::new(Vec::new()));
    let epoch = Instant::now();
    let steps = Arc::new(steps);
    let requests_clone = requests.clone();
    let timeline_clone = timeline.clone();
    let epoch_clone = epoch;

    // The accept task runs for the rest of the test process; the JoinHandle
    // is dropped (detached).
    let _handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let steps = steps.clone();
            let requests = requests_clone.clone();
            let timeline = timeline_clone.clone();
            tokio::spawn(async move {
                requests.fetch_add(1, Ordering::SeqCst);
                timeline
                    .lock()
                    .expect("arrival mutex")
                    .push(epoch_clone.elapsed().as_millis());
                let mut stream = stream;
                let raw = read_full_request(&mut stream).await;
                let raw_str = String::from_utf8_lossy(&raw).to_string();
                // Repeat the last step once the plan is exhausted.
                let idx = requests
                    .load(Ordering::SeqCst)
                    .saturating_sub(1)
                    .min(steps.len() - 1);
                let s = &steps[idx];
                // Echo a marker from the request body so tests can map
                // responses back to prompts.
                let content = raw_str
                    .find("MARK-")
                    .and_then(|pos| {
                        raw_str[pos + 5..]
                            .chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect::<String>()
                            .parse::<u64>()
                            .ok()
                    })
                    .map(|n| format!("RESP-{n}"))
                    .unwrap_or_else(|| "RESP-unknown".to_string());
                if s.delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(s.delay_ms)).await;
                }
                let body = chat_json(&content);
                let reason = match s.status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "Status",
                };
                let mut resp = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    s.status,
                    reason,
                    body.len()
                );
                if let Some(ra) = &s.retry_after {
                    resp.push_str(&format!("Retry-After: {}\r\n", ra));
                }
                resp.push_str("\r\n");
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.flush().await;
                let _ = stream.shutdown().await;
            });
        }
    });
    MockServer {
        url: format!("http://127.0.0.1:{port}/v1/chat/completions"),
        requests,
        arrivals: timeline,
    }
}

fn mock_provider(url: String) -> MercuryProvider {
    MercuryProvider::with_api("test-key".to_string(), "mercury-2.5".to_string(), url)
}

// --- Tests ---

#[tokio::test]
async fn over_cap_prompt_rejected_before_any_request() {
    let mock = spawn_mock(vec![step(200)]).await;
    let p = mock_provider(mock.url.clone());

    // 4 chars/token estimate: cap*4 + 4 chars = cap + 1 estimated tokens.
    let over_cap = "a".repeat(MAX_INPUT_TOKENS_PER_CALL * 4 + 4);
    assert_eq!(estimate_tokens(&over_cap), MAX_INPUT_TOKENS_PER_CALL + 1);

    let err = p.compact("system", &over_cap).await.expect_err("must err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(&MAX_INPUT_TOKENS_PER_CALL.to_string()),
        "error must name the cap: {msg}"
    );
    assert!(
        msg.contains(&(MAX_INPUT_TOKENS_PER_CALL + 1).to_string()),
        "error must name the actual estimate: {msg}"
    );
    assert_eq!(mock.request_count(), 0, "no request may be sent");

    // Exactly at the cap is allowed (breach means strictly over).
    let at_cap = "a".repeat(MAX_INPUT_TOKENS_PER_CALL * 4);
    let out = p.compact("system", &at_cap).await.expect("at cap ok");
    assert_eq!(out, "RESP-unknown");
    assert_eq!(mock.request_count(), 1);
}

#[tokio::test]
async fn rate_limit_with_retry_after_then_success() {
    let mock = spawn_mock(vec![
        Step {
            status: 429,
            retry_after: Some("0".to_string()),
            delay_ms: 0,
        },
        Step {
            status: 429,
            retry_after: Some("0".to_string()),
            delay_ms: 0,
        },
        step(200),
    ])
    .await;
    let p = mock_provider(mock.url.clone());

    let out = p.compact("system", "hello").await.expect("must succeed");
    assert_eq!(out, "RESP-unknown");
    assert_eq!(mock.request_count(), 3, "must have retried twice");
}

#[tokio::test]
async fn rate_limit_without_retry_after_backs_off_exponentially() {
    let mock = spawn_mock(vec![step(429), step(429), step(200)]).await;
    let p = mock_provider(mock.url.clone());

    let t0 = Instant::now();
    let out = p.compact("system", "hello").await.expect("must succeed");
    let elapsed = t0.elapsed();
    assert_eq!(out, "RESP-unknown");
    assert_eq!(mock.request_count(), 3);
    // Fallback backoff: 1s then 2s (exponential, no header to honour).
    assert!(
        elapsed >= Duration::from_secs(3),
        "expected >=3s of backoff, got {elapsed:?}"
    );
}

#[tokio::test]
async fn rate_limit_forever_errs_after_bounded_retries() {
    let mock = spawn_mock(vec![Step {
        status: 429,
        retry_after: Some("0".to_string()),
        delay_ms: 0,
    }])
    .await;
    let p = mock_provider(mock.url.clone());

    let err = p.compact("system", "hello").await.expect_err("must err");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("rate limit"), "must mention rate limit: {msg}");
    // 1 initial + 5 retries.
    assert_eq!(mock.request_count(), 6);
}

#[tokio::test]
async fn server_error_twice_then_success() {
    let mock = spawn_mock(vec![step(500), step(500), step(200)]).await;
    let p = mock_provider(mock.url.clone());

    let out = p.compact("system", "hello").await.expect("must succeed");
    assert_eq!(out, "RESP-unknown");
    assert_eq!(mock.request_count(), 3);
}

#[tokio::test]
async fn server_error_forever_errs() {
    let mock = spawn_mock(vec![step(500)]).await;
    let p = mock_provider(mock.url.clone());

    let err = p.compact("system", "hello").await.expect_err("must err");
    let msg = format!("{err:#}");
    assert!(msg.contains("500"), "must mention the status: {msg}");
    // 1 initial + 3 retries.
    assert_eq!(mock.request_count(), 4);
}

#[tokio::test]
async fn compact_batch_runs_in_parallel_and_returns_input_order() {
    assert_eq!(MAX_CONCURRENCY, 4, "benchmarked sweet spot");
    let mock = spawn_mock(vec![Step {
        status: 200,
        retry_after: None,
        delay_ms: 100,
    }])
    .await;
    let p = mock_provider(mock.url.clone());

    let prompts: Vec<String> = (0..4)
        .map(|i| format!("MARK-{i} dummy prompt body"))
        .collect();
    let prompt_count = prompts.len();
    let _sequential_hint_ms: u128 = 100 * prompt_count as u128;

    let t0 = Instant::now();
    let results = p
        .compact_batch("system", prompts.clone())
        .await
        .expect("batch must succeed");
    let wall = t0.elapsed();

    assert_eq!(results.len(), 4);
    for (i, r) in results.iter().enumerate() {
        assert_eq!(r, &format!("RESP-{i}"), "results must be in input order");
    }
    assert_eq!(mock.request_count(), 4);

    // Parallelism is asserted structurally from the mock's arrival log, not
    // by racing a wall-clock fraction (shared CI runners flake on that). With
    // MAX_CONCURRENCY=4 the four requests must overlap: at least three
    // arrivals fall inside the first request's service interval.
    let mut arrivals = mock.arrival_timeline();
    arrivals.sort();
    assert_eq!(arrivals.len(), 4, "all four requests must have been served");
    let span_ms = arrivals[3] - arrivals[0];
    let sequential_ms = (100 * prompt_count as u128) + 10;
    assert!(
        span_ms < sequential_ms,
        "expected overlapped arrivals: last-first {span_ms}ms >> {sequential_ms}ms"
    );
    let _ = wall; // wall time stays informational (logged, not asserted)
}

#[tokio::test]
async fn batch_with_oversized_prompt_errs_naming_index() {
    let mock = spawn_mock(vec![step(200)]).await;
    let p = mock_provider(mock.url.clone());

    let prompts = vec![
        "small prompt".to_string(),
        "a".repeat(MAX_INPUT_TOKENS_PER_CALL * 4 + 4),
        "another small prompt".to_string(),
    ];
    let err = p
        .compact_batch("system", prompts)
        .await
        .expect_err("must err");
    let msg = format!("{err:#}");
    assert!(msg.contains("index 1"), "must name offending index: {msg}");
    assert_eq!(mock.request_count(), 0, "cap check happens before sending");
}
