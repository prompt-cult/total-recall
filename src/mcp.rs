use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
        ServerInfo,
    },
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};

use crate::{
    RolloutAdapter, build_structured_prompt,
    harness::make_adapter,
    mercury::provider_for,
    prompt::SYSTEM_PROMPT,
    recall::{
        GOALS_SYSTEM_PROMPT, MAX_GOALS_BYTES, MAX_STATE_BYTES, STATE_SYSTEM_PROMPT,
        build_goals_prompt_bounded, build_plan_files_section, build_recall_output,
        build_recent_rollouts_table, build_state_prompt_bounded,
    },
};

// --- Tool parameter structs ---
//
// Every struct here denies unknown fields and states its numeric bounds in the
// schema, because the handler enforces both: a typo (`hour_back`,
// `directorys`) is a silently unscoped call, and a bound the schema does not
// state is a bound the caller has to discover by being rejected. The numerics
// are signed so a negative arrives as a value the shared validator answers in
// the house form instead of dying mid-parse as a serde type error.

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListSessionsParams {
    #[schemars(
        description = "Only include sessions updated within this many hours. A window in hours, 1 or more; omitted or 0 = the default window (Default: 240 = 10 days).",
        range(min = 1)
    )]
    #[serde(default)]
    pub hours_back: Option<i64>,
    #[schemars(
        description = "The whole store, explicitly: every session of every project. Default false; with hours_back set it is rejected — pass one, not both."
    )]
    #[serde(default)]
    pub all: bool,
    #[schemars(description = "Only include sessions whose directory contains this substring.")]
    pub directory: Option<String>,
    #[schemars(
        description = "Flood-control cap on the returned listing, bytes: 1 or more, up to the 8 MiB ceiling. Default: 16384. The listing renders as many of the most recent rows of the window as this budget holds and states how many rows it held back.",
        range(min = 1, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default = "default_max_bytes")]
    pub max_bytes: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    #[serde(default)]
    pub session_id: String,
    #[schemars(
        description = "Opt-in on-disk profile cache: serve from <shadow_root>/tr_<session-id>_meta.json when fresh (15 s staleness tolerance); recompute and rewrite otherwise. Default false."
    )]
    #[serde(default)]
    pub cache: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    #[serde(default)]
    pub session_id: String,
    #[schemars(
        description = "If true, read entire session. If false, read from last compaction point."
    )]
    #[serde(default)]
    pub full: bool,
    #[schemars(
        description = "Max records to return. 0 = default 100, 1 to 1000. Larger values rejected; page with offset/limit.",
        range(min = 0, max = "crate::bound::MAX_RECORD_LIMIT")
    )]
    #[serde(default)]
    pub limit: i64,
    #[schemars(
        description = "0-based start index into the chronological list. Omit = most recent `limit`; set 0 and follow bounds.next_offset to page the whole session.",
        range(min = 0)
    )]
    #[serde(default)]
    pub offset: Option<i64>,
    #[schemars(
        description = "Hard byte cap on the returned payload. 0 = default, clamped to the 8 MiB ceiling.",
        range(min = 0, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default)]
    pub max_bytes: i64,
    #[schemars(
        description = "Per-record clamp for content/thinking, bytes. 0 = default 262144 (256 KiB), up to the 8 MiB ceiling.",
        range(min = 0, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default)]
    pub max_record_bytes: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserMessagesParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    #[serde(default)]
    pub session_id: String,
    #[schemars(
        description = "Max records to return. 0 = default 100, 1 to 1000.",
        range(min = 0, max = "crate::bound::MAX_RECORD_LIMIT")
    )]
    #[serde(default)]
    pub limit: i64,
    #[schemars(
        description = "0-based start index. Omit = most recent `limit`; follow bounds.next_offset to page.",
        range(min = 0)
    )]
    #[serde(default)]
    pub offset: Option<i64>,
    #[schemars(
        description = "Hard byte cap on the returned payload. 0 = default 8 MiB ceiling.",
        range(min = 0, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default)]
    pub max_bytes: i64,
    #[schemars(
        description = "Per-record clamp, bytes. 0 = default 256 KiB.",
        range(min = 0, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default)]
    pub max_record_bytes: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractByTypeParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    #[serde(default)]
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
    #[schemars(
        description = "Include injected (synthetic) user records. Default false: they are skipped as in user-message extraction."
    )]
    #[serde(default)]
    pub include_injected: bool,
    #[schemars(
        description = "Max records to return. 0 = default 100, 1 to 1000.",
        range(min = 0, max = "crate::bound::MAX_RECORD_LIMIT")
    )]
    #[serde(default)]
    pub limit: i64,
    #[schemars(
        description = "0-based start index. Omit = most recent `limit`; follow bounds.next_offset to page.",
        range(min = 0)
    )]
    #[serde(default)]
    pub offset: Option<i64>,
    #[schemars(
        description = "Hard byte cap on the returned payload. 0 = default 8 MiB ceiling.",
        range(min = 0, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default)]
    pub max_bytes: i64,
    #[schemars(
        description = "Per-record clamp, bytes. 0 = default 256 KiB.",
        range(min = 0, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default)]
    pub max_record_bytes: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    #[serde(default)]
    pub session_id: String,
    #[schemars(
        description = "If true, compact entire session. If false, compact from last compaction point."
    )]
    #[serde(default)]
    pub full: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TotalRecallParams {
    #[schemars(description = "Session ID (partial match). Empty = most recent.")]
    #[serde(default)]
    pub session_id: String,
    #[schemars(
        description = "Hours back to include in the recent rollouts table. A window in hours, 1 or more; omitted or 0 = the default window (Default: 24).",
        range(min = 1)
    )]
    #[serde(default = "default_hours")]
    pub hours_back: i64,
    #[schemars(
        description = "Flood-control cap on the returned report, bytes: 1 or more, up to the 8 MiB ceiling. Default: 16384. An overflowing report is written whole to a private temp file and the return carries an EOF marker with its line histogram.",
        range(min = 1, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default = "default_max_bytes")]
    pub max_bytes: i64,
}

