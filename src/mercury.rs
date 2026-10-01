use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::redact::redact_secrets;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

#[cfg(feature = "mercury")]
const MERCURY_API_URL: &str = "https://api.inceptionlabs.ai/v1/chat/completions";
#[cfg(feature = "mercury")]
const MERCURY_MODEL: &str = "mercury-2.5";

#[cfg(feature = "mistral")]
const MISTRAL_API_URL: &str = "https://api.mistral.ai/v1/chat/completions";
#[cfg(feature = "mistral")]
const MISTRAL_MODEL: &str = "mistral-small-latest";

/// The LLM vendors compiled into this build. Empty in a vendor-free build
/// (`--no-default-features`); see [`provider_for`].
pub const VENDORS: &[&str] = &[
    #[cfg(feature = "mercury")]
    "mercury",
    #[cfg(feature = "mistral")]
    "mistral",
];

/// Every vendor name this crate knows about, whether or not it is compiled
/// into this build. Separates "you asked for a vendor that isn't in this
/// build" from "that is not a vendor at all".
pub const KNOWN_VENDORS: &[&str] = &["mercury", "mistral"];

/// The LLM providers compiled into this build, for error messages.
fn vendor_list() -> String {
    if VENDORS.is_empty() {
        "none".to_string()
    } else {
        VENDORS.join(", ")
    }
}

/// The error a vendor-free (or vendor-mismatched) build returns instead of
/// silently falling back to a different vendor. States what is missing, how to
/// get it, and that the current binary is otherwise fully functional.
pub fn vendor_not_compiled_error(requested: &str) -> String {
    format!(
        "LLM provider `{requested}` is not compiled into this build: it was built without the \
         `{requested}` cargo feature. Rebuild with `cargo build --release --features {requested}`, \
         or use the vendor-free build (`--no-default-features`) — it makes no LLM calls and needs \
         no API key, and every log-mining tool still works. Vendors compiled into this build: {}.",
        vendor_list()
    )
}

/// The error for a `--provider` value that is not a vendor name at all. Names
/// the valid values so the flag is self-documenting.
pub fn unknown_provider_error(requested: &str) -> String {
    format!(
        "unknown LLM provider `{requested}`: pass it to --provider. Known vendors: {}; compiled \
         into this build: {}.",
        KNOWN_VENDORS.join(", "),
        vendor_list()
    )
}

/// Build the LLM provider for `name` (`None` = the default vendor, `mercury`),
/// or fail descriptively when that vendor is not compiled into this build.
/// This is the single construction point for both the CLI and the MCP server,
/// so a vendor-free build fails the same way everywhere and never falls back
/// to a vendor the caller did not ask for.
pub fn provider_for(name: Option<&str>) -> Result<MercuryProvider> {
    let requested = name.unwrap_or("mercury");
    if !KNOWN_VENDORS.contains(&requested) {
        return Err(anyhow!(unknown_provider_error(requested)));
    }
    match requested {
        #[cfg(feature = "mercury")]
        "mercury" => MercuryProvider::new(),
        #[cfg(feature = "mistral")]
        "mistral" => MercuryProvider::new_mistral(),
        _ => Err(anyhow!(vendor_not_compiled_error(requested))),
    }
}

/// Per-call input cap, in tokens (estimated at ~4 chars/token, the FAQ's own
/// ratio). This is a user-set outer guard against slinging a 100MiB rollout
/// in one call: over-cap prompts are rejected with an error, never truncated.
///
/// Note the cap is deliberately larger than Mercury 2.5's documented 260K
/// token context window (the constant is set by user instruction, not by the
/// context window), so practical calls sit far below the cap: tool results
/// are already snipped to 500 chars in prompt assembly (`messages_to_text`),
/// which keeps prompts small.
pub const MAX_INPUT_TOKENS_PER_CALL: usize = 1_000_000;

/// Maximum in-flight requests. Benchmarked sweet spot from the ingestion
/// probe of 2026-09-15 (real rollout payloads of 5k/10k/20k tokens,
/// concurrency ramped 1→64, raw data in the gitignored local scratch file
/// `.tmp/mercury-bench/report.md`): ~10k-token prompts at concurrency 4
/// sustained ~22k input tok/s with p50 latency 1.9s and almost no 429s,
/// while concurrency ≥8 hits the 429 wall with no throughput gain. The
/// documented PAYG tier caps are 1,000 req/min, 1M input tok/min and 100k
/// output tok/min; the input-token/min cap is the binding limit (~1M
/// rolling).
pub const MAX_CONCURRENCY: usize = 4;

/// 429 responses are retried at most this many times before giving up.
pub const MAX_429_RETRIES: usize = 5;

/// 5xx responses are retried at most this many times before giving up.
pub const MAX_5XX_RETRIES: usize = 3;

/// Upper bound on a single backoff sleep.
pub const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// Spacing between 5xx retries.
const RETRY_5XX_DELAY: Duration = Duration::from_secs(1);

