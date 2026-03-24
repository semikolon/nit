//! Hash-based trigger system — runs scripts when watched files change

use crate::config::{NitConfig, TriggerDef};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Persisted state tracking file hashes per trigger
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TriggerState {
    /// trigger_name -> (relative_path -> sha256_hex)
    #[serde(default)]
    pub trigger_hashes: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    pub last_apply: Option<String>,
}

/// Result of checking whether a trigger's watched files changed
#[derive(Debug)]
pub enum TriggerCheck {
    /// Files changed — contains new hash map (relative_path -> sha256)
    Changed(HashMap<String, String>),
    /// All hashes match stored state
    Unchanged,
}

/// Status of a trigger run
#[derive(Debug, PartialEq)]
pub enum RunStatus {
    Success,
    Failed(i32),
    Skipped(String),
}

/// Result of running (or skipping) a trigger
#[derive(Debug)]
pub struct TriggerRunResult {
    pub name: String,
    pub status: RunStatus,
    pub log_path: PathBuf,
}

// ─── State persistence ───────────────────────────────────────────────

fn default_state_path() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".local/share/nit/state.json")
}

#[allow(dead_code)]
fn default_log_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".local/share/nit/logs")
}

/// Load trigger state from state.json (returns empty state if file missing)
pub fn load_trigger_state() -> TriggerState {
    load_trigger_state_from(&default_state_path())
}

/// Load trigger state from an explicit path (testable)
pub fn load_trigger_state_from(path: &Path) -> TriggerState {
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => TriggerState::default(),
    }
}

/// Save trigger state to the default path
pub fn save_trigger_state(state: &TriggerState) {
    save_trigger_state_to(state, &default_state_path());
}

/// Save trigger state to an explicit path (testable)
pub fn save_trigger_state_to(state: &TriggerState, path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(state).expect("failed to serialize trigger state");
    fs::write(path, json).expect("failed to write trigger state");
}

// ─── Hashing ─────────────────────────────────────────────────────────

/// SHA256 hex hash of a file's contents
pub fn hash_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

// ─── Glob resolution ─────────────────────────────────────────────────

