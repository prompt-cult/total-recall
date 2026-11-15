//! The dual-stream flood control primitive: one record stream feeds two
//! sinks at once. The response is built as a bare compact JSON array
//! (`'[' + records.join(',') + ']'`) that is cut at a record boundary — a
//! JSON array, never torn — while every record is ALSO written as pure JSONL
//! (one compact JSON object per line, `'\n'`-terminated, no commas) to a
//! private file as it is pushed. The streamed file is complete at all times
//! and carries ALL records even when the response is capped, so an
//! overflowing response splits into two line-oriented tools: `sed`/`rg`/`jq`
//! on the marker's file for the whole stream, `line_histogram` for its shape
//! — no pretty-JSON array postfixing, no walls of medium lines.
//!
//! Retention: keep on overflow only. The response fits ⇒ the streamed file
//! is deleted on [`JsonlSink::finish`]; it overflows ⇒ the file stays and
//! the EOF marker names it. Every call first prunes overflow files older
//! than 24 hours, sweeping BOTH the `.jsonl` sinks and the surviving text
//! path's `.md` files (the vendored awk script is never pruned).

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use crate::report_cap::{PRUNE_AFTER, line_histogram, overflow_file, prune_older_than};

/// The marker separator between the response array and the EOF block; the
/// array prefix stays extractable (`sed '/^--- \[EOF-TRUNCATED\]/,$d'`).
const MARKER: &str = "--- [EOF-TRUNCATED] ---";

/// Serialize one record to its compact JSON, ready for both sinks.
fn compact(record: &impl Serialize) -> String {
    serde_json::to_string(record).unwrap_or_else(|_| "null".to_string())
}

/// Open the streamed file PRIVATE: mode 600 on unix, identical to the way
/// the text path opens an overflow report.
fn open_private(path: &Path) -> std::io::Result<BufWriter<fs::File>> {
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    Ok(BufWriter::new(file))
}

/// Flood control for a record stream. [`JsonlSink::new`] prunes first, names
/// the `.jsonl` file (seq + pid, the same collision-proof pattern the text
/// overflow path uses) and opens it for streaming; [`JsonlSink::push_meta`]
/// writes record 0; [`JsonlSink::push`] writes every data record;
/// [`JsonlSink::finish`] closes the array, deletes the file if nothing
/// overflowed, and returns the response text.
pub struct JsonlSink {
    tool: String,
    max_bytes: usize,
    /// The compact meta line, once record 0 has been pushed.
    meta: Option<String>,
    /// The response array so far, WITHOUT the closing bracket.
    head: String,
    /// The bytes the full (uncapped) array would need, closing bracket
    /// included — the honest `total_bytes` when the stream overflows.
    full_bytes: usize,
    /// Data records pushed via [`JsonlSink::push`], file and meta excluded.
    records_total: usize,
    /// Data records streamed to the file but left out of the response array.
    dropped: usize,
    path: PathBuf,
    writer: Option<BufWriter<fs::File>>,
    overflow: bool,
}

impl JsonlSink {
    /// Start a dual-stream sink for `tool`/`scope`, capped at `max_bytes`.
    ///
    /// Overflow files older than the shared 24-hour policy are pruned first,
    /// exactly as the text path prunes on every capped call: the sweep takes
    /// both `.jsonl` and `.md` overflow files and never the vendored awk
    /// script.
    pub fn new(tool: &str, scope: &str, max_bytes: usize) -> Self {
        prune_older_than(
            SystemTime::now()
                .checked_sub(PRUNE_AFTER)
                .unwrap_or(UNIX_EPOCH),
        );
        let path = overflow_file(tool, scope, "jsonl");
        let writer = open_private(&path).ok();
        Self {
            tool: tool.to_string(),
            max_bytes,
            meta: None,
            head: "[".to_string(),
            full_bytes: 1, // the opening bracket
            records_total: 0,
            dropped: 0,
            path,
            writer,
            overflow: false,
        }
    }

    /// The comma before an element once the array already has one (the
    /// response is comma-prefixed between elements, never comma-tailed).
    fn opening_comma(&self) -> usize {
        usize::from(self.head.len() > 1)
    }

    /// Route one serialized candidate element at the two sinks: stream it to
    /// the file always; add it to the response array only while the array
    /// still fits `max_bytes` (closing bracket included). Returns whether
    /// the element joined the response.
    fn admit(&mut self, line: &str) -> bool {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writeln!(writer, "{line}");
        }
        self.full_bytes += self.opening_comma() + line.len();
        let grown = self.head.len() + self.opening_comma() + line.len() + 1; // + ']'
        if grown <= self.max_bytes {
            if self.opening_comma() == 1 {
                self.head.push(',');
            }
            self.head.push_str(line);
            true
        } else {
            self.overflow = true;
            false
        }
    }

    /// Record 0: the meta element that always leads the array and the file.
    /// Exactly one per call; any later call is ignored (the first one wins),
    /// so the meta record is never duplicated into the array or the file.
    pub fn push_meta(&mut self, meta: &Value) {
        if self.meta.is_some() {
            return;
        }
        let line = compact(meta);
        self.admit(&line);
        self.meta = Some(line);
    }

    /// One data record: serialized COMPACT, one JSONL line on disk, one
    /// array element in the response while the array fits.
    pub fn push(&mut self, record: &impl Serialize) {
        let line = compact(record);
        self.records_total += 1;
        if !self.admit(&line) {
            self.dropped += 1;
        }
    }

    /// Close the stream and produce the response text. Under the cap the
    /// response is the bare array alone and the streamed file is deleted;
    /// over the cap the array prefix (never torn) is followed by the EOF
    /// marker naming the tool, the byte counts, the record counts, the
    /// `.jsonl` file's path, the prune policy and the file's line histogram.
    pub fn finish(mut self) -> String {
        self.head.push(']');
        self.full_bytes += 1;
        if let Some(writer) = self.writer.take() {
            let _ = writer.into_inner();
        }
        if !self.overflow {
            // The response fits ⇒ the streamed file is deleted: retention
            // is keep on overflow ONLY.
            let _ = fs::remove_file(&self.path);
            return self.head;
        }
        let total_lines = usize::from(self.meta.is_some()) + self.records_total;
        let histogram = line_histogram(&self.path, None, None, None, None)
            .unwrap_or_else(|e| format!("line histogram unavailable: {e}"));
        format!(
            "{}{MARKER}\ntool: {}\nreturned_bytes: {}\ntotal_bytes: {}\ntotal_lines: {total_lines}\nrecords_returned: {}\nrecords_total: {}\nfull_report: {}\nnote: overflow files older than 24 hours are pruned on every capped call; page the file with the line_histogram tool (mode=extract)\n\n{histogram}\n",
            self.head,
            self.tool,
            self.head.len(),
            self.full_bytes,
            self.records_total - self.dropped,
            self.records_total,
            self.path.display(),
        )
    }
}
