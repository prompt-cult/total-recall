//! The JSONL dual-stream sink: one record stream feeds two sinks at once —
//! the response as a bare compact JSON array and the disk as pure JSONL (one
//! compact JSON object per line). Under the cap the response is just the
//! array and the streamed file is deleted; over the cap the array is cut at
//! a record boundary (NEVER torn), the marker names the tool, the byte
//! counts, the record counts, the `.jsonl` file's path, the prune policy and
//! the file's line histogram, and the file stays, carrying ALL records.
//! Written RED, before the module existed.

use total_recall::jsonl_sink::JsonlSink;

use serde_json::{Value, json};

const MARKER: &str = "--- [EOF-TRUNCATED] ---";

/// A record big enough that twenty of them overflow a small cap.
fn record(i: usize) -> Value {
    json!({"seq": i, "text": format!("record {i} {}", "x".repeat(48))})
}

fn meta() -> Value {
    json!({"tool": "test_tool", "scope": "jsonl_test", "records_total_hint": "?"})
}

fn jsonl_count_matching(prefix: &str) -> usize {
    std::fs::read_dir(total_recall::report_cap::temp_root())
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    e.path().extension().is_some_and(|x| x == "jsonl")
                        && e.file_name().to_string_lossy().starts_with(prefix)
                })
                .count()
        })
        .unwrap_or(0)
}

fn path_from_marker(out: &str) -> std::path::PathBuf {
    let line = out
        .lines()
        .find(|l| l.starts_with("full_report: "))
        .expect("the marker names the .jsonl file path");
    std::path::PathBuf::from(line.trim_start_matches("full_report: ").trim())
}

/// 1. Under the cap the response parses as a JSON array whose element 0 is
///    the meta record, and no `.jsonl` file is retained.
#[test]
fn under_the_cap_the_response_is_the_bare_array_and_no_jsonl_is_retained() {
    let prefix = "test_tool_undercap_";
    let before = jsonl_count_matching(prefix);
    let m = meta();
    let mut sink = JsonlSink::new("test_tool", "undercap", 4096);
    sink.push_meta(&m);
    for i in 0..4 {
        sink.push(&record(i));
    }
    let out = sink.finish();
    assert_eq!(
        jsonl_count_matching(prefix),
        before,
        "an under-the-cap call retains no .jsonl file"
    );
    let value: Value = serde_json::from_str(&out).expect("the response must parse as JSON");
    let array = value.as_array().expect("the response must be an array");
    assert_eq!(array.first(), Some(&m), "element 0 is the meta record");
    assert_eq!(array.len(), 5, "meta plus every record");
}

/// 2. Over the cap the response is a valid prefix array plus the marker; the
///    marker carries tool, returned_bytes, total_bytes, records_returned,
///    records_total and the full file's path.
#[test]
fn over_the_cap_the_marker_carries_the_counts_and_the_path() {
    let mut sink = JsonlSink::new("test_tool", "marker_fields", 256);
    sink.push_meta(&meta());
    for i in 0..20 {
        sink.push(&record(i));
    }
    let out = sink.finish();
    for needle in [
        "tool: test_tool",
        "returned_bytes: ",
        "total_bytes: ",
        "records_returned: ",
        "records_total: 20",
        "full_report: ",
    ] {
        assert!(
            out.contains(needle),
            "the marker must name `{needle}`:\n{out}"
        );
    }
    assert!(
        out.contains(".jsonl"),
        "the overflow file must carry the .jsonl extension:\n{out}"
    );
    assert!(
        !out.contains("records_total: 0"),
        "all twenty pushed records must be counted: {out}"
    );
}

/// 3. The `.jsonl` file: every line parses as one JSON object; no commas
///    between lines; the final line ends `\n`; the file carries ALL records
///    (the response is a subset of the file).
#[test]
fn the_jsonl_file_is_pure_jsonl_and_carries_every_record() {
    let m = meta();
    let mut sink = JsonlSink::new("test_tool", "pure_jsonl", 256);
    sink.push_meta(&m);
    for i in 0..20 {
        sink.push(&record(i));
    }
    let out = sink.finish();
    assert!(out.contains(MARKER), "precondition: this call overflowed");
    let path = path_from_marker(&out);
    let raw = std::fs::read_to_string(&path).expect("the streamed file exists");
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), 21, "meta plus every record, one per line");
    let mut seen = Vec::new();
    for line in &lines {
        assert!(
            !line.trim_end().ends_with(','),
            "JSONL carries no commas between lines: {line:?}"
        );
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("every line parses as one JSON object ({e}): {line:?}"));
        seen.push(value);
    }
    assert_eq!(
        seen.first(),
        Some(&m),
        "the file leads with the meta record"
    );
    assert_eq!(seen.len(), 21, "the file carries ALL records");
    // response ⊆ file: every array element in the response appears in the file.
    let head = out.split(MARKER).next().unwrap();
    let response: Value = serde_json::from_str(head).expect("the head parses");
    for element in response.as_array().unwrap() {
        assert!(
            seen.contains(element),
            "every response element must also be a file line: {element}"
        );
    }
}

