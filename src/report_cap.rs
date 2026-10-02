//! Flood control for tool responses.
//!
//! Every MCP tool response that can overflow the caller's context passes
//! through [`cap_report`]: a response under `max_bytes` is returned
//! untouched; an overflowing response is written whole to a private file
//! under the user's temp directory, the returned text is cut at a line
//! boundary (JSON responses are never torn — the marker returns alone),
//! and it ends with a line-oriented EOF marker naming the tool, the byte
//! counts, the line count, the full file's path, the prune policy and the
//! file's line histogram.
//!
//! The line histogram comes from the vendored `scripts/line_histogram.awk`
//! (embedded at compile time), invoked with a direct `awk -f` spawn — the
//! script's shebang is never relied on. [`line_histogram`] exposes the same
//! runner as an MCP tool, the paging companion for overflow files.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The default flood-control window: 16 KiB, a small-model-sensible size.
pub const DEFAULT_MAX_BYTES: usize = 16_384;

/// Overflow files older than this are pruned on every capped call.
pub const PRUNE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// The vendored `scripts/line_histogram.awk`, embedded at compile time.
const LINE_HISTOGRAM_AWK: &str = include_str!("../scripts/line_histogram.awk");

/// The temp-directory root holding overflow files: `$TMPDIR/total-recall/`.
pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("total-recall")
}

/// Remove overflow report files (`.md`) older than `cutoff` from
/// [`temp_root`]. The vendored awk script is never pruned. Trivial by
/// design: no manifest, no locks — a sweep per capped call.
pub fn prune_older_than(cutoff: SystemTime) {
    let Ok(entries) = fs::read_dir(temp_root()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        // ctime is not exposed portably by std; the creation time is the
        // honest reading of "how long has this file existed", with mtime as
        // the fallback.
        let age_marker = meta
            .created()
            .unwrap_or_else(|_| meta.modified().unwrap_or(SystemTime::now()));
        if age_marker < cutoff {
            let _ = fs::remove_file(&path);
        }
    }
}

fn write_private(path: &Path, content: &str) {
    let _ = fs::create_dir_all(path.parent().unwrap_or(path));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut f) = opts.open(path) {
        use std::io::Write;
        let _ = f.write_all(content.as_bytes());
    }
}

/// Stage the embedded awk script for `awk -f` and return its path. The
/// shebang in the script is never executed; a direct spawn of `awk` is.
fn stage_awk_script() -> PathBuf {
    let path = temp_root().join("line_histogram.awk");
    let _ = fs::create_dir_all(temp_root());
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut f) = opts.open(&path) {
        use std::io::Write;
        let _ = f.write_all(LINE_HISTOGRAM_AWK.as_bytes());
    }
    path
}

/// Run the vendored `line_histogram.awk` over `file`.
///
/// `mode` selects the awk modes: `histogram` (default) profiles the file by
/// line-size distribution; `extract` returns the line at `line`, or the
/// inclusive range `start`..=`end`. A direct `awk -f <script> <file>` spawn
/// — no shell, no shebang reliance.
pub fn line_histogram(
    file: &Path,
    mode: Option<&str>,
    line: Option<u64>,
    start: Option<u64>,
    end: Option<u64>,
) -> Result<String, String> {
    if !file.is_file() {
        return Err(format!("no file at {}: cannot histogram", file.display()));
    }
    let script = stage_awk_script();
    let mut cmd = Command::new("awk");
    cmd.arg("-f").arg(&script);
    if let Some(mode) = mode {
        cmd.arg(format!("-vmode={mode}"));
    }
    if let Some(line) = line {
        cmd.arg(format!("-vline={line}"));
    }
    if let Some(start) = start {
        cmd.arg(format!("-vstart={start}"));
    }
    if let Some(end) = end {
        cmd.arg(format!("-vend={end}"));
    }
    cmd.arg(file);
    let output = cmd
        .output()
        .map_err(|e| format!("cannot spawn awk -f: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "line_histogram.awk failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Per-process sequence for overflow filenames: two overflowing calls in the
/// same millisecond must never clobber each other's file. The scratch-helper
/// race taught this class of collision once already — a name keyed on time
/// alone collides; pid separates processes, the counter separates calls.
static OVERFLOW_SEQ: AtomicU64 = AtomicU64::new(0);

fn overflow_file(tool: &str, scope: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = OVERFLOW_SEQ.fetch_add(1, Ordering::Relaxed);
    let clean_tool = tool.replace(['/', ' '], "_");
    let clean_scope = if scope.is_empty() {
        "all".to_string()
    } else {
        scope.replace(['/', ' '], "_")
    };
    let pid = std::process::id();
    temp_root().join(format!(
        "{clean_tool}_{clean_scope}_{millis}_{pid}_{seq}.md"
    ))
}

/// Flood control: cap `content` at `max_bytes`.
///
/// Under the cap the content is returned untouched. Over it, the full
/// content is written to a private file (mode 600) under [`temp_root`], and
/// the return is the content cut at a line boundary — unless the content
/// is JSON, which is never torn: an overflowing JSON response returns the
/// marker alone. The marker is line-oriented and names the tool, the
/// returned and total byte counts, the total line count, the full file's
/// path, the prune policy, and the file's line histogram.
///
/// Every capped call first prunes overflow files older than 24 hours.
pub fn cap_report(tool: &str, scope: &str, content: String, max_bytes: usize) -> String {
    prune_older_than(
        SystemTime::now()
            .checked_sub(PRUNE_AFTER)
            .unwrap_or(SystemTime::UNIX_EPOCH),
    );
    let total = content.len();
    if total <= max_bytes {
        return content;
    }

    let path = overflow_file(tool, scope);
    write_private(&path, &content);
    let total_lines = content.lines().count();

    let trimmed = content.trim_start();
    let is_json = trimmed.starts_with('[') || trimmed.starts_with('{');
    let head = if is_json {
        // JSON is never torn: the marker returns alone and the whole,
        // parseable JSON lives in the overflow file.
        String::new()
    } else {
        // cut at a line boundary at or under the cap
        let mut cut = max_bytes.min(total);
        let bytes = content.as_bytes();
        while cut > 0 && bytes[cut - 1] != b'\n' {
            cut -= 1;
        }
        if cut == 0 {
            cut = max_bytes.min(total);
        }
        content[..cut].to_string()
    };
    let returned = head.len();

    let histogram = line_histogram(&path, None, None, None, None)
        .unwrap_or_else(|e| format!("line histogram unavailable: {e}"));

    format!(
        "{head}--- [EOF-TRUNCATED] ---\ntool: {tool}\nreturned_bytes: {returned}\ntotal_bytes: {total}\ntotal_lines: {total_lines}\nfull_report: {}\nnote: overflow files older than 24 hours are pruned on every capped call; page the file with the line_histogram tool (mode=extract)\n\n{histogram}\n",
        path.display()
    )
}