fn default_hours() -> i64 {
    24
}

fn default_she_said_hours() -> u64 {
    48
}

/// The natural call indexes the last day, not the whole store; whole-store
/// indexing is `all: true`, explicit.
fn default_index_hours() -> u64 {
    24
}

fn default_sheep_hours() -> u64 {
    48
}

fn default_list_hours() -> u64 {
    240
}

fn default_max_bytes() -> i64 {
    crate::report_cap::DEFAULT_MAX_BYTES as i64
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheSaidHeSaidParams {
    #[schemars(
        description = "Session ID (partial match). Empty = the window (hours_back / directory / all)."
    )]
    #[serde(default)]
    pub session_id: String,
    #[schemars(description = "Case-insensitive search terms. At least one is required.")]
    pub words: Vec<String>,
    #[schemars(
        description = "Hours back when session_id is empty. A window in hours, 1 or more; omitted or 0 = the default window (Default: 48).",
        range(min = 1)
    )]
    #[serde(default)]
    pub hours_back: Option<i64>,
    #[schemars(
        description = "The whole store, explicitly: every session of every project. Default false; with hours_back set it is rejected — pass one, not both."
    )]
    #[serde(default)]
    pub all: bool,
    #[schemars(description = "Optional directory substring filter (empty-session-id mode).")]
    pub directory: Option<String>,
    #[schemars(
        description = "Flood-control cap on the returned report, bytes: 1 or more, up to the 8 MiB ceiling. Default: 16384. An overflowing report is written whole to a private temp file and the return carries an EOF marker with its line histogram.",
        range(min = 1, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default = "default_max_bytes")]
    pub max_bytes: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IndexSessionsParams {
    #[schemars(
        description = "Session ID (partial match). Empty = the window (hours_back / directory / all)."
    )]
    #[serde(default)]
    pub session_id: String,
    #[schemars(
        description = "Hours back when session_id is empty. A window in hours, 1 or more; omitted or 0 = the default window (Default: 24 = the last day).",
        range(min = 1)
    )]
    #[serde(default)]
    pub hours_back: Option<i64>,
    #[schemars(
        description = "The whole store, explicitly: every session of every project. Default false; with hours_back set it is rejected — pass one, not both."
    )]
    #[serde(default)]
    pub all: bool,
    #[schemars(description = "Optional directory substring filter (empty-session-id mode).")]
    pub directory: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SheepParams {
    #[schemars(description = "Tantivy query syntax. Required.")]
    pub query: String,
    #[schemars(
        description = "Session ID (partial match). Empty = the window (hours_back / directory / all)."
    )]
    #[serde(default)]
    pub session_id: String,
    #[schemars(
        description = "Hours back when session_id is empty. A window in hours, 1 or more; omitted or 0 = the default window (Default: 48).",
        range(min = 1)
    )]
    #[serde(default)]
    pub hours_back: Option<i64>,
    #[schemars(
        description = "The whole store, explicitly: every session of every project. Default false; with hours_back set it is rejected — pass one, not both."
    )]
    #[serde(default)]
    pub all: bool,
    #[schemars(description = "Optional directory substring filter (empty-session-id mode).")]
    pub directory: Option<String>,
    #[schemars(
        description = "Flood-control cap on the returned report, bytes: 1 or more, up to the 8 MiB ceiling. Default: 16384. An overflowing report is written whole to a private temp file and the return carries an EOF marker with its line histogram.",
        range(min = 1, max = "crate::bound::MAX_BYTES_CEILING")
    )]
    #[serde(default = "default_max_bytes")]
    pub max_bytes: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineHistogramParams {
    #[schemars(description = "Path of the file to profile. Required.")]
    pub file_path: String,
    #[schemars(
        description = "histogram (default) or extract: line-size distribution, or the line at `line` / the inclusive range `start`..=`end`. The line selectors need mode: extract; extract needs one of them."
    )]
    pub mode: Option<String>,
    #[schemars(
        description = "extract mode: single 1-based line number.",
        range(min = 1)
    )]
    pub line: Option<i64>,
    #[schemars(
        description = "extract mode: inclusive 1-based range start (with end).",
        range(min = 1)
    )]
    pub start: Option<i64>,
    #[schemars(
        description = "extract mode: inclusive 1-based range end (with start).",
        range(min = 1)
    )]
    pub end: Option<i64>,
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

