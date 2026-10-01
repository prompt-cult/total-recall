//! The suite's scratch-directory discipline, asserted rather than remembered.
//!
//! `tests/common/scratch.rs` is the only file allowed to name a temporary path,
//! and this is why that rule is worth a test. A helper that builds its path from
//! a caller-supplied tag alone gives the same directory to every test that
//! passes the same tag, and the `remove_dir_all` at the head of such a helper
//! then deletes the fixture a sibling is still reading. The failure is invisible
//! in review — the tests are green one at a time — and only appears when two
//! test processes overlap, which is exactly how nobody runs them by hand. The
//! seven private helpers this test now covers were each one edit away from that.
//!
//! The check is deliberately narrow: it greps the `tests/` tree for the two ways
//! a file can name a temporary path without the shared helper, so it cannot fail
//! on formatting, renames or fixture layout, and it fails with the two entry
//! points to use instead.

use std::path::{Path, PathBuf};

/// The one file permitted to name a temporary path, relative to the `tests/` dir.
const OWNER: &str = "common/scratch.rs";

/// Naming a temporary path takes one of these; the shared helper uses both.
const NEEDLES: [&str; 2] = ["temp_dir", "CARGO_TARGET_TMPDIR"];

fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

#[test]
fn only_the_shared_helper_names_a_temporary_path() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tests = Path::new("tests");
    let mut files = Vec::new();
    rust_files(&manifest.join(tests), &mut files);
    files.sort();

    let owner = tests.join(OWNER);
    let this_test = Path::new(file!());
    let offenders: Vec<String> = files
        .iter()
        .map(|path| path.strip_prefix(manifest).unwrap_or(path).to_path_buf())
        .filter(|path| *path != owner && path != this_test)
        .filter_map(|path| {
            let source = without_comments(
                &std::fs::read_to_string(manifest.join(&path)).expect("readable test source"),
            );
            NEEDLES
                .iter()
                .any(|needle| source.contains(needle))
                .then(|| format!("{}: {}", path.display(), first_hit(&source)))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "only {OWNER} may name a temporary path; a path built from a test's own \
         tag is shared with every sibling that passes the same tag. Use \
         `common::scratch::scratch` for a fixture this process reads, or \
         `common::scratch::child_cwd` for a directory that becomes a spawned \
         child's working directory (a path under target/ would let dotenvy find \
         the developer's .env). Offenders:\n{}",
        offenders.join("\n")
    );
}

/// The code of a source file, with line comments dropped.
///
/// Prose may name `temp_dir` while explaining why a test uses the shared
/// helper — that is a comment about the rule, not a breach of it, and a guard
/// that failed on documentation would be rewritten to say less.
fn without_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The line a needle first appears on, so the failure names a place to look.
fn first_hit(code: &str) -> String {
    for (n, line) in code.lines().enumerate() {
        if NEEDLES.iter().any(|needle| line.contains(needle)) {
            return format!("line {}: {}", n + 1, line.trim());
        }
    }
    String::new()
}
