//! Reusable clipping library (ticket 804, Section B; additive).
//!
//! A single, self-contained module for bounded output: constants cataloguing
//! the fixed sizes already applied across this repo, plus functions that clip
//! a string front/end to a byte bound (UTF-8-safe), drive a dynamic
//! byte-bucket over rows (Law 0: byte bucket, estimate, clip each row, stop at
//! the byte limit), and emit the canonical clip notice.
//!
//! Nothing here is wired into live code paths. Every constant copies the
//! VALUE of an existing bound — the originals are untouched and stay
//! authoritative (see [`sizes`] for each one's home).
//!
//! Law 0 (#89): inline returns are bounded to decision-sufficient size
//! (~500B–2KB). Every clip is reported, anything larger is offloaded to disk
//! behind a pointer (`file://` for real files, `sid://` for store records).
//! Agent size hints are hints only — honouring them is our choice, flooding is
//! a bug.

/// Which end of a string the clip keeps: the front (prefix) or the end
/// (suffix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipSide {
    /// Keep the front (prefix); drop from the end.
    Front,
    /// Keep the end (suffix); drop from the front.
    End,
}

/// A predicate over one `char`, for take-while / drop-while clipping.
pub type CharPredicate = fn(char) -> bool;

/// A named clipping strategy. `Truncate` is the static byte-bound path;
/// `TakeWhile` / `DropWhile` are the structural paths. Applied with
/// [`clip_by_strategy`].
///
/// Not `PartialEq`/`Eq`: the strategies hold function pointers, whose
/// addresses are not stable or unique (comparing them is meaningless).
#[derive(Debug, Clone, Copy)]
pub enum ClipStrategy {
    /// Keep at most `max_bytes` (char-boundary-safe) from `side`.
    Truncate { max_bytes: usize, side: ClipSide },
    /// Keep the leading run of chars for which `predicate` holds.
    TakeWhile { predicate: CharPredicate },
    /// Drop the leading run of chars for which `predicate` holds.
    DropWhile { predicate: CharPredicate },
}

/// Clip `s` to at most `max_bytes` bytes without splitting a UTF-8 character
/// (a safe amendment of `rollout::truncate_chars`, which only clips the End,
/// to also support the Front).
///
/// Returns `(clipped, was_clipped, dropped_bytes)`:
/// - `clipped` is a borrow of `s` of length ≤ `max_bytes` (when `max_bytes`
///   is 0 the result is the empty string),
/// - `was_clipped` is true iff bytes were dropped,
/// - `dropped_bytes` is the number of bytes removed.
///
/// The `Front` cut starts at `len - max_bytes` and advances to the next char
/// boundary, so the returned suffix never exceeds `max_bytes`; the `End` cut
/// backs off to a char boundary the same way.
pub fn clip_bytes(s: &str, max_bytes: usize, side: ClipSide) -> (&str, bool, usize) {
    if s.len() <= max_bytes {
        return (s, false, 0);
    }
    match side {
        ClipSide::End => {
            let mut end = max_bytes;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            (&s[..end], true, s.len() - end)
        }
        ClipSide::Front => {
            let mut start = s.len() - max_bytes;
            while start < s.len() && !s.is_char_boundary(start) {
                start += 1;
            }
            (&s[start..], true, start)
        }
    }
}

/// Apply a [`ClipStrategy`] to `s`. Returns the same `(clipped, was_clipped,
/// dropped_bytes)` shape as [`clip_bytes`]. `TakeWhile` reports `was_clipped`
/// only when it actually dropped a trailing run; `DropWhile` reports it only
/// when it dropped a leading run.
pub fn clip_by_strategy(s: &str, strategy: ClipStrategy) -> (&str, bool, usize) {
    match strategy {
        ClipStrategy::Truncate { max_bytes, side } => clip_bytes(s, max_bytes, side),
        ClipStrategy::TakeWhile { predicate } => {
            let cut = s
                .char_indices()
                .find(|&(_, c)| !predicate(c))
                .map(|(i, _)| i)
                .unwrap_or(s.len());
            let clipped = &s[..cut];
            (clipped, cut < s.len(), s.len() - cut)
        }
        ClipStrategy::DropWhile { predicate } => {
            let start = s
                .char_indices()
                .find(|&(_, c)| !predicate(c))
                .map(|(i, _)| i)
                .unwrap_or(s.len());
            let clipped = &s[start..];
            (clipped, start > 0, start)
        }
    }
}

/// A byte bucket for the Law 0 dynamic path: rows are admitted until the
/// limit is exhausted, at which point [`admit`](Self::admit) reports false and
/// the driver stops returning rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteBudget {
    limit: usize,
    used: usize,
}

impl ByteBudget {
    /// A bucket that admits at most `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self { limit, used: 0 }
    }

    /// The bucket's byte limit.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Bytes admitted so far.
    pub fn used(&self) -> usize {
        self.used
    }

    /// Bytes still available; 0 when exhausted.
    pub fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.used)
    }

    /// True once the bucket can admit no more bytes.
    pub fn is_exhausted(&self) -> bool {
        self.used >= self.limit
    }

    /// Reserve `row_bytes` in the bucket. Returns false — reserving nothing —
    /// when the budget is exhausted (even for a zero-byte row: once full,
    /// the driver stops) or when `row_bytes` exceeds what remains.
    pub fn admit(&mut self, row_bytes: usize) -> bool {
        if self.is_exhausted() || row_bytes > self.remaining() {
            return false;
        }
        self.used += row_bytes;
        true
    }
}

