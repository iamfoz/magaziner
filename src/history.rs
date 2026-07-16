//! Persistent download-history store for `magaziner`.
//!
//! Tracks which magazine issues (identified by `(source, issue_id)`) have
//! already been downloaded, so the CLI can skip re-downloading them on
//! subsequent runs. The history is serialized as pretty-printed JSON on
//! disk and is written atomically (write-to-temp-file + rename) so that a
//! crash or interruption mid-save can never leave a corrupt or truncated
//! history file behind.

use anyhow::{Context, Result};
use std::path::Path;

/// A single recorded download.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    /// Source identifier, e.g. `"LRB"` or `"Harpers"`.
    pub source: String,
    /// Canonical issue id, e.g. `"v48/n01"`.
    pub issue_id: String,
    /// Human-readable title, e.g. `"Vol. 48 No. 1 · 2 January 2026"`.
    pub title: String,
    /// Output filename (without directory), e.g. `"LRB - Vol. 48 No. 1.epub"`.
    pub filename: String,
    /// RFC 3339 / ISO 8601 UTC timestamp of when the download was recorded.
    pub downloaded_at: String,
}

impl HistoryEntry {
    /// Build an entry with `downloaded_at` set to now (UTC, RFC 3339).
    pub fn now(
        source: impl Into<String>,
        issue_id: impl Into<String>,
        title: impl Into<String>,
        filename: impl Into<String>,
    ) -> HistoryEntry {
        HistoryEntry {
            source: source.into(),
            issue_id: issue_id.into(),
            title: title.into(),
            filename: filename.into(),
            downloaded_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// The full download history: a flat list of [`HistoryEntry`] records.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    /// Load history from `path`. If the file does not exist, an empty
    /// [`History`] is returned rather than an error. If the file exists but
    /// cannot be read or parsed, an error is returned.
    pub fn load(path: &Path) -> Result<History> {
        if !path.exists() {
            return Ok(History::default());
        }

        let data = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read history file: {}", path.display()))?;

        let history: History = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse history file: {}", path.display()))?;

        Ok(history)
    }

    /// True if an entry with this `(source, issue_id)` already exists.
    #[allow(dead_code)]
    pub fn contains(&self, source: &str, issue_id: &str) -> bool {
        self.get(source, issue_id).is_some()
    }

    /// The recorded entry for this `(source, issue_id)`, if any.
    pub fn get(&self, source: &str, issue_id: &str) -> Option<&HistoryEntry> {
        self.entries
            .iter()
            .find(|e| e.source == source && e.issue_id == issue_id)
    }

    /// Record a download. If an entry with the same `(source, issue_id)`
    /// already exists it is replaced in place (updating timestamp, title,
    /// and filename); otherwise the new entry is appended.
    pub fn record(&mut self, entry: HistoryEntry) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.source == entry.source && e.issue_id == entry.issue_id)
        {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// Persist to `path` as pretty-printed JSON.
    ///
    /// Writes atomically: the serialized data is first written to a
    /// `<path>.tmp` sibling file in the same directory, then renamed over
    /// `path`. This ensures a crash mid-write cannot corrupt an existing
    /// history file, since the rename is atomic on the underlying
    /// filesystem. Parent directories are created if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory: {}", parent.display()))?;
        }

        let json = serde_json::to_string_pretty(self)
            .context("failed to serialize history to JSON")?;

        let mut tmp_path = path.as_os_str().to_owned();
        tmp_path.push(".tmp");
        let tmp_path = Path::new(&tmp_path);

        let write_result = std::fs::write(tmp_path, json.as_bytes())
            .with_context(|| format!("failed to write temp history file: {}", tmp_path.display()));

        if let Err(err) = write_result {
            // Best-effort cleanup of a partially written temp file.
            let _ = std::fs::remove_file(tmp_path);
            return Err(err);
        }

        if let Err(err) = std::fs::rename(tmp_path, path) {
            // Best-effort cleanup of the temp file if the rename failed.
            let _ = std::fs::remove_file(tmp_path);
            return Err(err).with_context(|| {
                format!(
                    "failed to rename {} to {}",
                    tmp_path.display(),
                    path.display()
                )
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Returns a fresh, unique path under the system temp directory for use
    /// as a scratch history file in a single test. No file is created by
    /// this call.
    fn scratch_path(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("magaziner-history-tests-{pid}"));
        std::fs::create_dir_all(&dir).expect("failed to create scratch dir");
        dir.join(format!("{label}-{nonce}.json"))
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let _ = std::fs::remove_file(Path::new(&tmp));
    }

    #[test]
    fn load_missing_returns_empty() {
        let path = scratch_path("missing");
        // Ensure it really doesn't exist.
        let _ = std::fs::remove_file(&path);

        let history = History::load(&path).expect("load of missing file should not error");
        assert!(history.entries.is_empty());

        cleanup(&path);
    }

    #[test]
    fn load_corrupt_file_errors() {
        let path = scratch_path("corrupt");
        std::fs::write(&path, b"not valid json {{{").expect("failed to write corrupt file");

        let result = History::load(&path);
        assert!(result.is_err(), "expected an error parsing corrupt JSON");

        cleanup(&path);
    }

    #[test]
    fn record_then_contains() {
        let mut history = History::default();
        assert!(!history.contains("LRB", "v48/n01"));

        let entry = HistoryEntry::now(
            "LRB",
            "v48/n01",
            "Vol. 48 No. 1 · 2 January 2026",
            "LRB - Vol. 48 No. 1.epub",
        );
        history.record(entry);

        assert!(history.contains("LRB", "v48/n01"));
        assert!(!history.contains("LRB", "v48/n02"));
        assert!(!history.contains("Harpers", "v48/n01"));
        assert_eq!(history.entries.len(), 1);
    }

    #[test]
    fn record_replaces_duplicate() {
        let mut history = History::default();

        history.record(HistoryEntry::now(
            "LRB",
            "v48/n01",
            "Original Title",
            "original.epub",
        ));
        assert_eq!(history.entries.len(), 1);

        history.record(HistoryEntry::now(
            "LRB",
            "v48/n01",
            "Updated Title",
            "updated.epub",
        ));

        assert_eq!(
            history.entries.len(),
            1,
            "recording a duplicate (source, issue_id) must not add a second entry"
        );
        assert_eq!(history.entries[0].title, "Updated Title");
        assert_eq!(history.entries[0].filename, "updated.epub");
        assert!(history.contains("LRB", "v48/n01"));
    }

    #[test]
    fn save_then_load_round_trip() {
        let path = scratch_path("roundtrip");
        let _ = std::fs::remove_file(&path);

        let mut history = History::default();
        history.record(HistoryEntry::now(
            "LRB",
            "v48/n01",
            "Vol. 48 No. 1 · 2 January 2026",
            "LRB - Vol. 48 No. 1.epub",
        ));
        history.record(HistoryEntry::now(
            "Harpers",
            "2026/02",
            "February 2026",
            "Harpers - February 2026.epub",
        ));

        history.save(&path).expect("save should succeed");
        assert!(path.exists(), "history file should exist after save");

        // The temp file should not linger after a successful save.
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        assert!(
            !Path::new(&tmp).exists(),
            "temp file should be renamed away, not left behind"
        );

        let loaded = History::load(&path).expect("load should succeed");
        assert_eq!(loaded.entries.len(), 2);
        assert!(loaded.contains("LRB", "v48/n01"));
        assert!(loaded.contains("Harpers", "2026/02"));

        let lrb_entry = loaded
            .entries
            .iter()
            .find(|e| e.source == "LRB" && e.issue_id == "v48/n01")
            .expect("LRB entry should round-trip");
        assert_eq!(lrb_entry.title, "Vol. 48 No. 1 · 2 January 2026");
        assert_eq!(lrb_entry.filename, "LRB - Vol. 48 No. 1.epub");
        assert!(!lrb_entry.downloaded_at.is_empty());

        cleanup(&path);
    }

    #[test]
    fn save_creates_parent_directories() {
        let base = std::env::temp_dir().join(format!(
            "magaziner-history-tests-nested-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested_path = base.join("nested").join("dir").join("history.json");

        // Sanity check: parent directory does not exist yet.
        assert!(!nested_path.parent().unwrap().exists());

        let mut history = History::default();
        history.record(HistoryEntry::now("LRB", "v48/n01", "Title", "file.epub"));
        history
            .save(&nested_path)
            .expect("save should create missing parent directories");

        assert!(nested_path.exists());
        let loaded = History::load(&nested_path).expect("load should succeed");
        assert_eq!(loaded.entries.len(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }
}
