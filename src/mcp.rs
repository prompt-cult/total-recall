use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};

use crate::{
    MercuryProvider, RolloutAdapter, build_structured_prompt, harness::make_adapter,
    prompt::SYSTEM_PROMPT,
};

fn resolve_session(adapter: &dyn RolloutAdapter, session_id: &str) -> String {
    if session_id.is_empty() {
        let sessions = adapter.list_sessions();
        if sessions.is_empty() {
            return String::new();
        }
        sessions[0].session_id.clone()
    } else {
        session_id.to_string()
    }
}

// --- Tool parameter structs ---

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListSessionsParams {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ProfileParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    pub session_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ExtractParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    pub session_id: String,
    #[schemars(
        description = "If true, read entire session. If false, read from last compaction point."
    )]
    #[serde(default)]
    pub full: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UserMessagesParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    pub session_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CompactParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    pub session_id: String,
    #[schemars(
        description = "If true, compact entire session. If false, compact from last compaction point."
    )]
    #[serde(default)]
    pub full: bool,
}

// --- MCP Server ---

#[derive(Clone)]
pub struct CompactionServer {
    harness: String,
}

impl CompactionServer {
    pub fn with_harness(harness: String) -> Self {
        Self { harness }
    }

    pub fn harness(&self) -> &str {
        &self.harness
    }

    fn adapter(&self) -> Result<Box<dyn RolloutAdapter>, McpError> {
        make_adapter(&self.harness).map_err(|e| McpError::internal_error(e, None))
    }
}

#[tool_router]
impl CompactionServer {
    #[tool(
        name = "harness",
        description = "Report which harness this MCP server is bound to"
    )]
    async fn harness_tool(&self) -> Result<CallToolResult, McpError> {
        let json = serde_json::json!({ "harness": self.harness });
        let json = serde_json::to_string_pretty(&json)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(description = "List all agent session rollouts for the bound harness")]
    async fn list_sessions(&self) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let sessions = adapter.list_sessions();
        let json = serde_json::to_string_pretty(&sessions)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "Profile a session: file size, line count, role counts, interesting events"
    )]
    async fn profile_session(
        &self,
        Parameters(params): Parameters<ProfileParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = resolve_session(adapter.as_ref(), &params.session_id);
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }
        let profile = adapter.profile_session(&session_id);
        let json = serde_json::to_string_pretty(&profile)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(description = "Extract all messages from a session as structured JSON")]
    async fn extract_messages(
        &self,
        Parameters(params): Parameters<ExtractParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = resolve_session(adapter.as_ref(), &params.session_id);
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }
        let messages = if params.full {
            adapter.read_session_mmap(&session_id)
        } else {
            adapter.read_session_from_compaction(&session_id)
        };
        let json = serde_json::to_string_pretty(&messages)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(description = "Extract verbatim user messages from a session")]
    async fn extract_user_messages(
        &self,
        Parameters(params): Parameters<UserMessagesParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = resolve_session(adapter.as_ref(), &params.session_id);
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }
        let messages = adapter.extract_user_messages(&session_id);
        let json = serde_json::to_string_pretty(&messages)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "Compact a session using Mercury 2.5 — returns a structured summary with Accomplished, Current Work, Files, Next Steps, and Key Decisions"
    )]
    async fn compact_session(
        &self,
        Parameters(params): Parameters<CompactParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = resolve_session(adapter.as_ref(), &params.session_id);
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }
        let messages = if params.full {
            adapter.read_session_mmap(&session_id)
        } else {
            adapter.read_session_from_compaction(&session_id)
        };

        if messages.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No messages found in session".to_string(),
            )]));
        }

        let prompt = build_structured_prompt(&messages);

        let provider = MercuryProvider::new().map_err(|e| {
            McpError::internal_error(format!("Failed to create Mercury provider: {}", e), None)
        })?;

        let summary = provider
            .compact(SYSTEM_PROMPT, &prompt)
            .await
            .map_err(|e| McpError::internal_error(format!("Mercury API error: {}", e), None))?;

        Ok(CallToolResult::success(vec![ContentBlock::text(summary)]))
    }
}

#[tool_handler]
impl ServerHandler for CompactionServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}