/// Per-request timeout (unchanged from the pre-guardrail behaviour).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Rough token estimate at ~4 chars/token (ASCII-ish approx; fine for a
/// guard). This is the ratio Inception's own FAQ gives.
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f64,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

/// Mercury 2.5 provider for compaction.
///
/// Guarded against flooding and oversized calls per the ingestion probe of
/// 2026-09-15 (real rollout payloads 5k/10k/20k tokens, concurrency ramped
/// 1→64; raw data in the gitignored local scratch file
/// `.tmp/mercury-bench/report.md`): the binding limit is a rolling ~1M input
/// tokens/minute (documented PAYG cap; requests/min 1,000 and output
/// tok/min 100,000 never bind for compaction workloads). Guardrails:
///
/// - per-call input cap of [`MAX_INPUT_TOKENS_PER_CALL`] tokens
///   (error-not-truncate on breach),
/// - bounded concurrency of [`MAX_CONCURRENCY`] in-flight requests for
///   [`MercuryProvider::compact_batch`],
/// - 429 + `Retry-After` exponential backoff (5 retries, 10s/attempt cap),
///   429s logged via `tracing`, never hidden,
/// - 5xx retry (3 retries, ~1s spacing).
#[derive(Clone)]
pub struct MercuryProvider {
    api_key: String,
    model: String,
    api_url: String,
}

