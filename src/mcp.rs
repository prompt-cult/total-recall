use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};

use crate::{
    MercuryProvider, RolloutAdapter, build_structured_prompt,
    harness::make_adapter,
    prompt::SYSTEM_PROMPT,
    recall::{
        GOALS_SYSTEM_PROMPT, STATE_SYSTEM_PROMPT, build_goals_prompt, build_plan_files_section,
        build_recall_output, build_recent_rollouts_table, build_state_prompt,
        filter_recent_sessions,
    },
};

// --- Tool parameter structs ---

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListSessionsParams {
    #[schemars(
        description = "Only include sessions updated within this many hours. 0 = no bound (default)."
    )]
    #[serde(default)]
    pub hours_back: u64,
    #[schemars(description = "Only include sessions whose directory contains this substring.")]
    pub directory: Option<String>,
}

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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TotalRecallParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    pub session_id: String,
    #[schemars(description = "Hours back to include in the recent rollouts table. Default: 24.")]
    #[serde(default = "default_hours")]
    pub hours_back: u64,
}

fn default_hours() -> u64 {
    24
}

fn default_she_said_hours() -> u64 {
    48
}

fn default_index_hours() -> u64 {
    0
}

fn default_sheep_hours() -> u64 {
    48
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SheSaidHeSaidParams {
    #[schemars(
        description = "Session IDs (partial match). Empty = all sessions updated within hours_back."
    )]
    #[serde(default)]
    pub sessions: Vec<String>,
    #[schemars(description = "Case-insensitive search terms. At least one is required.")]
    pub words: Vec<String>,
    #[schemars(description = "Hours back when sessions is empty. 0 = no bound. Default: 48.")]
    #[serde(default = "default_she_said_hours")]
    pub hours_back: u64,
    #[schemars(description = "Optional directory substring filter (empty-session-list mode).")]
    pub directory: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexSessionsParams {
    #[schemars(
        description = "Session IDs (partial match). Empty = all sessions updated within hours_back."
    )]
    #[serde(default)]
    pub sessions: Vec<String>,
    #[schemars(description = "Hours back when sessions is empty. 0 = no bound. Default: 0.")]
    #[serde(default = "default_index_hours")]
    pub hours_back: u64,
    #[schemars(description = "Optional directory substring filter (empty-session-list mode).")]
    pub directory: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SheepParams {
    #[schemars(description = "Tantivy query syntax. Required.")]
    pub query: String,
    #[schemars(
        description = "Session IDs (partial match). Empty = all sessions updated within hours_back."
    )]
    #[serde(default)]
    pub sessions: Vec<String>,
    #[schemars(description = "Hours back when sessions is empty. 0 = no bound. Default: 48.")]
    #[serde(default = "default_sheep_hours")]
    pub hours_back: u64,
    #[schemars(description = "Optional directory substring filter (empty-session-list mode).")]
    pub directory: Option<String>,
}

// --- MCP Server ---

#[derive(Clone)]
pub struct TotalRecallServer {
    harness: String,
}

