pub mod claude;
pub mod codex;
pub mod mock;
pub mod opencode;
pub mod vibe;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::redact::redact_secrets;

/// The single error vocabulary for every adapter read path. A read that cannot
/// prove a session is empty returns one of these instead of an empty message
/// list, so "unreadable" can never be mistaken for "nothing found".
pub type ReadResult<T> = Result<T, String>;

/// Render a rollout-payload I/O failure, naming the path: the user has to be
/// able to see which file the tool could not read.
pub fn read_error(path: &Path, err: &std::io::Error) -> String {
    format!("cannot read rollout payload {}: {}", path.display(), err)
}

/// Render a failure to memory-map a rollout payload, naming the path.
pub fn mmap_error(path: &Path, err: &std::io::Error) -> String {
    format!("cannot mmap rollout payload {}: {}", path.display(), err)
}

/// Render an unresolvable session id against a store root, naming both.
pub fn no_session_error(root: &Path, session_id: &str) -> String {
    format!(
        "no session matching '{}' under {}",
        session_id,
        root.display()
    )
}

/// A single message from a rollout, normalized across harness formats.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RolloutMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub thinking: Option<String>,
    pub tool_calls_summary: Vec<String>,
    pub timestamp: Option<String>,
    pub injected: bool,
}

/// A raw typed entry for `extract_by_type`: one source record with its type
/// (`user` | `assistant` | `tool` | `thinking`), timestamp, and the record
/// itself as JSON. No summarization or aggregation is applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RolloutEntry {
    pub index: usize,
    pub entry_type: String,
    pub timestamp: Option<String>,
    pub record: serde_json::Value,
}

/// Derive typed entries from the normalized message stream (default source for
/// `read_session_entries`). Each message maps to its role type; a non-empty
/// `thinking` payload adds a separate `thinking` entry. Callers pre-filter
/// injected messages (see `RolloutAdapter::read_session_entries`).
pub fn entries_from_messages<'a>(
    messages: impl IntoIterator<Item = &'a RolloutMessage>,
) -> Vec<RolloutEntry> {
    let mut out = Vec::new();
    for m in messages {
        let entry_type = match m.role.as_str() {
            "user" | "assistant" | "tool" => m.role.clone(),
            _ => continue,
        };
        out.push(RolloutEntry {
            index: out.len(),
            entry_type,
            timestamp: m.timestamp.clone(),
            record: serde_json::to_value(m).unwrap_or(serde_json::Value::Null),
        });
        if let Some(t) = &m.thinking
            && !t.is_empty()
        {
            out.push(RolloutEntry {
                index: out.len(),
                entry_type: "thinking".to_string(),
                timestamp: m.timestamp.clone(),
                record: serde_json::json!({ "role": m.role, "thinking": t, "timestamp": m.timestamp }),
            });
        }
    }
    out
}

/// Render an entry timestamp for the `type,timestamp,json` line format:
/// ISO8601 when it parses, else a unix-epoch integer string, else "0".
pub fn entry_timestamp(ts: Option<&str>) -> String {
    match ts {
        Some(s) if is_iso8601(s) => s.to_string(),
        Some(s) if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() => s.to_string(),
        _ => "0".to_string(),
    }
}

/// One selectable option of a question asked through a harness question tool
/// (opencode `question`, vibe `ask_user_question`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct QaOption {
    pub label: String,
    pub description: String,
}

/// One question/answer pair extracted from a session: the question as the
/// harness asked it, its header and options where the format carries them,
/// and the answer(s) the user picked or typed. A question call that was
/// never answered, or whose prompt was cancelled, yields no pair.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct QaPair {
    pub question: String,
    pub header: Option<String>,
    pub options: Vec<QaOption>,
    pub answers: Vec<String>,
    pub timestamp: Option<String>,
}

/// One parsed answer record from a question tool's answer payload: the
/// answer text, plus the question it names when the payload carries it.
pub(crate) struct QaAnswerRecord {
    pub question: Option<String>,
    pub answer: String,
}