/// 4. The file is private (mode 600) and carries the .jsonl extension.
#[test]
fn the_streamed_file_is_private_jsonl() {
    let mut sink = JsonlSink::new("test_tool", "private", 256);
    sink.push_meta(&meta());
    for i in 0..20 {
        sink.push(&record(i));
    }
    let out = sink.finish();
    let path = path_from_marker(&out);
    assert_eq!(
        path.extension().and_then(|e| e.to_str()),
        Some("jsonl"),
        "the overflow file carries the .jsonl extension"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "private (mode 600), got {:o}", mode);
    }
}

/// 5. Two concurrent sinks never collide on filenames: the serial counter the
///    .md path uses separates the calls in a process, the pid separates
///    processes.
#[test]
fn concurrent_sinks_never_collide_on_filenames() {
    let mut handles = Vec::new();
    for _ in 0..4 {
        handles.push(std::thread::spawn(|| {
            let mut sink = JsonlSink::new("test_tool", "collision", 256);
            sink.push_meta(&json!({"who": "thread"}));
            for i in 0..20 {
                sink.push(&record(i));
            }
            sink.finish()
        }));
    }
    let mut paths = Vec::new();
    for handle in handles {
        let out = handle.join().expect("no sink thread panicked");
        let head = out.split(MARKER).next().unwrap();
        serde_json::from_str::<Value>(head).expect("even a concurrent head parses");
        paths.push(path_from_marker(&out));
    }
    let unique: std::collections::HashSet<&std::path::PathBuf> = paths.iter().collect();
    assert_eq!(
        unique.len(),
        4,
        "four concurrent sinks must have four distinct file names, got {paths:?}"
    );
    for path in &paths {
        assert!(
            path.exists(),
            "every overflowing sink leaves its complete file behind: {path:?}"
        );
    }
}

/// 6. The marker carries the .jsonl file's line histogram — the chonky record
///    lines and the sharded response must stay pageable by line_histogram.
#[test]
fn the_marker_carries_the_files_line_histogram() {
    let mut sink = JsonlSink::new("test_tool", "histogram", 256);
    sink.push_meta(&meta());
    for i in 0..20 {
        sink.push(&record(i));
    }
    let out = sink.finish();
    assert!(
        out.contains("Bucket Distribution"),
        "the marker must carry the streamed file's line histogram:\n{out}"
    );
    assert!(
        out.contains("24 hours"),
        "the marker must still record the prune policy:\n{out}"
    );
}

/// 7. The response array is NEVER torn: split at the marker, the head parses
///    as bare JSON on its own. Where the window admits it, the array leads
///    with the meta record and stays at or under the cap; at a window tinier
///    than the meta array itself the cut rule excludes record 0 too, and the
///    head is simply a bare, parseable `[]`.
#[test]
fn the_response_array_is_never_torn() {
    let m = meta();
    for max in [128, 256, 333, 700] {
        let mut sink = JsonlSink::new("test_tool", "never_torn", max);
        sink.push_meta(&m);
        for i in 0..20 {
            sink.push(&record(i));
        }
        let out = sink.finish();
        let head = out.split(MARKER).next().unwrap().trim_end();
        let value: Value = serde_json::from_str(head)
            .unwrap_or_else(|e| panic!("head at max={max} must parse as JSON ({e}): {head:?}"));
        let array: &[Value] = value
            .as_array()
            .unwrap_or_else(|| panic!("head at max={max} must be the bare array"));
        assert_eq!(
            array.first(),
            Some(&m),
            "element 0 is the meta record at max={max}"
        );
        assert!(
            head.len() <= max,
            "the head never grows past the window: {} > {max}",
            head.len()
        );
    }

    // A window smaller than the meta array alone: nothing fits, the head is
    // still a bare `[]`, never a torn fragment of one record.
    let mut sink = JsonlSink::new("test_tool", "never_torn", 32);
    sink.push_meta(&m);
    sink.push(&record(0));
    let out = sink.finish();
    let head = out.split(MARKER).next().unwrap().trim_end();
    let value: Value = serde_json::from_str(head).expect("even an empty head parses");
    assert!(
        value.as_array().is_some_and(|a| a.is_empty()),
        "a window smaller than the meta array yields a bare empty array, got {head:?}"
    );
}

/// 8. Zero data records still yields `[` + meta + `]`.
#[test]
fn zero_data_records_still_yields_the_meta_array() {
    let m = meta();
    let mut sink = JsonlSink::new("test_tool", "zero_records", 4096);
    sink.push_meta(&m);
    let out = sink.finish();
    let value: Value = serde_json::from_str(&out).expect("the response must parse");
    let array: &[Value] = value.as_array().expect("the response must be an array");
    assert_eq!(
        array,
        &vec![m],
        "a meta-only call still yields a bare array"
    );
}