/// The scope contract, enforced at the tool layer: validate `hours_back` in
/// the house form, then return the hours to hand the adapter — `0` means
/// unbounded at that mechanism layer and is reachable only through
/// `all: true` — or the rejection text.
///
/// `hours_back` is normalised after validation: `0` means "omitted" and takes
/// the tool's default window, because a client that serialises an unset
/// window as zero is asking the cheap question, not for the whole store. The
/// contradiction check runs on the normalised value, so `all: true` alongside
/// a filled-in `0` is the whole store the caller explicitly asked for, while
/// `all: true` with a real window is still rejected. The adapter's
/// `0` = unbounded stays the mechanism layer's own — the MCP layer never
/// passes `0` except through `all: true`.
fn window_hours(
    tool: &str,
    hours_back: Option<i64>,
    all: bool,
    default_hours: u64,
) -> Result<u64, String> {
    let hours_back = match hours_back {
        Some(hours) => Some(number(tool, "hours_back", hours, 0, i64::MAX, HOURS_CHEAP)?),
        None => None,
    };
    let hours_back = hours_back.filter(|h| *h >= 1);
    if all && hours_back.is_some() {
        return Err(
            "all: true is the whole store; hours_back is a window — pass one, not both".to_string(),
        );
    }
    Ok(if all {
        0
    } else {
        hours_back.map(|h| h as u64).unwrap_or(default_hours)
    })
}

/// The house form for a numeric input, one helper for every numeric parameter
/// of every tool: a value outside `[min, max]` is rejected by name, with the
/// cheap form spelled out, before any work starts. `cheap` is the caller's way
/// forward; `min`/`max` are the same bounds the schema states, so what a
/// client can send and what a handler accepts cannot drift. The value arrives
/// signed, so a negative is an answer rather than a serde type error.
fn number(
    tool: &str,
    field: &str,
    value: i64,
    min: i64,
    max: i64,
    cheap: &str,
) -> Result<i64, String> {
    if value < min || value > max {
        return Err(format!("{tool}: `{field}` is out of range — {cheap}"));
    }
    Ok(value)
}

/// `0` is "omitted" for the window, so the schema states `minimum: 1` and the
/// handler normalises instead of detonating: the caller gets the default
/// window it would have got by omitting the field.
const HOURS_CHEAP: &str = "pass hours_back >= 1, or omit it — 0 means the tool's default window, never the whole store (all: true is the whole store)";
const LIMIT_CHEAP: &str =
    "pass limit from 1 to 1000, or omit it for the default 100, and page with offset/limit";
const OFFSET_CHEAP: &str = "pass offset >= 0, or omit it for the most recent page";
const REPORT_MAX_BYTES_CHEAP: &str =
    "pass max_bytes from 1 to 8388608, or omit it for the default 16384";
const ENVELOPE_MAX_BYTES_CHEAP: &str =
    "pass max_bytes from 0 to 8388608, or omit it for the default 8 MiB ceiling";
const MAX_RECORD_BYTES_CHEAP: &str =
    "pass max_record_bytes from 1 to 8388608, or omit it (0 = no per-record clamp)";
const LINE_CHEAP: &str = "pass line >= 1 (1-based), or mode: extract with start and end";
const START_CHEAP: &str = "pass start >= 1 (1-based) with end, or mode: extract with line";
const END_CHEAP: &str = "pass end >= 1 (1-based) with start";

/// The bounded-extraction bounds of `extract_messages`,
/// `extract_user_messages` and `extract_by_type`, validated in the house form
/// and then normalised: `0` takes each bound's default, an over-ceiling value
/// is rejected rather than silently clamped down to a cap the caller never
/// asked for. Every numeric input is checked BEFORE the session is resolved, so
/// a bad bound is answered as a bound and never as "no sessions found".
fn envelope_bounds(
    tool: &str,
    limit: i64,
    offset: Option<i64>,
    max_bytes: i64,
    max_record_bytes: i64,
) -> Result<(usize, Option<usize>, usize, usize), String> {
    let limit = number(
        tool,
        "limit",
        limit,
        0,
        crate::bound::MAX_RECORD_LIMIT as i64,
        LIMIT_CHEAP,
    )
    .and_then(|l| crate::bound::normalize_limit(l as usize))?;
    let offset = match offset {
        Some(offset) => Some(number(tool, "offset", offset, 0, i64::MAX, OFFSET_CHEAP)? as usize),
        None => None,
    };
    let max_bytes = number(
        tool,
        "max_bytes",
        max_bytes,
        0,
        crate::bound::MAX_BYTES_CEILING as i64,
        ENVELOPE_MAX_BYTES_CHEAP,
    )
    .map(|v| crate::bound::normalize_max_bytes(v as usize))?;
    let max_record_bytes = number(
        tool,
        "max_record_bytes",
        max_record_bytes,
        0,
        crate::bound::MAX_BYTES_CEILING as i64,
        MAX_RECORD_BYTES_CHEAP,
    )
    .map(|v| crate::bound::normalize_max_record_bytes(v as usize))?;
    Ok((limit, offset, max_bytes, max_record_bytes))
}