/// Expand watch glob patterns relative to work_tree, return sorted matching file paths
pub fn resolve_watch_globs(watch: &[String], work_tree: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for pattern in watch {
        let full_pattern = work_tree.join(pattern);
        let pattern_str = full_pattern.to_string_lossy().to_string();
        if let Ok(entries) = glob::glob(&pattern_str) {
            for entry in entries.flatten() {
                if entry.is_file() {
                    paths.push(entry);
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

// ─── Trigger checking ────────────────────────────────────────────────

/// Hash all watched files and compare to stored state.
/// Returns Changed(new_hashes) if any file hash differs or is new, Unchanged otherwise.
pub fn check_trigger(
    trigger: &TriggerDef,
    state: &TriggerState,
    work_tree: &Path,
) -> TriggerCheck {
    let resolved = resolve_watch_globs(&trigger.watch, work_tree);
    let stored = state.trigger_hashes.get(&trigger.name);

    let mut new_hashes = HashMap::new();
    let mut changed = false;

    for path in &resolved {
        // Use path relative to work_tree as key
        let rel = path
            .strip_prefix(work_tree)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        match hash_file(path) {
            Ok(hash) => {
                if let Some(prev) = stored.and_then(|s| s.get(&rel)) {
                    if *prev != hash {
                        changed = true;
                    }
                } else {
                    // New file or new trigger — counts as changed
                    changed = true;
                }
                new_hashes.insert(rel, hash);
            }
            Err(_) => {
                // File disappeared between glob and hash — treat as changed
                changed = true;
            }
        }
    }

    // Also check if any previously-stored file is now missing
    if let Some(prev_map) = stored {
        for key in prev_map.keys() {
            if !new_hashes.contains_key(key) {
                changed = true;
                break;
            }
        }
    }

    // No prior state at all for this trigger → changed
    if stored.is_none() && !new_hashes.is_empty() {
        changed = true;
    }

    if changed {
        TriggerCheck::Changed(new_hashes)
    } else {
        TriggerCheck::Unchanged
    }
}

// ─── Trigger execution ──────────────────────────────────────────────

/// Execute a trigger script, capturing output to a log file.
/// Scripts run with cwd = project_dir.
pub fn run_trigger(
    trigger: &TriggerDef,
    project_dir: &Path,
    log_dir: &Path,
) -> Result<TriggerRunResult, Box<dyn std::error::Error>> {
    let _ = fs::create_dir_all(log_dir);
    let log_path = log_dir.join(format!("{}.log", trigger.name));

    let script_path = project_dir.join(&trigger.script);
    let output = Command::new("bash")
        .arg(&script_path)
        .current_dir(project_dir)
        .output()?;

    // Write combined stdout+stderr to log
    let mut log_content = Vec::new();
    log_content.extend_from_slice(&output.stdout);
    if !output.stderr.is_empty() {
        log_content.push(b'\n');
        log_content.extend_from_slice(&output.stderr);
    }
    fs::write(&log_path, &log_content)?;

    let status = if output.status.success() {
        RunStatus::Success
    } else {
        RunStatus::Failed(output.status.code().unwrap_or(-1))
    };

    Ok(TriggerRunResult {
        name: trigger.name.clone(),
        status,
        log_path,
    })
}

// ─── Orchestration ──────────────────────────────────────────────────

/// Run all applicable triggers that have changed watched files.
/// - skip_drifted: list of relative paths that are drifted — skip triggers watching these
/// - safe_mode: if true, skip all triggers (dry-run)
pub fn run_applicable_triggers(
    config: &NitConfig,
    state: &mut TriggerState,
    skip_drifted: &[String],
    safe_mode: bool,
    log_dir: &Path,
) -> Vec<TriggerRunResult> {
    let work_tree = dirs::home_dir().expect("cannot determine home directory");
    let mut results = Vec::new();

    for trigger in config.applicable_triggers() {
        // Check if any watched file is in the drifted list
        let resolved = resolve_watch_globs(&trigger.watch, &work_tree);
        let has_drifted = resolved.iter().any(|p| {
            let rel = p
                .strip_prefix(&work_tree)
                .unwrap_or(p)
                .to_string_lossy()
                .to_string();
            skip_drifted.contains(&rel)
        });

        if has_drifted {
            results.push(TriggerRunResult {
                name: trigger.name.clone(),
                status: RunStatus::Skipped("watched file has drift".to_string()),
                log_path: log_dir.join(format!("{}.log", trigger.name)),
            });
            continue;
        }

        if safe_mode {
            results.push(TriggerRunResult {
                name: trigger.name.clone(),
                status: RunStatus::Skipped("safe mode".to_string()),
                log_path: log_dir.join(format!("{}.log", trigger.name)),
            });
            continue;
        }

        match check_trigger(trigger, state, &work_tree) {
            TriggerCheck::Changed(new_hashes) => {
                match run_trigger(trigger, &config.project_dir, log_dir) {
                    Ok(result) => {
                        if result.status == RunStatus::Success {
                            // Update state only on success
                            state
                                .trigger_hashes
                                .insert(trigger.name.clone(), new_hashes);
                        }
                        results.push(result);
                    }
                    Err(_e) => {
                        results.push(TriggerRunResult {
                            name: trigger.name.clone(),
                            status: RunStatus::Failed(-1),
                            log_path: log_dir.join(format!("{}.log", trigger.name)),
                        });
                    }
                }
            }
            TriggerCheck::Unchanged => {
                // Nothing to do — trigger files haven't changed
            }
        }
    }

    state.last_apply = Some(Utc::now().to_rfc3339());
    results
}

/// Run a specific trigger by name, regardless of hash state
pub fn run_manual(
    trigger_name: &str,
    config: &NitConfig,
    log_dir: &Path,
) -> Result<TriggerRunResult, Box<dyn std::error::Error>> {
    let trigger = config
        .triggers
        .iter()
        .find(|t| t.name == trigger_name)
        .ok_or_else(|| format!("trigger '{}' not found", trigger_name))?;

    run_trigger(trigger, &config.project_dir, log_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_temp_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }

    // ─── hash_file ──────────────────────────────────────────────────

    #[test]
    fn test_hash_file_deterministic() {
        let dir = TempDir::new().unwrap();
        let path = make_temp_file(dir.path(), "test.txt", "hello world\n");

        let h1 = hash_file(&path).unwrap();
        let h2 = hash_file(&path).unwrap();
        assert_eq!(h1, h2);
        // SHA256 hex is always 64 chars
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_hash_file_known_value() {
        let dir = TempDir::new().unwrap();
        let path = make_temp_file(dir.path(), "known.txt", "nit");

        let hash = hash_file(&path).unwrap();
        // Compute expected: SHA256("nit")
        let mut hasher = Sha256::new();
        hasher.update(b"nit");
        let expected = hex::encode(hasher.finalize());
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_file_missing() {
        let result = hash_file(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    // ─── check_trigger: changed detection ───────────────────────────

    #[test]
    fn test_check_trigger_changed_new_hash() {
        let dir = TempDir::new().unwrap();
        make_temp_file(dir.path(), "config.toml", "version = 1");

        let trigger = TriggerDef {
            name: "test".to_string(),
            script: "test.sh".to_string(),
            watch: vec!["config.toml".to_string()],
            os: None,
            role: None,
        };

        // State has an old hash
        let mut state = TriggerState::default();
        let mut old = HashMap::new();
        old.insert("config.toml".to_string(), "oldhash".to_string());
        state.trigger_hashes.insert("test".to_string(), old);

        match check_trigger(&trigger, &state, dir.path()) {
            TriggerCheck::Changed(hashes) => {
                assert!(hashes.contains_key("config.toml"));
                assert_ne!(hashes["config.toml"], "oldhash");
            }
            TriggerCheck::Unchanged => panic!("expected Changed"),
        }
    }

    // ─── check_trigger: unchanged ───────────────────────────────────

    #[test]
    fn test_check_trigger_unchanged() {
        let dir = TempDir::new().unwrap();
        let path = make_temp_file(dir.path(), "config.toml", "version = 1");
        let current_hash = hash_file(&path).unwrap();

        let trigger = TriggerDef {
            name: "test".to_string(),
            script: "test.sh".to_string(),
            watch: vec!["config.toml".to_string()],
            os: None,
            role: None,
        };

        let mut state = TriggerState::default();
        let mut stored = HashMap::new();
        stored.insert("config.toml".to_string(), current_hash);
        state.trigger_hashes.insert("test".to_string(), stored);

        match check_trigger(&trigger, &state, dir.path()) {
            TriggerCheck::Unchanged => {} // correct
            TriggerCheck::Changed(_) => panic!("expected Unchanged"),
        }
    }

    // ─── check_trigger: new trigger (no prior state) ────────────────

    #[test]
    fn test_check_trigger_new_trigger() {
        let dir = TempDir::new().unwrap();
        make_temp_file(dir.path(), "Brewfile", "brew 'git'");

        let trigger = TriggerDef {
            name: "install-packages".to_string(),
            script: "install.sh".to_string(),
            watch: vec!["Brewfile".to_string()],
            os: None,
            role: None,
        };

        let state = TriggerState::default(); // empty — no prior state

        match check_trigger(&trigger, &state, dir.path()) {
            TriggerCheck::Changed(hashes) => {
                assert!(hashes.contains_key("Brewfile"));
            }
            TriggerCheck::Unchanged => panic!("expected Changed for new trigger"),
        }
    }

    // ─── run_trigger: success ───────────────────────────────────────

    #[test]
    fn test_run_trigger_success() {
        let project_dir = TempDir::new().unwrap();
        let log_dir = TempDir::new().unwrap();

        // Create a script that exits 0
        let script_path = project_dir.path().join("scripts");
        fs::create_dir_all(&script_path).unwrap();
        let script = script_path.join("ok.sh");
        fs::write(&script, "#!/bin/bash\necho 'all good'\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let trigger = TriggerDef {
            name: "ok-trigger".to_string(),
            script: "scripts/ok.sh".to_string(),
            watch: vec![],
            os: None,
            role: None,
        };

        let result = run_trigger(&trigger, project_dir.path(), log_dir.path()).unwrap();
        assert_eq!(result.name, "ok-trigger");
        assert_eq!(result.status, RunStatus::Success);
        assert!(result.log_path.exists());

        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("all good"));
    }

    // ─── run_trigger: failure ───────────────────────────────────────

    #[test]
    fn test_run_trigger_failed() {
        let project_dir = TempDir::new().unwrap();
        let log_dir = TempDir::new().unwrap();

        let script_path = project_dir.path().join("scripts");
        fs::create_dir_all(&script_path).unwrap();
        let script = script_path.join("fail.sh");
        fs::write(&script, "#!/bin/bash\necho 'oops' >&2\nexit 1\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let trigger = TriggerDef {
            name: "fail-trigger".to_string(),
            script: "scripts/fail.sh".to_string(),
            watch: vec![],
            os: None,
            role: None,
        };

        let result = run_trigger(&trigger, project_dir.path(), log_dir.path()).unwrap();
        assert_eq!(result.name, "fail-trigger");
        assert_eq!(result.status, RunStatus::Failed(1));
        assert!(result.log_path.exists());

        let log = fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("oops"));
    }

    // ─── skip_drifted ───────────────────────────────────────────────

    #[test]
    fn test_skip_drifted_trigger() {
        let dir = TempDir::new().unwrap();
        make_temp_file(dir.path(), "Brewfile", "brew 'git'");

        let trigger = TriggerDef {
            name: "install".to_string(),
            script: "install.sh".to_string(),
            watch: vec!["Brewfile".to_string()],
            os: None,
            role: None,
        };

        let resolved = resolve_watch_globs(&trigger.watch, dir.path());
        let skip_drifted = vec!["Brewfile".to_string()];

        // Simulate the skip logic from run_applicable_triggers
        let has_drifted = resolved.iter().any(|p| {
            let rel = p
                .strip_prefix(dir.path())
                .unwrap_or(p)
                .to_string_lossy()
                .to_string();
            skip_drifted.contains(&rel)
        });

        assert!(has_drifted, "trigger should be skipped when watched file is drifted");
    }

    // ─── state persistence roundtrip ────────────────────────────────

    #[test]
    fn test_state_save_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let state_path = dir.path().join("state.json");

        let mut state = TriggerState::default();
        state.last_apply = Some("2026-03-24T12:00:00Z".to_string());
        let mut hashes = HashMap::new();
        hashes.insert("file.txt".to_string(), "abc123".to_string());
        state
            .trigger_hashes
            .insert("my-trigger".to_string(), hashes);

        save_trigger_state_to(&state, &state_path);
        let loaded = load_trigger_state_from(&state_path);

        assert_eq!(loaded.last_apply, state.last_apply);
        assert_eq!(
            loaded.trigger_hashes["my-trigger"]["file.txt"],
            "abc123"
        );
    }

    #[test]
    fn test_load_state_missing_file() {
        let state = load_trigger_state_from(Path::new("/nonexistent/state.json"));
        assert!(state.trigger_hashes.is_empty());
        assert!(state.last_apply.is_none());
    }

    // ─── resolve_watch_globs ────────────────────────────────────────

    #[test]
    fn test_resolve_watch_globs_basic() {
        let dir = TempDir::new().unwrap();
        make_temp_file(dir.path(), "foo.toml", "a");
        make_temp_file(dir.path(), "bar.toml", "b");
        make_temp_file(dir.path(), "baz.json", "c");

        let patterns = vec!["*.toml".to_string()];
        let matched = resolve_watch_globs(&patterns, dir.path());

        assert_eq!(matched.len(), 2);
        let names: Vec<String> = matched
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"foo.toml".to_string()));
        assert!(names.contains(&"bar.toml".to_string()));
        assert!(!names.contains(&"baz.json".to_string()));
    }

    #[test]
    fn test_resolve_watch_globs_nested() {
        let dir = TempDir::new().unwrap();
        make_temp_file(dir.path(), "src/main.rs", "fn main() {}");
        make_temp_file(dir.path(), "src/lib.rs", "// lib");
        make_temp_file(dir.path(), "README.md", "# hi");

        let patterns = vec!["src/**/*.rs".to_string()];
        let matched = resolve_watch_globs(&patterns, dir.path());

        assert_eq!(matched.len(), 2);
    }

    #[test]
    fn test_resolve_watch_globs_no_match() {
        let dir = TempDir::new().unwrap();
        make_temp_file(dir.path(), "hello.txt", "hi");

        let patterns = vec!["*.rs".to_string()];
        let matched = resolve_watch_globs(&patterns, dir.path());

        assert!(matched.is_empty());
    }

    #[test]
    fn test_resolve_watch_globs_literal_file() {
        let dir = TempDir::new().unwrap();
        make_temp_file(dir.path(), ".Brewfile", "brew 'git'");

        let patterns = vec![".Brewfile".to_string()];
        let matched = resolve_watch_globs(&patterns, dir.path());

        assert_eq!(matched.len(), 1);
        assert!(matched[0].ends_with(".Brewfile"));
    }
}