/// One question parsed out of a question tool call's arguments: the text,
/// its header when present, and the options offered.
pub(crate) struct ParsedQuestion {
    pub question: String,
    pub header: Option<String>,
    pub options: Vec<QaOption>,
}

/// The args JSON of a question tool call, from its normalized summary line
/// (`question({...})` / `ask_user_question({...})`). `None` for any other
/// tool.
fn question_call_args(summary: &str) -> Option<&str> {
    if !summary.starts_with("question(") && !summary.starts_with("ask_user_question(") {
        return None;
    }
    let open = summary.find('(')?;
    let args = &summary[open + 1..];
    let args = args.strip_suffix(')').unwrap_or(args);
    Some(args)
}

/// Parse the `questions` payload of a question tool call's arguments. Empty
/// when the arguments are not a recognizable question payload.
pub(crate) fn parse_questions_payload(args: &str) -> Vec<ParsedQuestion> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(args) else {
        return out;
    };
    let Some(questions) = v.get("questions").and_then(|q| q.as_array()) else {
        return out;
    };
    for q in questions {
        let Some(question) = q.get("question").and_then(|t| t.as_str()) else {
            continue;
        };
        let header = q.get("header").and_then(|t| t.as_str()).map(String::from);
        let options = q
            .get("options")
            .and_then(|o| o.as_array())
            .map(|opts| {
                opts.iter()
                    .filter_map(|o| {
                        Some(QaOption {
                            label: o.get("label")?.as_str()?.to_string(),
                            description: o
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(ParsedQuestion {
            question: question.to_string(),
            header,
            options,
        });
    }
    out
}

/// One quoted string from a python-ish literal: `s` starts with `'` or `"`,
/// backslash escapes the next character. Returns the decoded string and the
/// rest of the input.
pub(crate) fn parse_quoted_string(s: &str) -> Option<(String, &str)> {
    let quote = s.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[1..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                let (_, escaped) = chars.next()?;
                out.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                });
            }
            _ if c == quote => return Some((out, &s[1 + i + c.len_utf8()..])),
            _ => out.push(c),
        }
    }
    None
}

/// One bare python-ish scalar (True/False/None, a number): the text up to the
/// next `,`, `]`, `}` or end. Returns it and the rest of the input.
fn parse_bare_value(s: &str) -> Option<(String, &str)> {
    let end = s.find([',', ']', '}']).unwrap_or(s.len());
    Some((s[..end].trim().to_string(), &s[end..]))
}

/// One key/value pair of a python-ish dict: `'key': value`.
fn parse_dict_pair(s: &str) -> Option<(String, String, &str)> {
    let (key, rest) = parse_quoted_string(s.trim_start())?;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    if let Some((value, rest)) = parse_quoted_string(rest) {
        return Some((key, value, rest));
    }
    let (value, rest) = parse_bare_value(rest)?;
    Some((key, value, rest))
}

/// The `{...}` dict bodies at the top level of a python-ish list body, in
/// order, string-aware so a `}` inside a value never splits a dict.
fn python_dict_bodies(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' => {
                if let Some((_, rest)) = parse_quoted_string(&body[i..]) {
                    i = body.len() - rest.len();
                    continue;
                }
                i += 1;
            }
            b'{' => {
                let start = i;
                let mut depth = 1usize;
                let mut j = i + 1;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        b'\'' | b'"' => {
                            if let Some((_, rest)) = parse_quoted_string(&body[j..]) {
                                j = body.len() - rest.len();
                                continue;
                            }
                            j += 1;
                        }
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                if depth == 0 {
                    out.push(&body[start + 1..j - 1]);
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    out
}

/// Parse the python-ish `answers: [...]` / `cancelled: ...` text a question
/// tool writes into its result content: the answer records (question text
/// when the payload carries it) and whether the prompt was cancelled.
pub(crate) fn parse_answers_text(content: &str) -> (Vec<QaAnswerRecord>, bool) {
    let cancelled = content
        .split_once("cancelled:")
        .map(|(_, tail)| tail.trim_start().starts_with(['T', 't']))
        .unwrap_or(false);

    let Some((_, tail)) = content.split_once("answers:") else {
        return (Vec::new(), cancelled);
    };
    let Some(open) = tail.find('[') else {
        return (Vec::new(), cancelled);
    };
    let body = &tail[open + 1..];
    let close = match body.rfind(']') {
        Some(c) => c,
        None => return (Vec::new(), cancelled),
    };
    let mut records = Vec::new();
    for dict in python_dict_bodies(&body[..close]) {
        let mut question = None;
        let mut answer = None;
        let mut rest = dict;
        while !rest.trim().is_empty() {
            let Some((key, value, after)) = parse_dict_pair(rest) else {
                break;
            };
            match key.as_str() {
                "question" => question = Some(value),
                "answer" => answer = Some(value),
                _ => {}
            }
            rest = after.trim_start();
            rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
        }
        if let Some(answer) = answer {
            records.push(QaAnswerRecord { question, answer });
        }
    }
    (records, cancelled)
}

/// Pair each parsed question with its answer record: the record naming the
/// question wins, the positionally matching record is the fallback. A
/// question with no record yields no pair; with no parsed questions at all,
/// each record is its own pair (the question text from the record, or the
/// raw summary line when the record carries none).
pub(crate) fn pair_questions_with_answers(
    questions: &[ParsedQuestion],
    records: &[QaAnswerRecord],
    fallback_question: Option<&str>,
    timestamp: Option<String>,
) -> Vec<QaPair> {
    let mut pairs = Vec::new();
    if questions.is_empty() {
        for record in records {
            pairs.push(QaPair {
                question: record
                    .question
                    .clone()
                    .or_else(|| fallback_question.map(String::from))
                    .unwrap_or_default(),
                header: None,
                options: Vec::new(),
                answers: vec![record.answer.clone()],
                timestamp: timestamp.clone(),
            });
        }
        return pairs;
    }
    for (i, q) in questions.iter().enumerate() {
        let record = records
            .iter()
            .find(|r| r.question.as_deref() == Some(q.question.as_str()))
            .or_else(|| records.get(i));
        let Some(record) = record else { continue };
        pairs.push(QaPair {
            question: q.question.clone(),
            header: q.header.clone(),
            options: q.options.clone(),
            answers: vec![record.answer.clone()],
            timestamp: timestamp.clone(),
        });
    }
    pairs
}

/// The default Q/A extraction over the normalized message stream: an
/// assistant tool-call summary starting `question(`/`ask_user_question(`
/// pairs against the next tool-role message, whose content is parsed as the
/// python-ish `answers: [...]` text a question tool writes. Cancelled and
/// unpaired calls yield nothing. Harnesses whose formats carry the full
/// question payload natively (opencode, vibe) override this for headers,
/// options and multi-select answers; harnesses without question tools
/// answer an empty list, never an error.
pub(crate) fn qa_pairs_from_messages(messages: &[RolloutMessage]) -> Vec<QaPair> {
    let mut pairs = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg.role != "assistant" {
            continue;
        }
        for summary in &msg.tool_calls_summary {
            let Some(args) = question_call_args(summary) else {
                continue;
            };
            let Some(tool_msg) = messages[i + 1..].iter().find(|m| m.role == "tool") else {
                continue;
            };
            let (records, cancelled) = parse_answers_text(&tool_msg.content);
            if cancelled || records.is_empty() {
                continue;
            }
            let questions = parse_questions_payload(args);
            pairs.extend(pair_questions_with_answers(
                &questions,
                &records,
                Some(summary.as_str()),
                msg.timestamp.clone(),
            ));
        }
    }
    pairs
}

/// Trait abstracting over different CLI tool session formats.
pub trait RolloutAdapter: Send + Sync {
    /// Name of the harness ("vibe", "codex", "claude", "opencode")
    fn name(&self) -> &'static str;

    /// Find all rollout sessions, return summary info
    fn list_sessions(&self) -> Vec<SessionSummary>;

    /// The bounded form of [`RolloutAdapter::list_sessions`]: the window
    /// (`hours_back`, in hours; `0` is no bound at this mechanism layer —
    /// policy on what a caller must pass lives above it) and the optional
    /// `directory` substring are applied where the data lives, at most
    /// [`LISTING_ROW_CAP`] most-recent rows are returned, and `window_count`
    /// says how many rows the window holds in total — the count a caller
    /// states as held back when it cannot print them all.
    fn list_sessions_scoped(&self, hours_back: u64, directory: Option<&str>) -> SessionListing;

    /// Stream messages from a specific session. Errors when the session cannot
    /// be resolved or its payload cannot be read — never an empty list.
    fn read_session(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>>;

    /// Read session using mmap for maximum throughput. Errors when the session
    /// cannot be resolved or its payload cannot be opened or mapped.
    fn read_session_mmap(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>>;

    /// Read messages from the last compaction point onward.
    /// If no compaction marker exists, returns all messages.
    fn read_session_from_compaction(&self, session_id: &str) -> ReadResult<Vec<RolloutMessage>>;

    /// Profile a session: file size, line count, role counts, interesting events
    fn profile_session(&self, session_id: &str) -> ReadResult<SessionProfile> {
        self.profile_session_opts(session_id, false)
    }

    /// Most recent session id. Default scans the full session list; adapters
    /// with a cheap store-side index override this.
    fn most_recent_session_id(&self) -> Option<String> {
        self.list_sessions()
            .into_iter()
            .next()
            .map(|s| s.session_id)
    }

    /// `profile_session` with the opt-in on-disk profile cache. `cache = true`
    /// serves the profile from `<shadow_root>/tr_<session-id>_meta.json` when
    /// it is fresh (staleness checked against this adapter's cheap
    /// time-updated source, 15 s tolerance); otherwise the profile is computed
    /// as today and the cache is rewritten. Errors when the session cannot be
    /// resolved or its payload cannot be read — never an all-zero profile.
    fn profile_session_opts(&self, session_id: &str, cache: bool) -> ReadResult<SessionProfile>;

    /// Extract only user messages (verbatim)
    fn extract_user_messages(&self, session_id: &str) -> ReadResult<Vec<String>>;

    /// Raw typed entries for `extract_by_type`: one record per user message,
    /// assistant message, tool call/result, or thinking entry, preserving the
    /// source record where the adapter can reach it. `full` reads the entire
    /// session; `false` reads from the last compaction point. Injected
    /// (synthetic) user messages are skipped unless `include_injected`.
    /// Default derives entries from the normalized message stream; adapters
    /// with access to the native format override for byte-faithful records.
    fn read_session_entries(
        &self,
        session_id: &str,
        full: bool,
        include_injected: bool,
    ) -> ReadResult<Vec<RolloutEntry>> {
        let messages = if full {
            self.read_session_mmap(session_id)
        } else {
            self.read_session_from_compaction(session_id)
        }?;
        let selected: Vec<&RolloutMessage> = messages
            .iter()
            .filter(|m| include_injected || !m.injected)
            .collect();
        Ok(entries_from_messages(selected.iter().copied()))
    }

    /// Question/answer pairs of a session, chronologically: each question a
    /// harness question tool asked (opencode `question`, vibe
    /// `ask_user_question`) paired with the answer(s) the user selected or
    /// typed. The default pairs the normalized `question(...)` summary line
    /// against the next tool-role message; adapters with native access to
    /// the question payload override for headers, options and multi-select
    /// answers. A session without question calls reads as an empty list,
    /// never an error.
    fn read_qa_pairs(&self, session_id: &str) -> ReadResult<Vec<QaPair>> {
        let messages = self.read_session(session_id)?;
        Ok(qa_pairs_from_messages(&messages))
    }

    /// Every `todowrite` flush of a session, in order, for `todo_history`:
    /// the whole todo list as each flush wrote it, with the ISO8601
    /// timestamp of the message that carried it. `full` reads the entire
    /// session; `false` reads from the last compaction point. The default
    /// refuses: a harness without native todowrite access must say so,
    /// never return an empty list that reads as "no todos".
    fn read_todo_writes(
        &self,
        _session_id: &str,
        _full: bool,
    ) -> ReadResult<Vec<crate::todo_history::TodoWrite>> {
        Err(format!(
            "todo_history is only available for the opencode harness (this is the {} harness)",
            self.name()
        ))
    }

    /// Case-insensitive term-matched dialogue and tool actions, as a markdown
    /// report of HE SAID (user text), SHE SAID (assistant text) and THEY DID
    /// (tool calls). Harnesses without a native implementation return a clear
    /// unsupported error.
    /// Root directory for the disposable per-session full-text shadow index
    /// (`<root>/<session_id>/`). Sibling of the rollout store, never inside it.
    fn shadow_index_root(&self) -> PathBuf;

    /// Whether this adapter's storage root came from its `TOTAL_RECALL_<H>_ROOT`
    /// environment override (true) or the `$HOME`-derived live default (false).
    /// Used by the sandbox guard; adapters that take an explicit root (`with_root`,
    /// mock) report true so they are never refused.
    fn root_is_from_env(&self) -> bool {
        true
    }

    fn she_said_he_said_action(
        &self,
        _sessions: &[String],
        _words: &[String],
        _hours_back: u64,
        _directory: Option<&str>,
    ) -> Result<String, String> {
        Err(format!(
            "she_said_he_said_action is not implemented for harness '{}'; supported: opencode",
            self.name()
        ))
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub file_size: u64,
    pub line_count: u64,
    pub user_count: u64,
    pub assistant_count: u64,
    pub tool_count: u64,
    pub has_compaction: bool,
    pub directory: Option<String>,
    pub parent_session_id: Option<String>,
    pub child_sessions: Vec<String>,
    pub has_tantivy_index: bool,
    /// Other directory/session names that resolve to the same underlying
    /// rollout payload as this canonical entry (e.g. a resumed session that
    /// kept its content but got a new directory name). Empty when the entry
    /// is unique. Lets callers discover aliases without a second selectable
    /// row that would double-process the store.
    pub aliases: Vec<String>,
    /// Set when the rollout payload could not be read (permissions, I/O
    /// error) — the entry is still listed so damage is visible, with counts
    /// from whatever could be read. Absent for a healthy payload, and also
    /// absent when the payload file simply does not exist yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_error: Option<String>,
}

/// The per-row cap on navigation ids a rendered listing row carries: the most
/// [`IDS_PER_LISTING_ROW`] child ids and the most [`IDS_PER_LISTING_ROW`]
/// alias names, with the true count of each beside them.
///
/// Ten is the value because of what it buys and what it costs. Ten ids are
/// ~350 bytes pretty-printed — enough for a caller to see a subagent fan-out
/// or an alias set and navigate from it — while a row that carries them stays
/// under a kilobyte, so the worst one row in a store can do to a 16 KiB budget
/// is a rounding error. A larger cap buys ids the caller already has another
/// route to: `parent_session_id` walks up, `child_session_count` says how deep
/// the fan-out goes, and the counts plus a narrower window get the rest.
pub const IDS_PER_LISTING_ROW: usize = 10;

/// A rendered listing row: every navigational field of a [`SessionSummary`],
/// with its two unbounded id arrays bounded to [`IDS_PER_LISTING_ROW`] and the
/// truth carried alongside in `child_session_count`, `alias_count` and
/// `notice`.
///
/// [`SessionSummary`] keeps every id the store produced — the recall table,
/// the profile and the shadow-index annotation all read the full set — so this
/// is the one place the listing's byte cost per row is expressed. An unbounded
/// array in a store that accumulates subagent children is not a slow query, it
/// is a row that can spend the whole response budget on its own.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ListingRow {
    pub session_id: String,
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub file_size: u64,
    pub line_count: u64,
    pub user_count: u64,
    pub assistant_count: u64,
    pub tool_count: u64,
    pub has_compaction: bool,
    pub directory: Option<String>,
    pub parent_session_id: Option<String>,
    /// The most recent child ids, at most [`IDS_PER_LISTING_ROW`] of them.
    pub child_sessions: Vec<String>,
    /// How many child ids the row's session actually has. Absent when the
    /// session has none — an empty array says that already.
    #[serde(skip_serializing_if = "is_zero_count")]
    pub child_session_count: usize,
    pub has_tantivy_index: bool,
    /// The alias names, at most [`IDS_PER_LISTING_ROW`] of them.
    pub aliases: Vec<String>,
    /// How many alias names the row's session actually has. Absent when the
    /// entry is unique — an empty array says that already.
    #[serde(skip_serializing_if = "is_zero_count")]
    pub alias_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_error: Option<String>,
    /// What this row held back, in the row itself: a caller reading one row
    /// learns that it is a partial view rather than mistaking a bounded array
    /// for the whole fan-out. Absent when nothing was held back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

fn is_zero_count(count: &usize) -> bool {
    *count == 0
}

impl ListingRow {
    /// Bound one summary's id arrays into a row. Cheap: at most
    /// `2 * IDS_PER_LISTING_ROW` ids are copied regardless of how many the
    /// session has.
    pub fn new(summary: &SessionSummary) -> Self {
        let (child_sessions, child_session_count) = bounded_ids(&summary.child_sessions);
        let (aliases, alias_count) = bounded_ids(&summary.aliases);
        let mut notice = Vec::new();
        if child_session_count > child_sessions.len() {
            notice.push(format!(
                "{} of {child_session_count} child_sessions not shown",
                child_session_count - child_sessions.len()
            ));
        }
        if alias_count > aliases.len() {
            notice.push(format!(
                "{} of {alias_count} aliases not shown",
                alias_count - aliases.len()
            ));
        }
        Self {
            session_id: summary.session_id.clone(),
            title: summary.title.clone(),
            start_time: summary.start_time.clone(),
            end_time: summary.end_time.clone(),
            file_size: summary.file_size,
            line_count: summary.line_count,
            user_count: summary.user_count,
            assistant_count: summary.assistant_count,
            tool_count: summary.tool_count,
            has_compaction: summary.has_compaction,
            directory: summary.directory.clone(),
            parent_session_id: summary.parent_session_id.clone(),
            child_sessions,
            child_session_count,
            has_tantivy_index: summary.has_tantivy_index,
            aliases,
            alias_count,
            read_error: summary.read_error.clone(),
            notice: (!notice.is_empty()).then(|| notice.join("; ")),
        }
    }
}

/// The leading [`IDS_PER_LISTING_ROW`] ids and the count they were cut from.
fn bounded_ids(ids: &[String]) -> (Vec<String>, usize) {
    let kept = IDS_PER_LISTING_ROW.min(ids.len());
    (ids[..kept].to_vec(), ids.len())
}

/// The row cap a listing query returns: at most the 200 most recent rows of
/// the requested window, most recent first. One bound, one owner at this
/// layer — the MCP listing and the recall table's rollouts listing both take
/// their cap from here, so the work a listing does can never grow with the
/// store. The MCP listing then renders as many of those rows as its response
/// budget holds, so the cap is the ceiling and the byte budget is what the
/// caller actually sees.
pub const LISTING_ROW_CAP: usize = 200;

/// A bounded listing: at most [`LISTING_ROW_CAP`] rows of the requested
/// window, most recent first, plus how many rows the window holds in total
/// — so a caller that cannot print them all can state the held-back count
/// instead of silently dropping rows.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionListing {
    pub sessions: Vec<SessionSummary>,
    pub window_count: usize,
}

/// The window test for file-based stores: `hours_back == 0` is no bound;
/// otherwise the payload's mtime must fall inside the window. An mtime that
/// cannot be determined passes — a session whose recency is unknown stays
/// visible, the same safe default the ISO-timestamp filters keep.
pub(crate) fn mtime_in_window(mtime: Option<std::time::SystemTime>, hours_back: u64) -> bool {
    let Some(cutoff) = std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(
        hours_back.saturating_mul(3600),
    )) else {
        return true;
    };
    hours_back == 0 || mtime.is_none_or(|m| m >= cutoff)
}

/// The directory substring test shared by the scoped listings: an absent or
/// empty filter matches everything, and a session with no directory passes —
/// it cannot contradict the filter.
pub(crate) fn directory_matches(filter: Option<&str>, directory: Option<&str>) -> bool {
    filter
        .filter(|f| !f.is_empty())
        .is_none_or(|f| directory.is_none_or(|d| d.contains(f)))
}

/// Assemble the bounded listing from one window's rows keyed by mtime:
/// sorted most-recent-first, capped at [`LISTING_ROW_CAP`], with the full
/// window count reported so held-back rows are stated, not silent.
pub(crate) fn listing_by_mtime(
    mut rows: Vec<(Option<std::time::SystemTime>, SessionSummary)>,
) -> SessionListing {
    rows.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    let window_count = rows.len();
    rows.truncate(LISTING_ROW_CAP);
    SessionListing {
        sessions: rows.into_iter().map(|(_, s)| s).collect(),
        window_count,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionProfile {
    pub session_id: String,
    pub file_size: u64,
    pub line_count: u64,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    pub role_counts: HashMap<String, u64>,
    pub has_tantivy_index: bool,
    pub interesting_events: Vec<InterestingEvent>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InterestingEvent {
    pub line_number: u64,
    pub event_type: EventType,
    pub summary: String,
    pub gap_lines: u64,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub enum EventType {
    Compaction,
    TodoCreate,
    TodoUpdate,
    TodoDelete,
    GitCommit,
    GitPush,
    GitTag,
    UserMessage,
}

/// Summarize a tool call into a compact one-line description.
pub fn summarize_tool_call(name: &str, args_str: &str) -> String {
    let args: serde_json::Value = match serde_json::from_str(args_str) {
        Ok(v) => v,
        Err(_) => return format!("{}({})", name, truncate_chars(args_str, 200)),
    };

    match name {
        "bash" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            format!("bash: {}", truncate_chars(cmd, 300))
        }
        "write_file" => {
            let fp = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let content_len = args
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            format!("write_file({}, {} chars)", fp, content_len)
        }
        "edit" => {
            let fp = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let old_len = args
                .get("old_string")
                .and_then(|v| v.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            format!("edit({}, {} chars replaced)", fp, old_len)
        }
        "read_file" => {
            let fp = args
                .get("file_path")
                .or_else(|| args.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("read_file({})", fp)
        }
        "grep" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("grep({}, {})", pattern, path)
        }
        "task" => {
            let agent = args.get("agent").and_then(|v| v.as_str()).unwrap_or("?");
            format!("task({})", agent)
        }
        "todo" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("?");
            format!("todo({})", action)
        }
        "skill" => {
            let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            format!("skill({})", skill_name)
        }
        _ => {
            let s = serde_json::to_string(&args).unwrap_or_default();
            format!("{}({})", name, truncate_chars(&s, 300))
        }
    }
}

/// Convert messages to a compact text representation for the LLM.
/// Tool calls and results are summarized, not dumped verbatim.
///
/// Every string that enters the output is redacted first
/// ([`crate::redact::redact_secrets`]) and truncated second. That order is
/// load-bearing: a snip can cut a key in half, and a half key is a prefix that
/// no redactor will recognise on the way out. This is the single funnel for
/// every LLM prompt built from a session, so the redaction cannot be bypassed
/// by reaching for a different prompt builder.
pub fn messages_to_text(messages: &[RolloutMessage]) -> String {
    messages
        .iter()
        .filter_map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One message's block of [`messages_to_text`].
///
/// Split out so a byte-bounded prompt builder can render the newest messages
/// only, instead of formatting a whole session it is about to drop. Joining the
/// non-empty blocks of [`message_to_text`] with a newline reproduces
/// [`messages_to_text`] exactly; a message that contributes no lines
/// contributes no block, and so no extra separator.
pub fn message_to_text(msg: &RolloutMessage) -> Option<String> {
    let mut lines = Vec::new();
    let role = msg.role.to_uppercase();

    for tc_summary in &msg.tool_calls_summary {
        lines.push(format!("  {} -> {}", role, redact_secrets(tc_summary)));
    }

    if !msg.content.is_empty() {
        let content = redact_secrets(&msg.content);
        if role == "TOOL" {
            let truncated = truncate_chars(&content, 500);
            lines.push(format!("  TOOL RESULT: {}", truncated));
            if content.len() > 500 {
                lines.push("  ... (truncated)".to_string());
            }
        } else {
            let truncated = truncate_chars(&content, 1500);
            lines.push(format!("[{}]", role));
            lines.push(truncated.to_string());
            if content.len() > 1500 {
                lines.push("... (truncated)".to_string());
            }
        }
        lines.push(String::new());
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Does `s` look like an ISO8601 timestamp (so it can be compared
/// lexicographically against a cutoff)? Unparsable times are kept by the
/// bound filters (safe default).
pub fn is_iso8601(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 19 && b[4] == b'-' && b[7] == b'-' && (b[10] == b'T' || b[10] == b' ')
}

/// Truncate a string to at most `max_bytes` without splitting a UTF-8 character.
pub fn truncate_chars(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Resolve a partial session ID to a full session directory path.
/// Returns the most recent matching session if multiple match.
pub fn resolve_session_dir(root: &PathBuf, partial_id: &str) -> Option<PathBuf> {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("store {}: cannot be listed: {e}", root.display());
            return None;
        }
    };
    let mut matches: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.contains(partial_id)
            && let Ok(meta) = entry.metadata()
        {
            let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            matches.push((entry.path(), mtime));
        }
    }

    matches.sort_by_key(|b| std::cmp::Reverse(b.1));
    matches.into_iter().next().map(|(p, _)| p)
}

/// Slice messages from the last compaction marker onward.
/// If no compaction marker is found, returns all messages.
pub fn slice_from_compaction(messages: Vec<RolloutMessage>) -> Vec<RolloutMessage> {
    let last_compaction = messages
        .iter()
        .rposition(|m| m.content.contains("context compaction"));

    match last_compaction {
        Some(idx) => messages[idx..].to_vec(),
        None => messages,
    }
}

// --- Storage-root environment overrides -------------------------------------

/// Per-harness environment variable that overrides the storage root.
/// When set (non-empty), the adapter reads from this path instead of the
/// `$HOME`-derived live store. Used to point the CLI and MCP server at
/// fixtures or simulated data without ever touching live sessions.
pub const VIBE_ROOT_ENV_VAR: &str = "TOTAL_RECALL_VIBE_ROOT";
pub const CLAUDE_ROOT_ENV_VAR: &str = "TOTAL_RECALL_CLAUDE_ROOT";
pub const CODEX_ROOT_ENV_VAR: &str = "TOTAL_RECALL_CODEX_ROOT";
pub const OPENCODE_ROOT_ENV_VAR: &str = "TOTAL_RECALL_OPENCODE_ROOT";

/// When set to "1"/"true", `make_adapter` refuses to build any adapter whose
/// root did NOT come from its env override — mechanically enforcing that no
/// code path reads a live store in sandboxed dev/test runs.
pub const SANDBOX_ENV_VAR: &str = "TOTAL_RECALL_SANDBOX";

/// Read a root-override env var. Empty string counts as unset.
pub fn env_root(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

/// Whether the sandbox guard is armed.
pub fn sandbox_enabled() -> bool {
    std::env::var(SANDBOX_ENV_VAR)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Resolve an adapter storage root. Precedence:
/// `TOTAL_RECALL_<H>_ROOT` (non-empty) > `$HOME`-derived default > relative fallback.
/// Returns the path and whether it came from the env override.
pub fn resolve_root(var: &str, home_segments: &[&str]) -> (PathBuf, bool) {
    if let Some(p) = env_root(var) {
        return (p, true);
    }
    let mut path = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    for seg in home_segments {
        path.push(seg);
    }
    (path, false)
}