impl MercuryProvider {
    /// Create from INCEPTION_API_KEY env var (or .env file).
    #[cfg(feature = "mercury")]
    pub fn new() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let api_key = std::env::var("INCEPTION_API_KEY")
            .map_err(|_| anyhow!("INCEPTION_API_KEY not set in env or .env file"))?;
        Ok(Self {
            api_key,
            model: MERCURY_MODEL.to_string(),
            api_url: MERCURY_API_URL.to_string(),
        })
    }

    #[cfg(feature = "mercury")]
    pub fn with_model(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            api_url: MERCURY_API_URL.to_string(),
        }
    }

    /// Create with a custom API URL, key, and model (e.g. for Mistral).
    pub fn with_api(api_key: String, model: String, api_url: String) -> Self {
        Self {
            api_key,
            model,
            api_url,
        }
    }

    /// Create a Mistral provider from MISTRAL_API_KEY env var.
    #[cfg(feature = "mistral")]
    pub fn new_mistral() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let api_key = std::env::var("MISTRAL_API_KEY")
            .map_err(|_| anyhow!("MISTRAL_API_KEY not set in env or .env file"))?;
        Ok(Self::with_api(
            api_key,
            MISTRAL_MODEL.to_string(),
            MISTRAL_API_URL.to_string(),
        ))
    }

    /// Send the conversation text to Mercury for compaction.
    pub async fn compact(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        self.send_guarded(system_prompt, user_prompt).await
    }

    /// Run several prompts through the guarded request path under a
    /// concurrency semaphore of [`MAX_CONCURRENCY`] (the benchmarked sweet
    /// spot; see the probe notes on [`MAX_CONCURRENCY`]). Per-prompt results
    /// come back in input order. Each prompt is individually subject to the
    /// per-call input cap. All-or-error semantics: if any prompt fails after
    /// retries, the whole batch errs with the failing index and reason.
    ///
    /// The probe (2026-09-15) showed the documented PAYG tier caps at 1,000
    /// req/min, 1M input tok/min, 100k output tok/min, with the input-token
    /// cap binding as a rolling ~1M tok/min wall.
    pub async fn compact_batch(
        &self,
        system_prompt: &str,
        prompts: Vec<String>,
    ) -> Result<Vec<String>> {
        let calls: Vec<(String, String)> = prompts
            .into_iter()
            .map(|prompt| (system_prompt.to_string(), prompt))
            .collect();
        self.compact_batch_pairs(calls).await
    }

    /// Batch machinery behind [`MercuryProvider::compact_batch`] for calls
    /// that each carry their own system prompt: `calls[i]` is
    /// `(system_prompt, user_prompt)`. Same bounded-concurrency, cap, and
    /// all-or-error semantics; results in input order.
    pub async fn compact_batch_pairs(&self, calls: Vec<(String, String)>) -> Result<Vec<String>> {
        // Validate every prompt's cap BEFORE sending anything.
        for (i, (_, prompt)) in calls.iter().enumerate() {
            self.check_input_cap(prompt)
                .map_err(|e| anyhow!("batch prompt index {i}: {e:#}"))?;
        }

        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENCY));
        let this = Arc::new(self.clone());
        let mut handles = Vec::with_capacity(calls.len());
        for (i, (system, prompt)) in calls.into_iter().enumerate() {
            let this = this.clone();
            let semaphore = semaphore.clone();
            let system = system.clone();
            handles.push(tokio::spawn(async move {
                // Bounded concurrency: the permit is held for the whole
                // request (including retries), capping in-flight requests at
                // MAX_CONCURRENCY.
                let _permit = semaphore.acquire_owned().await.expect("semaphore open");
                (i, this.send_guarded(&system, &prompt).await)
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            let (i, outcome) = handle
                .await
                .map_err(|e| anyhow!("batch prompt task panicked: {e}"))?;
            let text = outcome.map_err(|e| anyhow!("batch prompt index {i} failed: {e:#}"))?;
            results.push(text);
        }
        Ok(results)
    }

    fn check_input_cap(&self, prompt: &str) -> Result<()> {
        let estimated = estimate_tokens(prompt);
        if estimated > MAX_INPUT_TOKENS_PER_CALL {
            return Err(anyhow!(
                "prompt too large: ~{estimated} estimated tokens exceeds the per-call cap of {MAX_INPUT_TOKENS_PER_CALL} tokens (~4 chars/token); never truncates — split the input"
            ));
        }
        Ok(())
    }

    /// Shared guarded request path: redaction, input-cap check, then a retry
    /// loop over 429 (Retry-After honoured, exponential backoff capped at 10s,
    /// [`MAX_429_RETRIES`] retries) and 5xx ([`MAX_5XX_RETRIES`] retries,
    /// ~1s spacing). Each attempt keeps the 120s per-request timeout.
    ///
    /// Redaction runs here, first, on both prompts. The prompt builders
    /// already redact — see [`crate::redact`] — and this is the backstop that
    /// makes the guarantee a property of the process rather than of every
    /// caller's diligence: a future prompt builder that forgets will leak into
    /// a prompt object but not onto the wire. [`crate::redact::redact_secrets`]
    /// is idempotent, so a double pass costs a scan and mangles nothing.
    async fn send_guarded(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let system_prompt = redact_secrets(system_prompt);
        let user_prompt = redact_secrets(user_prompt);
        let (system_prompt, user_prompt) = (system_prompt.as_str(), user_prompt.as_str());
        self.check_input_cap(user_prompt)?;

        let client = reqwest::Client::new();
        // Only send reasoning_effort for Mercury API
        #[cfg(feature = "mercury")]
        let is_mercury_api = self.api_url == MERCURY_API_URL;
        #[cfg(not(feature = "mercury"))]
        let is_mercury_api = false;
        let reasoning_effort = if is_mercury_api {
            Some("low".to_string())
        } else {
            None
        };
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt.to_string(),
                },
            ],
            temperature: 0.1,
            max_tokens: 4000,
            reasoning_effort,
        };

        let mut retries_429 = 0usize;
        let mut retries_5xx = 0usize;
        loop {
            let resp = client
                .post(&self.api_url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&request)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await?;

            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                if retries_429 >= MAX_429_RETRIES {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(anyhow!(
                        "Mercury rate limit: still 429 Too Many Requests after {MAX_429_RETRIES} retries: {}",
                        body
                    ));
                }
                retries_429 += 1;
                // Honour Retry-After when present; otherwise back off
                // exponentially (1s, 2s, 4s, 8s, capped at MAX_BACKOFF).
                let fallback = Duration::from_secs(1 << (retries_429 - 1));
                let delay = retry_after_or(resp.headers(), fallback);
                tracing::warn!(
                    "Mercury 429 (retry {retries_429}/{MAX_429_RETRIES}); backing off {:?}",
                    delay
                );
                tokio::time::sleep(delay).await;
                continue;
            }
            if status.is_server_error() {
                if retries_5xx >= MAX_5XX_RETRIES {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(anyhow!(
                        "Mercury API error {} after {MAX_5XX_RETRIES} retries: {}",
                        status,
                        body
                    ));
                }
                retries_5xx += 1;
                tracing::warn!(
                    "Mercury 5xx {} (retry {retries_5xx}/{MAX_5XX_RETRIES}); retrying in {:?}",
                    status,
                    RETRY_5XX_DELAY
                );
                tokio::time::sleep(RETRY_5XX_DELAY).await;
                continue;
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow!("Mercury API error {}: {}", status, body));
            }

            let chat_response: ChatResponse = resp.json().await?;
            return chat_response
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .ok_or_else(|| anyhow!("Mercury returned no content"));
        }
    }
}

/// Read `Retry-After` (seconds form) from response headers; missing or
/// unparseable → `fallback`. The value is capped at [`MAX_BACKOFF`]. Public
/// because the cap and the seconds-only parsing are part of the documented
/// ingestion contract and are asserted directly.
pub fn retry_after_or(headers: &reqwest::header::HeaderMap, fallback: Duration) -> Duration {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs).min(MAX_BACKOFF))
        .unwrap_or(fallback.min(MAX_BACKOFF))
}

#[cfg(feature = "mercury")]
impl Default for MercuryProvider {
    fn default() -> Self {
        Self::new().expect("Failed to create MercuryProvider")
    }
}
