//! Per-machine sync status — tracks the result of every `nit update`.
//!
//! Sacred principle: nit update NEVER clobbers local state. If the work tree
//! has any modifications/deletions of tracked files, update aborts and writes
//! a status file recording why. The user resolves the drift manually
//! (commit, discard, or selectively stage), then the next sync proceeds.
//!
//! Status file lives at `~/.local/share/nit/last-sync.json`. `nit status`
//! reads it to show a one-line health summary; fleet-wide tooling (hemma)
//! can aggregate across machines via SSH.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncResult {
    /// Update completed cleanly: pull succeeded, templates/secrets deployed,
    /// triggers ran (some may have failed but the sync wasn't blocked).
    Ok,
    /// Pre-pull drift detection found modified/deleted tracked files.
    /// Pull was NOT attempted. User must resolve drift manually.
    AbortedDrift,
    /// `git pull` failed (network, conflict, auth). No deploy attempted.
    PullFailed,
    /// One or more triggers exited non-zero. Templates/secrets did deploy.
    TriggersFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatus {
    /// Machine name from local.toml.
    pub machine: String,
    /// When this attempt started.
    pub started_at: String,
    /// When this attempt finished (success or failure).
    pub completed_at: String,
    /// Outcome.
    pub result: SyncResult,
    /// Tracked files with local modifications/deletions (drift). Populated when
    /// result == AbortedDrift. Each entry is `<porcelain_status> <path>`
    /// (e.g., " M .cargo/config.toml", " D .zshrc").
    #[serde(default)]
    pub drift_files: Vec<String>,
    /// Number of templates whose targets had drift and were skipped (preserves
    /// local fixes per AC-9.2). Distinct from pre-pull drift.
    #[serde(default)]
    pub templates_skipped_drift: usize,
    /// Templates deployed cleanly.
    #[serde(default)]
    pub templates_deployed: usize,
    /// Triggers that ran successfully.
    #[serde(default)]
    pub triggers_succeeded: usize,
    /// Triggers that failed (exit non-zero).
    #[serde(default)]
    pub triggers_failed: usize,
    /// Free-form error messages (e.g., from `git pull`).
    #[serde(default)]
    pub errors: Vec<String>,
    /// ISO timestamp of the last AbortedDrift result, for staleness tracking.
    #[serde(default)]
    pub last_success_at: Option<String>,
}

impl SyncStatus {
    pub fn new(machine: String) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            machine,
            started_at: now.clone(),
            completed_at: now,
            result: SyncResult::Ok,
            drift_files: Vec::new(),
            templates_skipped_drift: 0,
            templates_deployed: 0,
            triggers_succeeded: 0,
            triggers_failed: 0,
            errors: Vec::new(),
            last_success_at: None,
        }
    }
}

fn default_status_path() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".local/share/nit/last-sync.json")
}

/// Load the previous sync status (if any). Used to preserve `last_success_at`
/// across runs.
pub fn load_status() -> Option<SyncStatus> {
    load_status_from(&default_status_path())
}

pub fn load_status_from(path: &std::path::Path) -> Option<SyncStatus> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write the status to disk. Atomic-ish (write + rename).
pub fn save_status(status: &SyncStatus) {
    save_status_to(status, &default_status_path());
}

pub fn save_status_to(status: &SyncStatus, path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = match serde_json::to_string_pretty(status) {
        Ok(s) => s,
        Err(_) => return,
    };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, &json).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Detect modified/deleted tracked files via `git status --porcelain`.
/// Returns a vec of porcelain lines (e.g., " M .cargo/config.toml") for any
/// drift that would block a pull. Untracked files (`??`) are NOT drift —
/// they're either gitignored or genuinely new and don't block pull.
pub fn detect_pre_pull_drift(porcelain_output: &str) -> Vec<String> {
    porcelain_output
        .lines()
        .filter(|line| {
            let prefix = line.get(0..2).unwrap_or("  ");
            // `??` = untracked, `!!` = ignored. Both safe to ignore for drift purposes.
            !prefix.starts_with("??") && !prefix.starts_with("!!") && line.len() >= 3
        })
        .map(|s| s.to_string())
        .collect()
}

/// One-line summary suitable for `nit status` output.
pub fn one_line_summary(status: &SyncStatus) -> String {
    let result_label = match status.result {
        SyncResult::Ok => "ok",
        SyncResult::AbortedDrift => "ABORTED (drift)",
        SyncResult::PullFailed => "FAILED (pull)",
        SyncResult::TriggersFailed => "ok-with-trigger-failures",
    };
    format!(
        "last sync: {} — {} at {}",
        result_label, status.machine, status.completed_at
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_pre_pull_drift_modified_file() {
        let porcelain = " M .cargo/config.toml\n";
        let drift = detect_pre_pull_drift(porcelain);
        assert_eq!(drift.len(), 1);
        assert!(drift[0].contains(".cargo/config.toml"));
    }

    #[test]
    fn test_detect_pre_pull_drift_deleted_file() {
        let porcelain = " D .zshrc\n";
        let drift = detect_pre_pull_drift(porcelain);
        assert_eq!(drift.len(), 1);
    }

    #[test]
    fn test_detect_pre_pull_drift_staged_modification() {
        let porcelain = "M  CLAUDE.md\n";
        let drift = detect_pre_pull_drift(porcelain);
        assert_eq!(drift.len(), 1);
    }

    #[test]
    fn test_detect_pre_pull_drift_untracked_ignored() {
        let porcelain = "?? Documents/new-file.txt\n?? .DS_Store\n";
        let drift = detect_pre_pull_drift(porcelain);
        assert!(drift.is_empty(), "untracked files must not count as drift");
    }

    #[test]
    fn test_detect_pre_pull_drift_mixed() {
        let porcelain = " M .cargo/config.toml\n?? .DS_Store\n D .zshrc\n M dotfiles/CLAUDE.md\n";
        let drift = detect_pre_pull_drift(porcelain);
        assert_eq!(
            drift.len(),
            3,
            "should detect 3 drifts (2 modified + 1 deleted), not the untracked"
        );
    }

    #[test]
    fn test_detect_pre_pull_drift_empty() {
        assert!(detect_pre_pull_drift("").is_empty());
        assert!(detect_pre_pull_drift("\n\n").is_empty());
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("last-sync.json");

        let mut status = SyncStatus::new("test-machine".to_string());
        status.result = SyncResult::AbortedDrift;
        status.drift_files = vec![" M .cargo/config.toml".to_string()];

        save_status_to(&status, &path);
        let loaded = load_status_from(&path).expect("status should load");

        assert_eq!(loaded.machine, "test-machine");
        assert!(matches!(loaded.result, SyncResult::AbortedDrift));
        assert_eq!(loaded.drift_files.len(), 1);
    }

    #[test]
    fn test_load_status_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        assert!(load_status_from(&path).is_none());
    }

    #[test]
    fn test_one_line_summary_ok() {
        let status = SyncStatus::new("macmini".to_string());
        let line = one_line_summary(&status);
        assert!(line.contains("ok"));
        assert!(line.contains("macmini"));
    }

    #[test]
    fn test_one_line_summary_drift_aborted() {
        let mut status = SyncStatus::new("merian".to_string());
        status.result = SyncResult::AbortedDrift;
        let line = one_line_summary(&status);
        assert!(line.contains("ABORTED"));
        assert!(line.contains("merian"));
    }
}