/// Every tool's success text that can overflow WITHOUT its own bound passes
/// through here: flood control is generic and open-ended, so a tool added
/// tomorrow inherits the cap by calling this like every other tool does.
/// Under `max_bytes` the text is returned untouched; over it the whole text
/// is written to a private temp file and the return carries an EOF marker
/// with the line histogram.
///
/// The bounded-extraction tools (`extract_messages`,
/// `extract_user_messages`, `extract_by_type`) do NOT route through here:
/// their responses carry their own envelope — `limit`, `offset`, `max_bytes`
/// and `max_record_bytes` per call with an 8 MiB hard ceiling, explicit
/// truncation notices and `next_offset` paging — which is their flood
/// control. Double-capping would strangle the documented envelope contract.
fn capped_text(tool: &str, scope: &str, text: String, max_bytes: usize) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(crate::report_cap::cap_report(
        tool, scope, text, max_bytes,
    ))])
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
        Ok(capped_text(
            "harness",
            "harness",
            json,
            crate::report_cap::DEFAULT_MAX_BYTES,
        ))
    }

    #[tool(
        description = "Total-recall MCP tool: list all agent session rollouts for the bound harness, optionally bounded by hours_back (a window in hours, 1 or more; 0 or omitted = the 240-hour default) and a directory substring filter; all: true lists the whole store. The listing renders as many of the most recent rows as max_bytes holds and states the held-back count."
    )]
    async fn list_sessions(
        &self,
        Parameters(params): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_bytes = match number(
            "list_sessions",
            "max_bytes",
            params.max_bytes,
            1,
            crate::bound::MAX_BYTES_CEILING as i64,
            REPORT_MAX_BYTES_CHEAP,
        ) {
            Ok(v) => v as usize,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let adapter = self.adapter()?;
        let hours = match window_hours(
            "list_sessions",
            params.hours_back,
            params.all,
            default_list_hours(),
        ) {
            Ok(h) => h,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        // The scoped listing: the window and the directory substring are
        // applied where the data lives, at most the 200 most recent rows of the
        // window reach this layer, and the rows the window holds but did not
        // print are stated in the response itself.
        let listing = adapter.list_sessions_scoped(hours, params.directory.as_deref());
        let mut sessions = listing.sessions;
        crate::index::annotate_sessions(&mut sessions, adapter.as_ref());
        let json = listing_json(&sessions, listing.window_count, max_bytes);
        Ok(capped_text("list_sessions", "list", json, max_bytes))
    }

    #[tool(
        name = "she_said_he_said_action",
        description = "Total-recall MCP tool: given case-insensitive terms, extract per session the HE SAID (user text), SHE SAID (assistant text) and THEY DID (tool calls) matching any term, as a markdown report ordered most-recent session first. The session_id (partial match) selects one session, or, when empty, the window (hours_back, default 48 — 0 or omitted means that default; directory; or all: true). Matching runs inside SQLite on a read-only connection."
    )]
    async fn she_said_he_said_action(
        &self,
        Parameters(params): Parameters<SheSaidHeSaidParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_bytes = match number(
            "she_said_he_said_action",
            "max_bytes",
            params.max_bytes,
            1,
            crate::bound::MAX_BYTES_CEILING as i64,
            REPORT_MAX_BYTES_CHEAP,
        ) {
            Ok(v) => v as usize,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let adapter = self.adapter()?;
        if params.words.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "she_said_he_said_action requires at least one term in `words`".to_string(),
            )]));
        }
        let hours = match window_hours(
            "she_said_he_said_action",
            params.hours_back,
            params.all,
            default_she_said_hours(),
        ) {
            Ok(h) => h,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let sessions: Vec<String> = if params.session_id.is_empty() {
            Vec::new()
        } else {
            vec![params.session_id.clone()]
        };
        match adapter.she_said_he_said_action(
            &sessions,
            &params.words,
            hours,
            params.directory.as_deref(),
        ) {
            Ok(report) => Ok(capped_text(
                "she_said_he_said_action",
                &params.session_id,
                report,
                max_bytes,
            )),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }

    #[tool(
        description = "Total-recall MCP tool: build or refresh per-session tantivy full-text shadow indexes for the selected sessions. The session_id (partial match) selects one session, or, when empty, the window (hours_back, default 24 = the last day; directory; or all: true)."
    )]
    async fn index_sessions(
        &self,
        Parameters(params): Parameters<IndexSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let adapter = self.adapter()?;
        let hours = match window_hours(
            "index_sessions",
            params.hours_back,
            params.all,
            default_index_hours(),
        ) {
            Ok(h) => h,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let sessions: Vec<String> = if params.session_id.is_empty() {
            Vec::new()
        } else {
            vec![params.session_id.clone()]
        };
        let (selected, unmatched) = crate::index::select_sessions(
            adapter.as_ref(),
            &sessions,
            hours,
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
                Ok(stats) => lines.push(format!(
                    "indexed {} docs={}",
                    stats.session_id, stats.doc_count
                )),
                Err(e) => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(e)]));
                }
            }
        }
        Ok(capped_text(
            "index_sessions",
            &params.session_id,
            lines.join("\n"),
            crate::report_cap::DEFAULT_MAX_BYTES,
        ))
    }

    #[tool(
        name = "do_android_dream_of_electric_sheep",
        description = "Total-recall MCP tool: full-text search (tantivy) across per-session shadow indexes, merging top hits per session by score. The session_id (partial match) selects one session, or, when empty, the window (hours_back, default 48; directory; or all: true). Sessions without an index are reported as not indexed (run index_sessions first)."
    )]
    async fn do_android_dream_of_electric_sheep(
        &self,
        Parameters(params): Parameters<SheepParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_bytes = match number(
            "do_android_dream_of_electric_sheep",
            "max_bytes",
            params.max_bytes,
            1,
            crate::bound::MAX_BYTES_CEILING as i64,
            REPORT_MAX_BYTES_CHEAP,
        ) {
            Ok(v) => v as usize,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let adapter = self.adapter()?;
        if params.query.trim().is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "do_android_dream_of_electric_sheep requires a tantivy query in `query`"
                    .to_string(),
            )]));
        }
        let hours = match window_hours(
            "do_android_dream_of_electric_sheep",
            params.hours_back,
            params.all,
            default_sheep_hours(),
        ) {
            Ok(h) => h,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let sessions: Vec<String> = if params.session_id.is_empty() {
            Vec::new()
        } else {
            vec![params.session_id.clone()]
        };
        match crate::index::search(
            adapter.as_ref(),
            &sessions,
            &params.query,
            hours,
            params.directory.as_deref(),
        ) {
            Ok(report) => Ok(capped_text(
                "do_android_dream_of_electric_sheep",
                &params.session_id,
                report,
                max_bytes,
            )),
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
        let mut profile = match adapter.profile_session_opts(&session_id, params.cache) {
            Ok(p) => p,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        profile.has_tantivy_index = crate::index::index_exists(adapter.as_ref(), &session_id);
        let json = serde_json::to_string_pretty(&profile)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(capped_text(
            "profile_session",
            &session_id,
            json,
            crate::report_cap::DEFAULT_MAX_BYTES,
        ))
    }

    #[tool(
        description = "Total-recall MCP tool: extract messages from a session as a bounded JSON envelope. Output is capped (default most-recent-100, 8 MiB ceiling); bounds carries next_offset to page large sessions and an explicit truncation notice."
    )]
    async fn extract_messages(
        &self,
        Parameters(params): Parameters<ExtractParams>,
    ) -> Result<CallToolResult, McpError> {
        let (limit, offset, max_bytes, max_record_bytes) = match envelope_bounds(
            "extract_messages",
            params.limit,
            params.offset,
            params.max_bytes,
            params.max_record_bytes,
        ) {
            Ok(bounds) => bounds,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }

        let messages = match read_window(adapter.as_ref(), &session_id, params.full) {
            Ok(m) => m,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };

        let json = match bounded_messages_envelope(
            &session_id,
            self.harness(),
            params.full,
            "messages",
            messages,
            limit,
            offset,
            max_bytes,
            max_record_bytes,
        ) {
            Ok(json) => json,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        // Bounded extraction: the envelope is this tool's flood control
        // (limit/max_bytes/max_record_bytes + truncation notices); no cap.
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "Total-recall MCP tool: extract verbatim user messages from a session as a bounded JSON envelope. Output is capped (default most-recent-100, 8 MiB ceiling); bounds carries next_offset to page and an explicit truncation notice."
    )]
    async fn extract_user_messages(
        &self,
        Parameters(params): Parameters<UserMessagesParams>,
    ) -> Result<CallToolResult, McpError> {
        let (limit, offset, max_bytes, max_record_bytes) = match envelope_bounds(
            "extract_user_messages",
            params.limit,
            params.offset,
            params.max_bytes,
            params.max_record_bytes,
        ) {
            Ok(bounds) => bounds,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }

        // Derive from the bounded message stream so we bound during iteration
        // rather than materializing the full unbounded Vec first.
        let messages = match adapter.read_session_mmap(&session_id) {
            Ok(m) => m,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let user: Vec<crate::RolloutMessage> = messages
            .into_iter()
            .filter(|m| m.role == "user" && !m.injected)
            .collect();

        let json = match bounded_messages_envelope(
            &session_id,
            self.harness(),
            false,
            "user_messages",
            user,
            limit,
            offset,
            max_bytes,
            max_record_bytes,
        ) {
            Ok(json) => json,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        // Bounded extraction: the envelope is this tool's flood control.
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

        let (limit, offset, max_bytes, max_record_bytes) = match envelope_bounds(
            "extract_by_type",
            params.limit,
            params.offset,
            params.max_bytes,
            params.max_record_bytes,
        ) {
            Ok(bounds) => bounds,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }

        let entries =
            match adapter.read_session_entries(&session_id, params.full, params.include_injected) {
                Ok(e) => e,
                Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
            };
        let selected: Vec<String> = if want_all {
            VALID.iter().map(|s| s.to_string()).collect()
        } else {
            params.types.clone()
        };
        let filtered: Vec<&crate::rollout::RolloutEntry> = entries
            .iter()
            .filter(|e| want_all || params.types.contains(&e.entry_type))
            .collect();

        let text = match extract_by_type_report(
            &session_id,
            self.harness(),
            params.full,
            &selected,
            &filtered,
            limit,
            offset,
            max_bytes,
            max_record_bytes,
        ) {
            Ok(text) => text,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        // Bounded extraction: the envelope is this tool's flood control.
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(
        description = "Total-recall MCP tool: fast compaction of a session using the LLM vendor compiled in as the default (Mercury) — the server has no provider selector — returns a structured summary with Accomplished, Current Work, Files, Next Steps, and Key Decisions. Supplements the built-in slower compactions."
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
        let messages = match read_window(adapter.as_ref(), &session_id, params.full) {
            Ok(m) => m,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };

        if messages.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No messages found in session".to_string(),
            )]));
        }

        let prompt = build_structured_prompt(&messages);

        // A vendor-free build keeps this tool registered (clients bind by
        // name) and reports the missing feature as a tool error.
        let provider = match provider_for(None) {
            Ok(provider) => provider,
            Err(e) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "{e:#}"
                ))]));
            }
        };

        let summary = provider
            .compact(SYSTEM_PROMPT, &prompt)
            .await
            .map_err(|e| McpError::internal_error(format!("Mercury API error: {}", e), None))?;

        Ok(capped_text(
            "compact_session",
            &session_id,
            summary,
            crate::report_cap::DEFAULT_MAX_BYTES,
        ))
    }

    #[tool(
        name = "total_recall",
        description = "Total-recall MCP tool: fast compaction and log mining of session rollouts as a long-term memory store. Summarises the current session state, extracts user goals/tasks/steers, lists recent rollouts, and lists plan/todo files. Makes two parallel LLM calls and assembles a combined output to supplement the shorter faster compactions."
    )]
    async fn total_recall(
        &self,
        Parameters(params): Parameters<TotalRecallParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_bytes = match number(
            "total_recall",
            "max_bytes",
            params.max_bytes,
            1,
            crate::bound::MAX_BYTES_CEILING as i64,
            REPORT_MAX_BYTES_CHEAP,
        ) {
            Ok(v) => v as usize,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        // The rollouts table works a window like every other tool, so `0` here
        // means the default window and never the adapter's unbounded `0`.
        let hours = match window_hours(
            "total_recall",
            Some(params.hours_back),
            false,
            default_hours() as u64,
        ) {
            Ok(h) => h,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        let adapter = self.adapter()?;
        let session_id = crate::harness::resolve_session(adapter.as_ref(), &params.session_id)
            .unwrap_or_default();
        if session_id.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No sessions found".to_string(),
            )]));
        }

        // Read messages from last compaction point
        let messages = match adapter.read_session_from_compaction(&session_id) {
            Ok(m) => m,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        if messages.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "No messages found in session".to_string(),
            )]));
        }

        // Extract user messages from the full session
        let user_messages = match adapter.extract_user_messages(&session_id) {
            Ok(m) => m,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };

        // Build prompts. Both are byte-bounded: the newest context is kept, the
        // oldest dropped, and the drop reported in the timing comment below.
        let state_prompt = build_state_prompt_bounded(&messages, MAX_STATE_BYTES);
        let goals_prompt = build_goals_prompt_bounded(&user_messages, MAX_GOALS_BYTES);
        let prompt_stats = serde_json::json!({
            "state_prompt_bytes": state_prompt.bytes,
            "state_prompt_dropped_bytes": state_prompt.dropped_bytes,
            "state_prompt_dropped_messages": state_prompt.dropped_items,
            "goals_prompt_bytes": goals_prompt.bytes,
            "goals_prompt_dropped_bytes": goals_prompt.dropped_bytes,
            "goals_prompt_dropped_messages": goals_prompt.dropped_items,
        });
        let prompt_stats = prompt_stats.as_object().cloned().unwrap_or_default();

        // Create provider
        let provider = match provider_for(None) {
            Ok(provider) => provider,
            Err(e) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "{e:#}"
                ))]));
            }
        };

        // One bounded-concurrency batch call for both LLM calls
        let t0 = std::time::Instant::now();
        let batch_results = provider
            .compact_batch_pairs(vec![
                (STATE_SYSTEM_PROMPT.to_string(), state_prompt.text),
                (GOALS_SYSTEM_PROMPT.to_string(), goals_prompt.text),
            ])
            .await;
        let total_time = t0.elapsed();

        let [state_summary, goals_summary] = batch_results
            .map_err(|e| McpError::internal_error(format!("Mercury API error: {e:#}"), None))?
            .try_into()
            .expect("compact_batch_pairs returns one result per call");

        // Build recent rollouts table from the scoped listing: the window is
        // applied where the data lives and the table holds the listing's
        // bounded rows plus the held-back count.
        let listing = adapter.list_sessions_scoped(hours, None);
        let rollouts_table = build_recent_rollouts_table(&listing, Some(&session_id), hours);
        let table_rows = listing.sessions.len();

        // Build plan files section
        let plan_files = build_plan_files_section();

        // Assemble output
        let output =
            build_recall_output(&state_summary, &goals_summary, &rollouts_table, &plan_files);

        // Log timing info as JSON prefix (for debugging), carrying what the
        // prompt byte budgets dropped so a bounded prompt is never silent.
        let mut timing = serde_json::Map::from_iter([
            (
                "total_time_s".to_string(),
                serde_json::json!(total_time.as_secs_f64()),
            ),
            (
                "session_messages_count".to_string(),
                serde_json::json!(messages.len()),
            ),
            (
                "user_messages_count".to_string(),
                serde_json::json!(user_messages.len()),
            ),
            (
                "recent_sessions_count".to_string(),
                serde_json::json!(listing.window_count),
            ),
            (
                "rollout_table_rows".to_string(),
                serde_json::json!(table_rows),
            ),
        ]);
        timing.extend(prompt_stats);
        let timing_str = serde_json::to_string_pretty(&serde_json::Value::Object(timing))
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(capped_text(
            "total_recall",
            &session_id,
            format!("<!-- {} -->\n\n{}", timing_str, output),
            max_bytes,
        ))
    }

    #[tool(
        name = "line_histogram",
        description = "Total-recall MCP tool: profile a file by line-size distribution (histogram mode, ten buckets), or extract a line range (mode=extract with line, or start and end). Runs the vendored line_histogram.awk with a direct awk -f spawn. The paging companion for flood-control overflow files and any large dump on disk."
    )]
    async fn line_histogram(
        &self,
        Parameters(params): Parameters<LineHistogramParams>,
    ) -> Result<CallToolResult, McpError> {
        let selectors = match line_selectors(
            params.mode.as_deref(),
            params.line,
            params.start,
            params.end,
        ) {
            Ok(selectors) => selectors,
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        };
        match crate::report_cap::line_histogram(
            std::path::Path::new(&params.file_path),
            selectors.mode.as_deref(),
            selectors.line,
            selectors.start,
            selectors.end,
        ) {
            Ok(out) => Ok(capped_text(
                "line_histogram",
                &params.file_path,
                out,
                crate::report_cap::DEFAULT_MAX_BYTES,
            )),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }
}

