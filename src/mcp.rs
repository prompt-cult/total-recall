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
    #[schemars(
        description = "Max records to return. 0 = default 100, max 1000. Larger values rejected; page with offset/limit."
    )]
    #[serde(default)]
    pub limit: usize,
    #[schemars(
        description = "0-based start index into the chronological list. Omit = most recent `limit`; set 0 and follow bounds.next_offset to page the whole session."
    )]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(
        description = "Hard byte cap on the returned payload. 0 = default, clamped to the 8 MiB ceiling."
    )]
    #[serde(default)]
    pub max_bytes: usize,
    #[schemars(
        description = "Per-record clamp for content/thinking, bytes. 0 = default 262144 (256 KiB)."
    )]
    #[serde(default)]
    pub max_record_bytes: usize,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UserMessagesParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    pub session_id: String,
    #[schemars(description = "Max records to return. 0 = default 100, max 1000.")]
    #[serde(default)]
    pub limit: usize,
    #[schemars(
        description = "0-based start index. Omit = most recent `limit`; follow bounds.next_offset to page."
    )]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(description = "Hard byte cap on the returned payload. 0 = default 8 MiB ceiling.")]
    #[serde(default)]
    pub max_bytes: usize,
    #[schemars(description = "Per-record clamp, bytes. 0 = default 256 KiB.")]
    #[serde(default)]
    pub max_record_bytes: usize,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ExtractByTypeParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    pub session_id: String,
    #[schemars(
        description = "If true, read entire session. If false, read from last compaction point."
    )]
    #[serde(default)]
    pub full: bool,
    #[schemars(
        description = "Entry types to include: any of user|assistant|tool|thinking, or \"all\". Empty = all."
    )]
    #[serde(default)]
    pub types: Vec<String>,
    #[schemars(description = "Max records to return. 0 = default 100, max 1000.")]
    #[serde(default)]
    pub limit: usize,
    #[schemars(
        description = "0-based start index. Omit = most recent `limit`; follow bounds.next_offset to page."
    )]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(description = "Hard byte cap on the returned payload. 0 = default 8 MiB ceiling.")]
    #[serde(default)]
    pub max_bytes: usize,
    #[schemars(description = "Per-record clamp, bytes. 0 = default 256 KiB.")]
    #[serde(default)]
    pub max_record_bytes: usize,
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
        description = "Total-recall MCP tool: extract messages from a session as a bounded JSON envelope. Output is capped (default most-recent-100, 8 MiB ceiling); bounds carries next_offset to page large sessions and an explicit truncation notice."
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
        let limit = match crate::bound::normalize_limit(params.limit) {
            Ok(l) => l,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let max_bytes = crate::bound::normalize_max_bytes(params.max_bytes);
        let max_record_bytes = crate::bound::normalize_max_record_bytes(params.max_record_bytes);

        let messages = if params.full {
            adapter.read_session_mmap(&session_id)
        } else {
            adapter.read_session_from_compaction(&session_id)
        };

        let json = bounded_messages_envelope(
            &session_id,
            self.harness(),
            params.full,
            "messages",
            messages,
            limit,
            params.offset,
            max_bytes,
            max_record_bytes,
        );
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "Total-recall MCP tool: extract verbatim user messages from a session as a bounded JSON envelope. Output is capped (default most-recent-100, 8 MiB ceiling); bounds carries next_offset to page and an explicit truncation notice."
    )]
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
        let limit = match crate::bound::normalize_limit(params.limit) {
            Ok(l) => l,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let max_bytes = crate::bound::normalize_max_bytes(params.max_bytes);
        let max_record_bytes = crate::bound::normalize_max_record_bytes(params.max_record_bytes);

        // Derive from the bounded message stream so we bound during iteration
        // rather than materializing the full unbounded Vec first.
        let messages = adapter.read_session_mmap(&session_id);
        let user: Vec<crate::RolloutMessage> = messages
            .into_iter()
            .filter(|m| m.role == "user" && !m.injected)
            .collect();

        let json = bounded_messages_envelope(
            &session_id,
            self.harness(),
            false,
            "user_messages",
            user,
            limit,
            params.offset,
            max_bytes,
            max_record_bytes,
        );
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        name = "extract_by_type",
        description = "Total-recall MCP tool: raw extract of a session's entries filtered by type (user|assistant|tool|thinking, or \"all\"). Emits one record per line as `type,timestamp,json` with no summarization, under a hard byte cap (default most-recent-100, 8 MiB ceiling); a `#` header line carries bounds with next_offset and an explicit truncation notice. Recovers data from large or damaged sessions."
    )]
    async fn extract_by_type(
        &self,
        Parameters(params): Parameters<ExtractByTypeParams>,
    ) -> Result<CallToolResult, McpError> {
        // Validate type selection up front.
        const VALID: [&str; 4] = ["user", "assistant", "tool", "thinking"];
        let want_all = params.types.is_empty() || params.types.iter().any(|t| t == "all");
        if !want_all {
            for t in &params.types {
                if !VALID.contains(&t.as_str()) {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "extract_by_type: unknown type '{}'; valid: {} or \"all\"",
                        t,
                        VALID.join("|")
                    ))]));
                }
            }
        }

        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }
        let limit = match crate::bound::normalize_limit(params.limit) {
            Ok(l) => l,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let max_bytes = crate::bound::normalize_max_bytes(params.max_bytes);
        let max_record_bytes = crate::bound::normalize_max_record_bytes(params.max_record_bytes);

        let entries = adapter.read_session_entries(&session_id, params.full);
        let selected: Vec<String> = if want_all {
            VALID.iter().map(|s| s.to_string()).collect()
        } else {
            params.types.clone()
        };
        let filtered: Vec<&crate::rollout::RolloutEntry> = entries
            .iter()
            .filter(|e| want_all || params.types.contains(&e.entry_type))
            .collect();

        let text = extract_by_type_report(
            &session_id,
            self.harness(),
            params.full,
            &selected,
            &filtered,
            limit,
            params.offset,
            max_bytes,
            max_record_bytes,
        );
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
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

/// Build a bounded JSON envelope around a window of messages. Selects the
/// window (tail when `offset` is None, else an index range), clamps oversized
/// records, fits as many as the byte budget allows, and emits a pretty JSON
/// object `{ session_id, harness, full, bounds, <records_key>: [...] }` whose
/// serialized size never exceeds `max_bytes`. Truncation is always signalled
/// in `bounds.notice` / `bounds.truncated`.
#[allow(clippy::too_many_arguments)]
fn bounded_messages_envelope(
    session_id: &str,
    harness: &str,
    full: bool,
    records_key: &str,
    messages: Vec<crate::RolloutMessage>,
    limit: usize,
    offset: Option<usize>,
    max_bytes: usize,
    max_record_bytes: usize,
) -> String {
    let total = messages.len();
    let (start, end) = crate::bound::window(total, offset, limit);
    let windowed = &messages[start..end];

    // Clamp oversized records; remember which (window-relative) indices.
    let mut clamped_indices = Vec::new();
    let mut sized: Vec<crate::bound::SizedRecord> = Vec::with_capacity(windowed.len());
    for (i, m) in windowed.iter().cloned().enumerate() {
        let mut m = m;
        if crate::bound::clamp_message(&mut m, max_record_bytes) {
            clamped_indices.push(i);
        }
        let json = serde_json::to_string(&m).unwrap_or_else(|_| "{}".to_string());
        let bytes = json.len();
        sized.push(crate::bound::SizedRecord { json, bytes });
    }

    // Fit under the byte budget (reserving room for the envelope keys).
    let budget = max_bytes.saturating_sub(crate::bound::ENVELOPE_RESERVE);
    let fit = crate::bound::fit_count(&sized, budget);
    // record_limit truncation: the requested window could not be satisfied in
    // full — tail mode dropped leading records (start>0), or offset mode
    // couldn't reach the end (end<total).
    let record_limit_hit = start > 0 || end < total;
    let byte_cap_hit = fit < sized.len();
    let kept = &sized[..fit];
    let bytes: usize = kept.iter().map(|r| r.bytes).sum();

    let bounds = crate::bound::finalize_bounds(
        total,
        start,
        limit,
        fit,
        record_limit_hit,
        byte_cap_hit,
        clamped_indices,
        max_bytes,
        bytes,
    );

    let records: Vec<serde_json::Value> = kept
        .iter()
        .map(|r| serde_json::from_str(&r.json).unwrap_or(serde_json::Value::Null))
        .collect();

    let mut envelope = serde_json::json!({
        "session_id": session_id,
        "harness": harness,
        "full": full,
        "bounds": bounds,
    });
    envelope[records_key] = serde_json::Value::Array(records);

    serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string())
}

