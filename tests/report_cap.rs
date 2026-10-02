//! Flood control: every MCP response that can overflow the caller's context
//! is capped. An overflowing response is written whole to a private file
//! under the user's temp directory, the returned text is cut at a line
//! boundary (JSON is never torn — the marker returns alone), and the marker
//! names the tool, byte counts, line count, the full file's path, the prune
//! policy, and the file's line histogram (the vendored line_histogram.awk).
//! Written RED, before the module existed.
//!
//! File-system-coupled assertions live in ONE test (`overflow_file_lifecycle`)
//! because the temp root is shared, tests run concurrently in one process,
//! and a prune in one test must never delete another test's overflow file
//! mid-assertion. Every other test asserts on the returned text alone.

mod common;

use total_recall::report_cap::{DEFAULT_MAX_BYTES, cap_report, prune_older_than, temp_root};

use std::time::{Duration, SystemTime};

fn input() -> String {
    let mut s = String::new();
    for i in 0..40 {
        s.push_str(&format!(
            "line {i} with enough words to pass any cap check\n"
        ));
    }
    s
}

fn json_input() -> String {
    let mut json = String::from("[\n");
    for i in 0..40 {
        json.push_str(&format!(
            "  {{\"row\": {i}, \"pad\": \"{}\"}},\n",
            "x".repeat(60)
        ));
    }
    json.push_str("  {\"row\": 40}\n]\n");
    json
}

fn cap(content: String, max: usize) -> String {
    cap_report("test_tool", "scope", content, max)
}

fn path_from_marker(out: &str) -> std::path::PathBuf {
    let line = out
        .lines()
        .find(|l| l.starts_with("full_report: "))
        .expect("marker names the full report path");
    std::path::PathBuf::from(line.trim_start_matches("full_report: ").trim())
}

fn md_count_matching(prefix: &str) -> usize {
    std::fs::read_dir(temp_root())
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    e.path().extension().is_some_and(|x| x == "md")
                        && e.file_name().to_string_lossy().starts_with(prefix)
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn under_the_cap_the_response_is_returned_untouched() {
    let content = "short\nreport\n".to_string();
    // A scope unique to this test: concurrent tests write their own overflow
    // files; only THIS call's name counts.
    let prefix = "test_tool_undercap_";
    let before = md_count_matching(prefix);
    let out = cap_report("test_tool", "undercap", content.clone(), 1024);
    assert_eq!(out, content);
    assert_eq!(
        md_count_matching(prefix),
        before,
        "an under-the-cap call writes no overflow report"
    );
}

#[test]
fn the_default_window_is_model_sensible() {
    assert_eq!(DEFAULT_MAX_BYTES, 16_384, "the default window is 16 KiB");
}

#[test]
fn overflow_cuts_at_a_line_boundary_at_or_under_the_cap() {
    let out = cap(input(), 64);
    let head = out.split("--- [EOF-TRUNCATED] ---").next().unwrap();
    assert!(
        !head.is_empty() && head.len() <= 64,
        "the returned head must be cut at or under the cap, got {} bytes",
        head.len()
    );
    assert!(head.ends_with('\n'), "the cut must fall on a line boundary");
}

#[test]
fn the_marker_names_the_stats_the_path_and_the_policy() {
    let out = cap(input(), 64);
    for needle in [
        "--- [EOF-TRUNCATED] ---",
        "tool: test_tool",
        "returned_bytes:",
        "total_bytes:",
        "total_lines:",
        "full_report:",
        "24 hours",
        "page the file with the line_histogram tool (mode=extract)",
    ] {
        assert!(
            out.contains(needle),
            "the marker must name `{needle}`:\n{out}"
        );
    }
}

#[test]
fn the_marker_carries_the_line_histogram() {
    let out = cap(input(), 64);
    assert!(
        out.contains("Bucket Distribution"),
        "the marker must carry the overflow file's line histogram:\n{out}"
    );
}

#[test]
fn json_is_never_torn_the_marker_returns_alone() {
    let out = cap(json_input(), 64);
    assert!(
        out.starts_with("--- [EOF-TRUNCATED] ---"),
        "an overflowing JSON response must return the marker ALONE, got: {}",
        &out[..out.len().min(120)]
    );
}

/// The single owner of every file-system assertion: a text report and a JSON
/// response overflow, their files carry the whole content (the JSON parses),
/// the files are private (mode 600), a past cutoff prunes nothing, a future
/// cutoff prunes the overflow reports but never the vendored awk script.
/// ctime cannot be backdated portably, so the cutoff moves, not the files.
#[test]
fn overflow_file_lifecycle() {
    let content = input();
    let json = json_input();

    let out_text = cap(content.clone(), 64);
    let text_path = path_from_marker(&out_text);
    assert!(text_path.exists(), "the text overflow file exists");
    assert_eq!(
        std::fs::read_to_string(&text_path).unwrap(),
        content,
        "the overflow file carries the FULL text response, byte for byte"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&text_path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "private (mode 600), got {:o}",
            mode & 0o777
        );
    }

    let out_json = cap(json.clone(), 64);
    let json_path = path_from_marker(&out_json);
    assert!(json_path.exists(), "the json overflow file exists");
    let written = std::fs::read_to_string(&json_path).unwrap();
    assert_eq!(written, json, "the overflow file carries the whole JSON");
    serde_json::from_str::<serde_json::Value>(&written).expect("the overflow JSON file must parse");

    // precondition for the script-survival assertion below: the histogram
    // ran while capping, so the vendored script was staged.
    assert!(
        out_text.contains("Bucket Distribution"),
        "precondition: the histogram ran, so the script was staged"
    );
    let script = temp_root().join("line_histogram.awk");
    assert!(script.exists(), "the vendored script is staged");

    // A cutoff in the past prunes nothing.
    prune_older_than(SystemTime::now() - Duration::from_secs(3600));
    assert!(
        text_path.exists(),
        "a past cutoff keeps fresh overflow files"
    );
    assert!(
        json_path.exists(),
        "a past cutoff keeps fresh overflow files"
    );
    assert!(script.exists(), "a past cutoff keeps the vendored script");

    // A cutoff in the future treats fresh files as older than it: the
    // overflow reports go, the vendored awk script never does.
    prune_older_than(SystemTime::now() + Duration::from_secs(3600));
    assert!(
        !text_path.exists(),
        "an older-than-cutoff overflow report is pruned"
    );
    assert!(
        !json_path.exists(),
        "the json overflow report is pruned too"
    );
    assert!(
        script.exists(),
        "prune must never remove the vendored awk script, only overflow reports"
    );
}