impl TotalRecallServer {
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
impl TotalRecallServer {
    #[tool(
        name = "harness",
        description = "Total-recall MCP tool: report which harness this server is bound to"
    )]
    async fn harness_tool(&self) -> Result<CallToolResult, McpError> {
        let json = serde_json::json!({ "harness": self.harness });
        let json = serde_json::to_string_pretty(&json)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "Total-recall MCP tool: list all agent session rollouts for the bound harness, optionally bounded by hours_back (default 0 = no bound) and a directory substring filter"
    )]
    async fn list_sessions(
        &self,
        Parameters(params): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let mut sessions = adapter.list_sessions();
        if params.hours_back > 0 {
            let cutoff = crate::rollout::opencode::iso_cutoff(params.hours_back);
            sessions.retain(|s| !crate::rollout::is_iso8601(&s.end_time) || s.end_time.as_str() >= cutoff.as_str());
        }
        if let Some(directory) = params.directory.as_deref().filter(|d| !d.is_empty()) {
            sessions.retain(|s| s.directory.as_deref().is_none_or(|d| d.contains(directory)));
        }
        crate::index::annotate_sessions(&mut sessions, adapter.as_ref());
        let json = serde_json::to_string_pretty(&sessions)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        name = "she_said_he_said_action",
        description = "Total-recall MCP tool: given case-insensitive terms, extract per session the HE SAID (user text), SHE SAID (assistant text) and THEY DID (tool calls) matching any term, as a markdown report ordered most-recent session first. Sessions are given by partial IDs, or, when the list is empty, all rollouts updated within hours_back (default 48) optionally filtered by a directory substring. Matching runs inside SQLite on a read-only connection."
    )]
    async fn she_said_he_said_action(
        &self,
        Parameters(params): Parameters<SheSaidHeSaidParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        if params.words.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "she_said_he_said_action requires at least one term in `words`".to_string(),
            )]));
        }
        match adapter.she_said_he_said_action(
            &params.sessions,
            &params.words,
            params.hours_back,
            params.directory.as_deref(),
        ) {
            Ok(report) => Ok(CallToolResult::success(vec![ContentBlock::text(report)])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }

    #[tool(
        description = "Total-recall MCP tool: build or refresh per-session tantivy full-text shadow indexes for the selected sessions. Sessions are given by partial IDs, or, when the list is empty, all rollouts updated within hours_back (default 0 = no bound) optionally filtered by a directory substring."
    )]
    async fn index_sessions(
        &self,
        Parameters(params): Parameters<IndexSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let (selected, unmatched) = crate::index::select_sessions(
            adapter.as_ref(),
            &params.sessions,
            params.hours_back,
            params.directory.as_deref(),
        );
        if !unmatched.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "index_sessions: unmatched session ids: {}",
                unmatched.join(", ")
            ))]));
        }
        let mut lines = Vec::new();
        for summary in &selected {
            match crate::index::index_session(adapter.as_ref(), &summary.session_id) {
                Ok(stats) => {
                    lines.push(format!("indexed {} docs={}", stats.session_id, stats.doc_count))
                }
                Err(e) => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(e)]));
                }
            }
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(
            lines.join("\n"),
        )]))
    }

    #[tool(
        name = "do_android_dream_of_electric_sheep",
        description = "Total-recall MCP tool: full-text search (tantivy) across per-session shadow indexes, merging top hits per session by score. Sessions are given by partial IDs, or, when the list is empty, all rollouts updated within hours_back (default 48) optionally filtered by a directory substring. Sessions without an index are reported as not indexed (run index_sessions first)."
    )]
    async fn do_android_dream_of_electric_sheep(
        &self,
        Parameters(params): Parameters<SheepParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        if params.query.trim().is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "do_android_dream_of_electric_sheep requires a tantivy query in `query`"
                    .to_string(),
            )]));
        }
        match crate::index::search(
            adapter.as_ref(),
            &params.sessions,
            &params.query,
            params.hours_back,
            params.directory.as_deref(),
        ) {
            Ok(report) => Ok(CallToolResult::success(vec![ContentBlock::text(report)])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }

    #[tool(
        description = "Total-recall MCP tool: profile a session — file size, line count, role counts, interesting events"
    )]
    async fn profile_session(
        &self,
        Parameters(params): Parameters<ProfileParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }
        let mut profile = adapter.profile_session(&session_id);
        profile.has_tantivy_index = crate::index::index_exists(adapter.as_ref(), &session_id);
        let json = serde_json::to_string_pretty(&profile)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "Total-recall MCP tool: extract all messages from a session as structured JSON"
    )]
    async fn extract_messages(
        &self,
        Parameters(params): Parameters<ExtractParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
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

    #[tool(description = "Total-recall MCP tool: extract verbatim user messages from a session")]
    async fn extract_user_messages(
        &self,
        Parameters(params): Parameters<UserMessagesParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
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
        description = "Total-recall MCP tool: fast compaction of a session using Mercury 2.5 — returns a structured summary with Accomplished, Current Work, Files, Next Steps, and Key Decisions. Supplements the built-in slower compactions."
    )]
    async fn compact_session(
        &self,
        Parameters(params): Parameters<CompactParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
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

    #[tool(
        name = "total_recall",
        description = "Total-recall MCP tool: fast compaction and log mining of session rollouts as a long-term memory store. Summarises the current session state, extracts user goals/tasks/steers, lists recent rollouts, and lists plan/todo files. Makes two parallel LLM calls and assembles a combined output to supplement the shorter faster compactions."
    )]
    async fn total_recall(
        &self,
        Parameters(params): Parameters<TotalRecallParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }

        // Read messages from last compaction point
        let messages = adapter.read_session_from_compaction(&session_id);
        if messages.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No messages found in session".to_string(),
            )]));
        }

        // Extract user messages from the full session
        let user_messages = adapter.extract_user_messages(&session_id);

        // Build prompts
        let state_prompt = build_state_prompt(&messages);
        let goals_prompt = build_goals_prompt(&user_messages);

        // Create provider
        let provider = MercuryProvider::new().map_err(|e| {
            McpError::internal_error(format!("Failed to create Mercury provider: {}", e), None)
        })?;

        // One bounded-concurrency batch call for both LLM calls
        let t0 = std::time::Instant::now();
        let batch_results = provider
            .compact_batch_pairs(vec![
                (STATE_SYSTEM_PROMPT.to_string(), state_prompt),
                (GOALS_SYSTEM_PROMPT.to_string(), goals_prompt),
            ])
            .await;
        let total_time = t0.elapsed();

        let [state_summary, goals_summary] = batch_results
            .map_err(|e| McpError::internal_error(format!("Mercury API error: {e:#}"), None))?
            .try_into()
            .expect("compact_batch_pairs returns one result per call");

        // Build recent rollouts table
        let all_sessions = adapter.list_sessions();
        let recent_sessions = filter_recent_sessions(&all_sessions, params.hours_back);
        let rollouts_table =
            build_recent_rollouts_table(&recent_sessions, Some(&session_id), params.hours_back);

        // Build plan files section
        let plan_files = build_plan_files_section();

        // Assemble output
        let output =
            build_recall_output(&state_summary, &goals_summary, &rollouts_table, &plan_files);

        // Log timing info as JSON prefix (for debugging)
        let timing = serde_json::json!({
            "total_time_s": total_time.as_secs_f64(),
            "session_messages_count": messages.len(),
            "user_messages_count": user_messages.len(),
            "recent_sessions_count": recent_sessions.len(),
        });
        let timing_str = serde_json::to_string_pretty(&timing)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "<!-- {} -->\n\n{}",
            timing_str, output
        ))]))
    }
}

#[tool_handler]
impl ServerHandler for TotalRecallServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}