#[tool_handler]
impl ServerHandler for TotalRecallServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    /// The one boundary every call crosses, so the parameter errors are stated
    /// in one voice. rmcp answers a rejected argument set with serde's own
    /// message; a typo is a silently unscoped call otherwise, so the message is
    /// rewritten here to name the unrecognised field and the fields the tool
    /// does accept — read from the served schema, which is the same authority
    /// `tools/list` publishes, so the list cannot drift from the struct.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let router = Self::tool_router();
        let tool = request.name.to_string();
        let accepted = accepted_fields(&router, &tool);
        let response = router
            .call(ToolCallContext::new(self, request, context))
            .await?;
        let CallToolResponse::Complete(mut result) = response else {
            return Ok(response);
        };
        if result.is_error == Some(true)
            && let Some(ContentBlock::Text(text)) = result.content.first_mut()
            && let Some(house) = house_form_params(&tool, &accepted, &text.text)
        {
            text.text = house;
        }
        Ok(CallToolResponse::Complete(result))
    }
}

/// rmcp's prefix on a rejected argument set: everything after it is serde's
/// own message, which is written for a serde user rather than for the agent
/// holding the call.
const PARAM_ERROR_PREFIX: &str = "failed to deserialize parameters:";

/// The house form for a rejected argument set, or `None` when the error is
/// not a parameter error and must be left alone.
fn house_form_params(tool: &str, accepted: &[String], raw: &str) -> Option<String> {
    let message = raw.trim_start_matches(PARAM_ERROR_PREFIX).trim();
    if message.len() == raw.len() && !raw.starts_with(PARAM_ERROR_PREFIX) {
        return None;
    }
    // serde appends " at line 1 column 42" to every message it produces from a
    // `serde_json::Value`; the position is noise to a caller holding a tool
    // call, and the house errors carry the position nowhere.
    let message = message
        .rsplit_once(" at line ")
        .filter(|(_, tail)| {
            tail.trim_end()
                .chars()
                .all(|c| c.is_ascii_digit() || c == ' ')
        })
        .map_or(message, |(head, _)| head);
    let accepts = format!("{tool} accepts: {}", accepted.join("|"));
    if let Some(field) = message
        .strip_prefix("unknown field ")
        .and_then(|rest| rest.split('`').nth(1))
    {
        return Some(format!(
            "{tool}: unrecognised field `{field}` — a typo is a call run on the wrong scope; {accepts}"
        ));
    }
    Some(format!("{tool}: {message} — {accepts}"))
}