/// The Law 0 dynamic row-clip driver: estimate a row, clip it, admit it into
/// the bucket, and — when the bucket is full — stop by returning an empty
/// vector.
///
/// Returns either:
/// - `vec![clipped]` — the row was clipped and admitted (the caller appends
///   it and continues), or
/// - `vec![]` — the row could not fit even clipped; the bucket is full and
///   the caller stops.
///
/// `estimate` sizes a row (it is applied both to the raw row, as an upper
/// bound on what we will charge, and to the clipped row, for the real charge).
/// `clip` may shrink the row — typically into the *remaining* budget, e.g.
/// `|v| clip_bytes(&v, room, ClipSide::End).0.to_string()` with `room`
/// captured from `budget.remaining()` before the call.
pub fn dynamic_clip_row<V>(
    budget: &mut ByteBudget,
    row: V,
    estimate: impl Fn(&V) -> usize,
    clip: impl Fn(V) -> V,
) -> Vec<V> {
    let raw_estimate = estimate(&row);
    let clipped = clip(row);
    // Charge the real size of what we keep, never more than the raw row's own
    // estimate (a clip that grows a row must not inflate the bucket).
    let bytes = estimate(&clipped).min(raw_estimate);
    if budget.admit(bytes) {
        vec![clipped]
    } else {
        Vec::new()
    }
}

/// The canonical clip notice. `[clipped: N bytes; full: <pointer>]` — every
/// clip is reported, and the pointer names where the full content lives
/// (`file://` for real files, `sid://` for store records). Returns the empty
/// string when nothing was clipped. With no pointer the pointer clause is
/// omitted.
pub fn marker(clipped: bool, dropped_bytes: usize, pointer: Option<&str>) -> String {
    if !clipped {
        return String::new();
    }
    match pointer {
        Some(p) => format!("[clipped: {dropped_bytes} bytes; full: {p}]"),
        None => format!("[clipped: {dropped_bytes} bytes]"),
    }
}

/// The fixed sizes already applied in this repo, catalogue from the Section A
/// inventory (ticket 804). Each value copies the original; the originals stay
/// authoritative. Tests assert these literals match the inventory.
pub mod sizes {
    /// `recall.rs`: byte budget for the current-state prompt payload.
    pub const MAX_STATE_BYTES: usize = 200 * 1024;
    /// `recall.rs`: byte budget for the user-goals prompt payload.
    pub const MAX_GOALS_BYTES: usize = 200 * 1024;
    /// `recall.rs`: bytes held back from a prompt budget for its truncation
    /// marker.
    pub const MARKER_RESERVE: usize = 256;
    /// `rollout/mod.rs` `message_to_text`: truncate_chars bound for TOOL
    /// result content.
    pub const TOOL_MESSAGE_TRUNCATE_CHARS: usize = 500;
    /// `rollout/mod.rs` `message_to_text`: truncate_chars bound for all other
    /// roles' content.
    pub const OTHER_MESSAGE_TRUNCATE_CHARS: usize = 1500;
    /// `rollout/mod.rs` `summarize_tool_call`: fallback when args do not
    /// parse as JSON.
    pub const TOOL_CALL_FALLBACK_TRUNCATE_CHARS: usize = 200;
    /// `rollout/mod.rs` `summarize_tool_call`: bash command truncate_chars
    /// bound (also the generic arms' 300).
    pub const TOOL_CALL_BASH_TRUNCATE_CHARS: usize = 300;
    /// `bound.rs`: default record count when the caller gives no explicit
    /// bound.
    pub const DEFAULT_RECORD_LIMIT: usize = 100;
    /// `bound.rs`: upper bound on `limit`; requests above it are rejected.
    pub const MAX_RECORD_LIMIT: usize = 1000;
    /// `bound.rs`: hard byte ceiling on the serialized payload.
    pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;
    /// `bound.rs`: per-record clamp for `content`/`thinking`.
    pub const DEFAULT_MAX_RECORD_BYTES: usize = 256 * 1024;
    /// `report_cap.rs`: default flood-control window for text tool responses.
    pub const REPORT_TEXT_FLOOD_CAP_BYTES: usize = 16 * 1024;
    /// `recall.rs`: capped count of todo-log lines shown per list in a
    /// prior-period summary.
    pub const PRIOR_PERIOD_TODO_LOG_LIST_CAP: usize = 200;
    /// `recall.rs`: count of newest compaction markers shown in the footer.
    pub const PRIOR_PERIOD_COMPACTION_FOOTER_CAP: usize = 3;
    /// `recall.rs`: per-line truncate_chars bound in prior-period sections.
    pub const PRIOR_PERIOD_LINE_TRUNCATE: usize = 300;
}
