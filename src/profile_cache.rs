use std::path::{Path, PathBuf};

use crate::profile_cache_types as generated;
use crate::rollout::SessionProfile;

/// A cache file younger than this is trusted even if the source moved on:
/// `fresh` when `time_updated_ms <= cache_mtime_ms + STALENESS_TOLERANCE_MS`.
pub const STALENESS_TOLERANCE_MS: i64 = 15_000;

/// Flat cache file inside the adapter's shadow index root:
/// `<shadow_root>/tr_<session-id>_meta.json`.
pub fn cache_path(shadow_root: &Path, session_id: &str) -> PathBuf {
    shadow_root.join(format!("tr_{session_id}_meta.json"))
}

/// File mtime as unix epoch milliseconds; `None` when the file (or its mtime)
/// is unreachable.
pub fn mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let ms = modified
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some(ms)
}

/// Serve the cached profile when the cache file exists, deserializes through
/// the JTD-generated validator, and is not stale. Any failure (missing, stale,
/// corrupt, structurally wrong) yields `None` and the caller recomputes.
pub fn read_fresh(cache_path: &Path, time_updated_ms: i64) -> Option<SessionProfile> {
    let cache_mtime_ms = mtime_ms(cache_path)?;
    if time_updated_ms > cache_mtime_ms + STALENESS_TOLERANCE_MS {
        return None;
    }
    let data = std::fs::read(cache_path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&data).ok()?;
    if !generated::validate(&value).is_empty() {
        return None;
    }
    let envelope: generated::ProfileCacheEnvelope = serde_json::from_value(value).ok()?;
    SessionProfile::try_from(envelope).ok()
}

/// Write the cache envelope for `profile`. The envelope is validated with the
/// generated JTD validator before it hits disk, so a cache file we write can
/// never fail the read-side gate.
pub fn write(cache_path: &Path, profile: &SessionProfile) {
    let envelope = generated::envelope_json(profile, now_ms().max(0) as u64);
    if !generated::validate(&envelope).is_empty() {
        return;
    }
    let Ok(json) = serde_json::to_vec(&envelope) else {
        return;
    };
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(cache_path, json);
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
