//! Bounded extraction: shared primitives that keep any tool result under a
//! hard byte ceiling so a large session can never crash an MCP client. Used by
//! `extract_messages`, `extract_user_messages`, and `extract_by_type`.

/// Default number of records returned when the caller gives no explicit bound.
pub const DEFAULT_RECORD_LIMIT: usize = 100;
/// Upper bound on `limit`; requests above it are rejected.
pub const MAX_RECORD_LIMIT: usize = 1000;
/// Hard byte ceiling on the serialized payload. ~2x headroom under the
/// observed >16 MiB client failure (pretty JSON + JSON-RPC framing inflate).
pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;
/// The ceiling `max_bytes` is clamped to; never "unbounded" on the MCP side.
pub const MAX_BYTES_CEILING: usize = DEFAULT_MAX_BYTES;
/// Per-record clamp for `content`/`thinking`, guarding a pathological
/// multi-MiB single record. 0 = no clamp.
pub const DEFAULT_MAX_RECORD_BYTES: usize = 256 * 1024;
/// Reserve left for the envelope keys around the records.
pub const ENVELOPE_RESERVE: usize = 8 * 1024;

use crate::rollout::truncate_chars;
use serde::Serialize;

/// Paging + truncation metadata carried in every bounded envelope.
#[derive(Debug, Clone, Serialize)]
pub struct Bounds {
    pub total_records: usize,
    pub returned_records: usize,
    pub offset: usize,
    pub limit: usize,
    /// Offset to pass to fetch the next page; null when no more records.
    pub next_offset: Option<usize>,
    pub truncated: bool,
    /// Why output was cut: "record_limit" | "byte_cap" | "record_clamp" (may
    /// combine, comma-joined). Empty when not truncated.
    pub truncation_reason: String,
    pub max_bytes: usize,
    /// Byte size of the records array as emitted (before the envelope).
    pub bytes: usize,
    /// Indices (within the returned window) of records that were clamped.
    pub clamped_record_indices: Vec<usize>,
    /// Human-readable truncation notice; empty when complete.
    pub notice: String,
}

/// Normalize a requested `limit` against the defaults. `0` = default.
/// Errors when the request exceeds `MAX_RECORD_LIMIT`.
pub fn normalize_limit(limit: usize) -> Result<usize, String> {
    if limit == 0 {
        return Ok(DEFAULT_RECORD_LIMIT);
    }
    if limit > MAX_RECORD_LIMIT {
        return Err(format!(
            "limit {} exceeds the maximum {}; use offset/limit to page",
            limit, MAX_RECORD_LIMIT
        ));
    }
    Ok(limit)
}

/// Normalize a requested `max_bytes`. `0` = default ceiling; anything larger
/// is clamped to the ceiling.
pub fn normalize_max_bytes(max_bytes: usize) -> usize {
    if max_bytes == 0 || max_bytes > MAX_BYTES_CEILING {
        MAX_BYTES_CEILING
    } else {
        max_bytes
    }
}

/// Normalize a per-record clamp. `0` = no clamp (usize::MAX sentinel).
pub fn normalize_max_record_bytes(max_record_bytes: usize) -> usize {
    if max_record_bytes == 0 {
        usize::MAX
    } else {
        max_record_bytes
    }
}

/// A record ready for bounded emission: its serialized single-line JSON plus
/// the byte length, so byte budgeting needs no re-serialization.
pub struct SizedRecord {
    pub json: String,
    pub bytes: usize,
}

/// Select the `[start, end)` window into `total` records.
/// `offset = None` selects the most-recent `limit` (a tail window);
/// `offset = Some(k)` selects records `k .. k+limit` (an index-range walk).
/// Returns `(start, end)` with `end` exclusive, clamped to `total`.
pub fn window(total: usize, offset: Option<usize>, limit: usize) -> (usize, usize) {
    match offset {
        None => {
            let start = total.saturating_sub(limit);
            (start, total)
        }
        Some(k) => {
            let start = k.min(total);
            let end = start.saturating_add(limit).min(total);
            (start, end)
        }
    }
}

/// Clamp a `RolloutMessage`'s `content` and `thinking` to `max_record_bytes`
/// (char-boundary safe). Returns true when anything was clamped.
pub fn clamp_message(msg: &mut crate::rollout::RolloutMessage, max_record_bytes: usize) -> bool {
    if max_record_bytes == usize::MAX {
        return false;
    }
    let mut clamped = false;
    if msg.content.len() > max_record_bytes {
        msg.content = truncate_chars(&msg.content, max_record_bytes).to_string();
        clamped = true;
    }
    if let Some(t) = &msg.thinking
        && t.len() > max_record_bytes
    {
        msg.thinking = Some(truncate_chars(t, max_record_bytes).to_string());
        clamped = true;
    }
    clamped
}

/// Fit as many serialized records as possible under `budget` bytes, taking
/// from the front of `records`. Returns the number that fit.
pub fn fit_count(records: &[SizedRecord], budget: usize) -> usize {
    let mut used = 0usize;
    let mut n = 0usize;
    for r in records {
        // +1 for the newline/comma separator between records.
        let cost = r.bytes.saturating_add(1);
        if used.saturating_add(cost) > budget && n > 0 {
            break;
        }
        used = used.saturating_add(cost);
        n += 1;
        if used >= budget {
            break;
        }
    }
    n
}

/// Assemble the `Bounds` for a windowed, fitted result.
#[allow(clippy::too_many_arguments)]
pub fn finalize_bounds(
    total: usize,
    offset: usize,
    limit: usize,
    returned: usize,
    record_limit_hit: bool,
    byte_cap_hit: bool,
    clamped_indices: Vec<usize>,
    max_bytes: usize,
    bytes: usize,
) -> Bounds {
    let mut reasons = Vec::new();
    if record_limit_hit {
        reasons.push("record_limit");
    }
    if byte_cap_hit {
        reasons.push("byte_cap");
    }
    if !clamped_indices.is_empty() {
        reasons.push("record_clamp");
    }
    // An empty result window (e.g. paging past the end, or an empty session)
    // cut nothing; never signal truncation for it.
    if returned == 0 {
        reasons.clear();
    }
    let truncated = !reasons.is_empty();
    let next_offset = if offset + returned < total {
        Some(offset + returned)
    } else {
        None
    };
    let notice = if truncated {
        format!(
            "TRUNCATED: returned records {}..{} of {} ({}). Use offset/limit to page{}.",
            offset,
            offset + returned,
            total,
            reasons.join(", "),
            match next_offset {
                Some(n) => format!("; next_offset={}", n),
                None => String::new(),
            }
        )
    } else {
        String::new()
    };
    Bounds {
        total_records: total,
        returned_records: returned,
        offset,
        limit,
        next_offset,
        truncated,
        truncation_reason: reasons.join(", "),
        max_bytes,
        bytes,
        clamped_record_indices: clamped_indices,
        notice,
    }
}
