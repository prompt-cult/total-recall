use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

const MERCURY_API_URL: &str = "https://api.inceptionlabs.ai/v1/chat/completions";
const MERCURY_MODEL: &str = "mercury-2.5";

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f64,
    max_tokens: u32,
    reasoning_effort: String,
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
pub struct MercuryProvider {
    api_key: String,
    model: String,
}

impl MercuryProvider {
    /// Create from INCEPTION_API_KEY env var (or .env file).
    pub fn new() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let api_key = std::env::var("INCEPTION_API_KEY")
            .map_err(|_| anyhow!("INCEPTION_API_KEY not set in env or .env file"))?;
        Ok(Self {
            api_key,
            model: MERCURY_MODEL.to_string(),
        })
    }

    pub fn with_model(api_key: String, model: String) -> Self {
        Self { api_key, model }
    }

    /// Send the conversation text to Mercury for compaction.
    /// Uses the structured prompt with reasoning_effort=low.
    pub async fn compact(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let client = reqwest::Client::new();
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
            reasoning_effort: "low".to_string(),
        };

        let resp = client
            .post(MERCURY_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Mercury API error {}: {}", status, body));
        }

        let chat_response: ChatResponse = resp.json().await?;
        chat_response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| anyhow!("Mercury returned no content"))
    }
}

impl Default for MercuryProvider {
    fn default() -> Self {
        Self::new().expect("Failed to create MercuryProvider")
    }
}