/// The parameter names a tool's served schema declares, sorted. Read from the
/// schema the server publishes rather than from a second list kept beside the
/// struct.
fn accepted_fields(
    router: &rmcp::handler::server::router::tool::ToolRouter<TotalRecallServer>,
    tool: &str,
) -> Vec<String> {
    let mut fields: Vec<String> = router
        .get(tool)
        .and_then(|tool| tool.input_schema.get("properties"))
        .and_then(|props| props.as_object())
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default();
    fields.sort();
    fields
}

/// The validated `line_histogram` selectors. `mode` is an enum, not a free
/// string (the awk treats anything that is not `extract` as a histogram, so a
/// typo silently answers a different question), the line numbers are 1-based,
/// and the selector set is coherent: `extract` needs one selector, a range
/// needs both ends, `start` may not sit past `end`, and a selector without
/// `mode: extract` is a request the histogram mode would ignore.
struct LineSelectors {
    mode: Option<String>,
    line: Option<u64>,
    start: Option<u64>,
    end: Option<u64>,
}

fn line_selectors(
    mode: Option<&str>,
    line: Option<i64>,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<LineSelectors, String> {
    let mode = match mode {
        None | Some("") | Some("histogram") => "histogram",
        Some("extract") => "extract",
        Some(other) => {
            return Err(format!(
                "line_histogram: unknown mode `{other}` — pass histogram (the default) or extract"
            ));
        }
    };
    let line = line
        .map(|l| number("line_histogram", "line", l, 1, i64::MAX, LINE_CHEAP))
        .transpose()?
        .map(|l| l as u64);
    let start = start
        .map(|s| number("line_histogram", "start", s, 1, i64::MAX, START_CHEAP))
        .transpose()?
        .map(|s| s as u64);
    let end = end
        .map(|e| number("line_histogram", "end", e, 1, i64::MAX, END_CHEAP))
        .transpose()?
        .map(|e| e as u64);

    let extract = mode == "extract";
    if end.is_some() && start.is_none() {
        return Err(
            "line_histogram: `end` needs `start` — pass the inclusive range as start and end (start=1, end=40), or line for a single line"
                .to_string(),
        );
    }
    if let (Some(start), Some(end)) = (start, end)
        && start > end
    {
        return Err(format!(
            "line_histogram: `start` ({start}) is past `end` ({end}) — pass start <= end for an inclusive range"
        ));
    }
    if line.is_some() && start.is_some() {
        return Err(
            "line_histogram: `line` and `start`/`end` are two answers to one question — pass one: line for a single line, or start with end for a range"
                .to_string(),
        );
    }
    if extract && line.is_none() && start.is_none() {
        return Err(
            "line_histogram: mode: extract needs a line selector — pass line=42 for one line, or start=1 with end=40 for a range"
                .to_string(),
        );
    }
    if !extract && (line.is_some() || start.is_some()) {
        return Err(
            "line_histogram: `line`/`start`/`end` are extract-mode selectors — pass mode: extract with them, or drop them for the histogram"
                .to_string(),
        );
    }
    Ok(LineSelectors {
        mode: if extract {
            Some("extract".to_string())
        } else {
            None
        },
        line,
        start,
        end,
    })
}

/// The `list_sessions` response, with the row count derived from the response
/// budget: rows are rendered most recent first until the next one would push
/// the response past `max_bytes`, and the rows the window holds but did not
/// print are stated in `held_back`.
///
/// The budget is the one owner of this bound. A fixed row cap cannot be: a
/// `SessionSummary` is ~500 bytes pretty-printed, so 200 of them is ~100 KB
/// and the 16 KiB flood cap replaces the whole listing with a marker — a
/// default call that returns nothing usable. The row count is therefore
/// measured, not guessed: the candidate response is serialised exactly as it
/// is emitted, so the contract ("the listing fits `max_bytes`") holds by
/// construction rather than by arithmetic.
///
/// A single row larger than the whole budget is the one case the budget cannot
/// satisfy. There the full listing is rendered and handed to flood control,
/// which writes it to the overflow file and returns the marker with its
/// histogram — the escape hatch every other report has. Answering with an
/// empty table instead would hide the store behind a bound.
fn listing_json(
    sessions: &[crate::SessionSummary],
    window_count: usize,
    max_bytes: usize,
) -> String {
    let mut rendered: Vec<&crate::SessionSummary> = Vec::new();
    let mut json = listing_envelope(&rendered, window_count, max_bytes);
    for row in sessions {
        rendered.push(row);
        let candidate = listing_envelope(&rendered, window_count, max_bytes);
        if candidate.len() > max_bytes {
            rendered.pop();
            break;
        }
        json = candidate;
    }
    if rendered.is_empty() && !sessions.is_empty() {
        // No row fits the budget: flood control takes the whole listing.
        return listing_envelope(
            &sessions.iter().collect::<Vec<_>>(),
            window_count,
            max_bytes,
        );
    }
    json
}

/// The listing envelope, serialised exactly as it is emitted.
fn listing_envelope(
    rows: &[&crate::SessionSummary],
    window_count: usize,
    max_bytes: usize,
) -> String {
    let held_back = window_count.saturating_sub(rows.len());
    let notice = if held_back > 0 {
        format!(
            "TRUNCATED: rendered {} of the {window_count} rows the window holds under max_bytes {max_bytes}. Raise max_bytes for a bigger slice, or narrow the window with hours_back or directory.",
            rows.len()
        )
    } else {
        String::new()
    };
    serde_json::to_string_pretty(&serde_json::json!({
        "sessions": rows,
        "window_count": window_count,
        "held_back": held_back,
        "max_bytes": max_bytes,
        "notice": notice,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

/// Read a session window respecting `full`, propagating read damage as a tool
/// error. A payload that cannot be read is never reported as an empty session.
fn read_window(
    adapter: &dyn RolloutAdapter,
    session_id: &str,
    full: bool,
) -> Result<Vec<crate::RolloutMessage>, String> {
    if full {
        adapter.read_session_mmap(session_id)
    } else {
        adapter.read_session_from_compaction(session_id)
    }
}

/// Build a bounded JSON envelope around a window of messages. Selects the
/// window (tail when `offset` is None, else an index range), clamps oversized
/// records, fits as many as the byte budget allows, and emits a compact JSON
/// object `{ session_id, harness, full, bounds, <records_key>: [...] }` whose
/// serialized size never exceeds `max_bytes` (bounding is computed on the same
/// compact serialization that is emitted, so pretty-printing cannot inflate
/// the payload past the contract). Truncation is always signalled in
/// `bounds.notice` / `bounds.truncated`. Errors when even one record cannot
/// fit under the budget (fix: raise `max_bytes` or enable the clamp).
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
) -> Result<String, String> {
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
    if sized
        .first()
        .is_some_and(|r| r.bytes.saturating_add(1) > budget)
    {
        return Err(format!(
            "even one record ({}) exceeds max_bytes ({}) after the envelope reserve; raise max_bytes or set max_record_bytes to enable the clamp",
            sized[0].bytes, max_bytes
        ));
    }
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

    Ok(serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string()))
}

/// Assemble the `extract_by_type` line-format report. Line 1 is a `#`-prefixed
/// compact-JSON header carrying the bounds; each subsequent line is
/// `type,timestamp,json` (compact JSON, embedded newlines escaped so the
/// one-record-per-line invariant holds). On truncation a final `# TRUNCATED:`
/// line is appended. Never summarizes or aggregates. Errors when even one
/// record cannot fit under the budget (fix: raise `max_bytes` or enable the
/// clamp).
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
) -> Result<String, String> {
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
            record_json =
                crate::rollout::truncate_chars(&record_json, max_record_bytes).to_string();
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
    if sized
        .first()
        .is_some_and(|r| r.bytes.saturating_add(1) > budget)
    {
        return Err(format!(
            "even one record ({}) exceeds max_bytes ({}) after the envelope reserve; raise max_bytes or set max_record_bytes to enable the clamp",
            sized[0].bytes, max_bytes
        ));
    }
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
    Ok(out)
}