/// Assemble the `extract_by_type` line-format report. Line 1 is a `#`-prefixed
/// compact-JSON header carrying the bounds; each subsequent line is
/// `type,timestamp,json` (compact JSON, embedded newlines escaped so the
/// one-record-per-line invariant holds). On truncation a final `# TRUNCATED:`
/// line is appended. Never summarizes or aggregates.
#[allow(clippy::too_many_arguments)]
fn extract_by_type_report(
    session_id: &str,
    harness: &str,
    full: bool,
    selected_types: &[String],
    entries: &[&crate::rollout::RolloutEntry],
    limit: usize,
    offset: Option<usize>,
    max_bytes: usize,
    max_record_bytes: usize,
) -> String {
    let total = entries.len();
    let (start, end) = crate::bound::window(total, offset, limit);
    let windowed = &entries[start..end];

    // Serialize each record to a single line, clamping the JSON payload.
    let mut clamped_indices = Vec::new();
    let mut sized: Vec<crate::bound::SizedRecord> = Vec::with_capacity(windowed.len());
    for (i, e) in windowed.iter().enumerate() {
        let mut record_json = serde_json::to_string(&e.record).unwrap_or_else(|_| "{}".to_string());
        let mut clamped = false;
        if record_json.len() > max_record_bytes {
            record_json = crate::rollout::truncate_chars(&record_json, max_record_bytes).to_string();
            clamped = true;
        }
        if clamped {
            clamped_indices.push(i);
        }
        let line = format!(
            "{},{},{}",
            e.entry_type,
            crate::rollout::entry_timestamp(e.timestamp.as_deref()),
            record_json
        );
        let bytes = line.len();
        sized.push(crate::bound::SizedRecord { json: line, bytes });
    }

    let budget = max_bytes.saturating_sub(crate::bound::ENVELOPE_RESERVE);
    let fit = crate::bound::fit_count(&sized, budget);
    let record_limit_hit = start > 0 || end < total;
    let byte_cap_hit = fit < sized.len();
    let kept = &sized[..fit];
    let bytes: usize = kept.iter().map(|r| r.bytes).sum();

    let bounds = crate::bound::finalize_bounds(
        total,
        start,
        limit,
        fit,
        record_limit_hit,
        byte_cap_hit,
        clamped_indices,
        max_bytes,
        bytes,
    );

    let header = serde_json::json!({
        "session_id": session_id,
        "harness": harness,
        "full": full,
        "types": selected_types,
        "bounds": bounds,
    });
    let mut out = String::new();
    out.push('#');
    out.push_str(&serde_json::to_string(&header).unwrap_or_else(|_| "{}".to_string()));
    out.push('\n');
    for r in kept {
        out.push_str(&r.json);
        out.push('\n');
    }
    if bounds.truncated {
        out.push_str("# TRUNCATED: ");
        out.push_str(&bounds.notice);
        out.push('\n');
    }
    out
}
