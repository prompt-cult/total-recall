//! One scratch-directory implementation for the whole test suite.
//!
//! Every helper in the suite used to build a path from a caller-supplied tag
//! alone, so the path was a function of the literal in the test body: two
//! tests (or two concurrent test processes) that passed the same tag shared
//! one directory, and the `remove_dir_all` at the head of the helper deleted
//! the fixture the other one was still reading. The helpers here name a
//! directory for one call and nothing else — pid plus a process-wide counter —
//! so a call can only ever clear a leftover from a dead process that was given
//! the same pid and the same counter.
//!
//! Two variants exist, and which one to call is not a detail:
//!
//! * [`scratch`] — target-local, for fixtures the test process reads in.
//!   Cargo owns `target/tmp` and cleans it, and no child process ever runs in
//!   here, so nothing can pick up a developer's `.env`.
//! * [`child_cwd`] — bare `std::env::temp_dir()`, for directories that become a
//!   spawned child's working directory. `dotenvy::dotenv()` walks up parent
//!   directories, so a child started inside `target/` would find the real
//!   `<repo>/.env` and make live API calls. Tests must never do that, and this
//!   path has no `.env` above it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A per-call discriminator. One test binary runs its tests concurrently in
/// ONE process, so the pid is shared by all of them and cannot separate them;
/// a tag alone cannot either, because a duplicate literal is the same flake.
static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

fn scratch_seq() -> u64 {
    SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// A scratch directory for one test, removed when that test ends.
///
/// The directory is created here and cleared on drop, panics included. Every
/// test that takes one binds it to a local and reads it only while it is alive,
/// so by the time it drops no process is still using it.
#[derive(Debug)]
pub struct ScratchRoot(PathBuf);

impl std::ops::Deref for ScratchRoot {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh, uniquely named scratch directory inside cargo's own tmpdir.
///
/// `env!("CARGO_TARGET_TMPDIR")` is the compile-time form: cargo sets it for
/// integration-test targets, and the runtime `std::env::var` of the same name
/// is not set, so the runtime form would silently fall out of `target/`.
///
/// Not for a child process's working directory — see [`child_cwd`].
pub fn scratch(tag: &str) -> ScratchRoot {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "tr_scratch_{tag}_{}_{}",
        std::process::id(),
        scratch_seq()
    ));
    // Only reachable if a dead process left this exact name behind: the pid and
    // counter are consumed, so no live test owns it.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    ScratchRoot(dir)
}

/// An empty working directory OUTSIDE the repository tree, for a spawned child.
///
/// The name is unique per call, but the caller — not this function — removes it,
/// because the child outlives the call that spawned it. Only the owner of the
/// `Child` knows when it has been reaped.
pub fn child_cwd(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tr_child_cwd_{tag}_{}_{}",
        std::process::id(),
        scratch_seq()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The regression these helpers exist to prevent: a second call carrying a tag
/// the first call already used must not destroy the directory the first call
/// handed out, and neither may collide with a sibling test process.
#[test]
fn scratch_dirs_are_unique_per_call_for_a_shared_tag() {
    let first = scratch("shared");
    let second = scratch("shared");
    let cwd = child_cwd("shared");

    assert_ne!(
        *first, *second,
        "a shared tag must still yield distinct fixture directories"
    );
    assert_ne!(
        *first, cwd,
        "the target-local and temp_dir() variants must not collide"
    );
    assert!(
        first.is_dir() && second.is_dir() && cwd.is_dir(),
        "every scratch directory must survive the calls that follow it: {first:?} {second:?} {cwd:?}"
    );
    assert!(
        !cwd.starts_with(env!("CARGO_MANIFEST_DIR")),
        "a child's working directory must sit outside the repository, or \
         dotenvy walks up and finds the developer's .env: {cwd:?}"
    );

    let _ = std::fs::remove_dir_all(&*cwd);
}
