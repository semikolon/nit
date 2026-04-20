//! Sync-base storage + drift detection (NO auto-merge, ever)
//!
//! Sync-base stores the last-deployed content for each template target.
//! Drift detection compares current target content against the sync-base
//! to determine if the target was edited outside nit.
//!
//! Ack system provides per-PPID commit gates — each CC session writes
//! only its own ack file, no locks needed.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Directory helpers
// ---------------------------------------------------------------------------

/// Root data directory: ~/.local/share/nit/
pub fn nit_data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".local/share/nit")
}

/// Sync-base directory: ~/.local/share/nit/sync-base/
pub fn sync_base_dir() -> PathBuf {
    nit_data_dir().join("sync-base")
}

/// Drift directory: ~/.local/share/nit/drift/
pub fn drift_dir() -> PathBuf {
    nit_data_dir().join("drift")
}

/// Acks directory: ~/.local/share/nit/acks/
pub fn acks_dir() -> PathBuf {
    nit_data_dir().join("acks")
}

// ---------------------------------------------------------------------------
// Sync-base operations
// ---------------------------------------------------------------------------

/// Read the last-deployed content for a target (relative path like ".zshenv").
/// Returns None if no sync-base exists yet (first deploy).
pub fn read_sync_base(target_rel: &str) -> Option<String> {
    let path = sync_base_dir().join(target_rel);
    fs::read_to_string(path).ok()
}

/// Write content to sync-base after a successful deploy.
/// Creates parent directories as needed. Uses atomic write (temp + rename).
pub fn write_sync_base(target_rel: &str, content: &str) {
    let path = sync_base_dir().join(target_rel);
    atomic_write(&path, content);
}

// ---------------------------------------------------------------------------
// Drift detection
// ---------------------------------------------------------------------------

/// Compare target content against sync-base.
/// Returns a unified diff string if they differ, None if identical or no base exists.
pub fn detect_drift(target_rel: &str, target_content: &str) -> Option<String> {
    let base_content = read_sync_base(target_rel)?;
    if base_content == target_content {
        return None;
    }

    let diff = TextDiff::from_lines(base_content.as_str(), target_content);
    let mut output = String::new();
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        output.push_str(sign);
        output.push_str(change.as_str().unwrap_or(""));
        // Ensure trailing newline for each line
        if !change.as_str().unwrap_or("").ends_with('\n') {
            output.push('\n');
        }
    }
    Some(output)
}

/// Save a drift diff to the drift directory.
pub fn save_drift(target_rel: &str, diff: &str) {
    let path = drift_dir().join(format!("{}.diff", target_rel));
    atomic_write(&path, diff);
}

/// Read a saved drift diff.
pub fn read_drift(target_rel: &str) -> Option<String> {
    let path = drift_dir().join(format!("{}.diff", target_rel));
    fs::read_to_string(path).ok()
}

/// Dismiss drift: read + delete the drift file. Returns the diff that was dismissed.
pub fn dismiss_drift(target_rel: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = drift_dir().join(format!("{}.diff", target_rel));
    let diff = fs::read_to_string(&path)
        .map_err(|e| format!("no drift saved for '{}': {}", target_rel, e))?;
    fs::remove_file(&path)?;
    Ok(diff)
}

/// List all files with saved drift (returns relative target paths).
pub fn list_drifted_files() -> Vec<String> {
    let dir = drift_dir();
    if !dir.exists() {
        return Vec::new();
    }

    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(rel) = name.strip_suffix(".diff") {
                files.push(rel.to_string());
            }
        }
    }
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Ack system (per-PPID commit gate)
// ---------------------------------------------------------------------------

/// A single ack entry for one template target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AckEntry {
    pub target_hash: String,
    pub rendered_hash: String,
    pub timestamp: String,
}

/// Get the parent PID of the current process (raw getppid syscall).
/// This is the IMMEDIATE parent — for stable session identity across
/// ephemeral shells (e.g., CC's Bash tool), use `get_session_anchor()`.
pub fn get_ppid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: getppid() has no side effects, always succeeds
        unsafe { getppid() as u32 }
    }
    #[cfg(not(unix))]
    {
        // Fallback for non-unix (shouldn't happen for this project)
        std::process::id()
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn getppid() -> i32;
}

/// Process name patterns that, when found in the parent chain, mark the
/// SESSION anchor — the agent IS the session. Match is on the basename
/// of the parent's `comm` (case-insensitive substring).
///
/// Add new agentic engineering harnesses here as they emerge. Long-term,
/// each agent should expose a stable `AGENT_SESSION_ID` env var so this
/// list isn't needed.
const KNOWN_AGENTS: &[&str] = &[
    "claude",       // Anthropic Claude Code
    "codex",        // OpenAI Codex CLI
    "cursor-agent", // Cursor agent mode
    "aider",        // Aider AI pair programmer
    "opencode",     // OpenCode
    "amp",          // Sourcegraph Amp
];

/// Process name patterns that mark the OUTER boundary — terminal emulators,
/// shell launchers, init systems. Walking up STOPS at these; the PID just
/// BELOW the boundary is the session anchor (the topmost shell in the
/// session). Match on basename of `comm` (case-insensitive substring).
const KNOWN_BOUNDARIES: &[&str] = &[
    // Terminal emulators
    "ghostty", "kitty", "alacritty", "iterm2", "iterm",
    "wezterm", "terminal", "warp",
    // Multiplexers / session managers
    "tmux", "screen", "zellij",
    // Remote / login / init
    "sshd", "login", "launchd", "systemd", "init",
    // Cron / job runners (each invocation = new session)
    "cron", "crond",
];

/// Find the most stable ancestor of the current process — the "session anchor".
///
/// **Why this exists:** the spec defines "session" as a stable agent identity
/// (one Claude Code conversation, one shell session, one cron run), not an
/// ephemeral shell. CC's Bash tool spawns a fresh `zsh -c "..."` per command,
/// so `getppid()` returns a different value EACH `nit` invocation within the
/// same conversation. Per-PPID acks then become per-Bash-call acks, breaking
/// the per-session semantic. This function walks up the parent chain to find
/// the actual session anchor.
///
/// **Walk semantics:**
/// - Stop ABOVE on `KNOWN_AGENTS`: the agent process IS the session anchor →
///   return its PID. (Example: walking from nit → bash → claude. claude is
///   the agent → return claude's PID. Stable across all of CC's Bash calls.)
/// - Stop BELOW on `KNOWN_BOUNDARIES`: terminal/init is outside the session →
///   return the PID just below the boundary (the topmost shell). (Example:
///   walking from nit → zsh → ghostty. ghostty is the boundary → return zsh's
///   PID. Stable across all commands in that shell.)
/// - Fallback to direct PPID if no anchor found in 16 hops.
pub fn get_session_anchor() -> u32 {
    let mut current = std::process::id();

    // Cap walk depth to avoid pathological process trees
    for _ in 0..16 {
        let parent = ppid_of(current);
        if parent <= 1 || parent == current {
            // Hit init/launchd/orphan or self-loop — current is the deepest
            // reasonable anchor.
            return current;
        }
        let parent_comm_basename = comm_basename(parent).to_lowercase();
        if parent_comm_basename.is_empty() {
            // ps lookup failed — fall back to direct PPID
            return get_ppid();
        }

        // STOP ABOVE: the agent process IS the session anchor
        if KNOWN_AGENTS
            .iter()
            .any(|a| parent_comm_basename.contains(a))
        {
            return parent;
        }
        // STOP BELOW: terminal/launcher boundary; we are the topmost shell
        if KNOWN_BOUNDARIES
            .iter()
            .any(|b| parent_comm_basename.contains(b))
        {
            return current;
        }

        current = parent;
    }

    // Walk depth exhausted: deepest reached is our best guess
    current
}

/// Look up the parent PID of an arbitrary PID via `ps`. Returns 0 on failure.
fn ppid_of(pid: u32) -> u32 {
    if pid == std::process::id() {
        return get_ppid();
    }
    let output = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0),
        _ => 0,
    }
}

/// Look up the basename of `comm` for a PID via `ps`. Returns "" on failure.
/// `ps -o comm=` returns the full path on macOS; we basename it for matching.
fn comm_basename(pid: u32) -> String {
    let output = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output();
    let raw = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => return String::new(),
    };
    // Some macOS shells appear as `-zsh` or `-/bin/zsh` (login shell). Strip
    // any leading `-` before basename.
    let raw = raw.trim_start_matches('-');
    raw.rsplit('/').next().unwrap_or("").to_string()
}

/// Path to a specific PPID's ack file.
pub fn ack_file_path(ppid: u32) -> PathBuf {
    acks_dir().join(format!("{}.json", ppid))
}

/// Read all ack entries for a given PPID.
pub fn read_acks(ppid: u32) -> HashMap<String, AckEntry> {
    let path = ack_file_path(ppid);
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

/// Write an ack entry for the current SESSION ANCHOR (not raw PPID).
/// Reads existing acks for this anchor, adds/updates the entry, writes atomically.
pub fn write_ack(target_rel: &str, target_hash: &str, rendered_hash: &str) {
    let anchor = get_session_anchor();
    write_ack_for_ppid(anchor, target_rel, target_hash, rendered_hash);
}

/// Write an ack entry for a specific PPID (testable).
pub fn write_ack_for_ppid(ppid: u32, target_rel: &str, target_hash: &str, rendered_hash: &str) {
    let mut acks = read_acks(ppid);
    acks.insert(
        target_rel.to_string(),
        AckEntry {
            target_hash: target_hash.to_string(),
            rendered_hash: rendered_hash.to_string(),
            timestamp: Utc::now().to_rfc3339(),
        },
    );
    let json = serde_json::to_string_pretty(&acks).expect("failed to serialize acks");
    atomic_write(&ack_file_path(ppid), &json);
}

// NOTE (Apr 21, 2026): `find_cross_session_ack` was removed.
//
// The original v5 design (Mar 24, 2026) included cross-session ack reuse:
// when committing a template source, if no own-session ack existed, scan
// ALL session ack files and proceed if any matched the current state.
// Rationale at the time: "same safety, less friction" (tasks.md:276) —
// avoid forcing the second-call dance when another session had already
// reviewed identical state.
//
// Why removed:
// 1. The named contamination incidents (sccache, Flux client, CLAUDE.md
//    revert) are defended by source-wins + no-auto-merge, NOT by acks.
//    Cross-session reuse wasn't load-bearing for any incident.
// 2. The "two pairs of eyes" framing relied on the committer engaging with
//    drift output, which is structurally weaker than the explicit two-call
//    pattern (especially for AI agents who batch-scroll output).
// 3. Per-agent accountability is cleaner: the committing agent ALWAYS
//    reviewed first.
// 4. With session-anchor walk-up (replacing raw PPID), within one CC
//    conversation all Bash calls share an anchor — the two-call dance is
//    one-call from the user's POV (call 1 writes ack, call 2 commits).
// 5. Multi-session friction (genuinely distinct sessions wanting to commit
//    overlapping templates) is bounded: one extra `nit commit` per agent.
//    Acceptable cost for the conceptual cleanup.
//
// If real-world friction at fleet scale becomes painful, re-add behind an
// explicit opt-in flag (`--accept-cross-session-ack` or equivalent) — never
// implicit.

/// Prune ack files whose anchor PID is no longer running.
///
/// Pure housekeeping — with cross-session ack reuse removed, dead-anchor acks
/// are unreachable cruft (only the OWN anchor's ack file is read at commit
/// time). Aggressive pruning is safe and keeps the directory tidy.
pub fn prune_dead_acks() {
    let dir = acks_dir();
    if !dir.exists() {
        return;
    }

    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(ppid_str) = name.strip_suffix(".json")
            && let Ok(pid) = ppid_str.parse::<u32>()
            && !is_pid_alive(pid)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Check if a PID is alive using kill(pid, 0).
fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill with signal 0 just checks if the process exists
        // SAFETY: kill(pid, 0) just checks if the process exists
        unsafe { kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// Compute SHA256 hash of content, returned as "sha256:<hex>".
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    format!("sha256:{}", hex::encode(result))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Atomic write: write to temp file in same directory, then rename.
/// Creates parent directories as needed.
fn atomic_write(path: &PathBuf, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("failed to create parent directories");
    }

    let temp_path = path.with_extension("tmp");
    let mut file = fs::File::create(&temp_path).expect("failed to create temp file");
    file.write_all(content.as_bytes())
        .expect("failed to write temp file");
    file.sync_all().expect("failed to sync temp file");
    fs::rename(&temp_path, path).expect("failed to rename temp file");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a test sync-base/drift/acks environment in a temp dir.
    /// Returns the temp dir (must be kept alive for the test duration).
    /// Overrides the directory functions by using the returned paths directly.
    struct TestEnv {
        _dir: tempfile::TempDir,
        sync_base: PathBuf,
        drift: PathBuf,
        acks: PathBuf,
    }

    impl TestEnv {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let sync_base = dir.path().join("sync-base");
            let drift = dir.path().join("drift");
            let acks = dir.path().join("acks");
            fs::create_dir_all(&sync_base).unwrap();
            fs::create_dir_all(&drift).unwrap();
            fs::create_dir_all(&acks).unwrap();
            TestEnv {
                _dir: dir,
                sync_base,
                drift,
                acks,
            }
        }

        // Sync-base helpers that operate on the test directory
        fn write_base(&self, rel: &str, content: &str) {
            let path = self.sync_base.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            atomic_write(&path, content);
        }

        fn read_base(&self, rel: &str) -> Option<String> {
            fs::read_to_string(self.sync_base.join(rel)).ok()
        }

        fn detect_drift(&self, rel: &str, target_content: &str) -> Option<String> {
            let base = self.read_base(rel)?;
            if base == target_content {
                return None;
            }
            let diff = TextDiff::from_lines(base.as_str(), target_content);
            let mut output = String::new();
            for change in diff.iter_all_changes() {
                let sign = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                output.push_str(sign);
                output.push_str(change.as_str().unwrap_or(""));
                if !change.as_str().unwrap_or("").ends_with('\n') {
                    output.push('\n');
                }
            }
            Some(output)
        }

        fn save_drift(&self, rel: &str, diff: &str) {
            let path = self.drift.join(format!("{}.diff", rel));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            atomic_write(&path, diff);
        }

        fn read_drift(&self, rel: &str) -> Option<String> {
            fs::read_to_string(self.drift.join(format!("{}.diff", rel))).ok()
        }

        fn dismiss_drift(&self, rel: &str) -> Result<String, Box<dyn std::error::Error>> {
            let path = self.drift.join(format!("{}.diff", rel));
            let diff = fs::read_to_string(&path)?;
            fs::remove_file(&path)?;
            Ok(diff)
        }

        fn list_drifted(&self) -> Vec<String> {
            let mut files = Vec::new();
            if let Ok(entries) = fs::read_dir(&self.drift) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if let Some(rel) = name.strip_suffix(".diff") {
                        files.push(rel.to_string());
                    }
                }
            }
            files.sort();
            files
        }

        // Ack helpers that operate on the test directory
        fn ack_path(&self, ppid: u32) -> PathBuf {
            self.acks.join(format!("{}.json", ppid))
        }

        fn write_ack_entry(&self, ppid: u32, rel: &str, target_hash: &str, rendered_hash: &str) {
            let path = self.ack_path(ppid);
            let mut acks: HashMap<String, AckEntry> = match fs::read_to_string(&path) {
                Ok(c) => serde_json::from_str(&c).unwrap_or_default(),
                Err(_) => HashMap::new(),
            };
            acks.insert(
                rel.to_string(),
                AckEntry {
                    target_hash: target_hash.to_string(),
                    rendered_hash: rendered_hash.to_string(),
                    timestamp: Utc::now().to_rfc3339(),
                },
            );
            let json = serde_json::to_string_pretty(&acks).unwrap();
            atomic_write(&path, &json);
        }

        fn read_acks_for(&self, ppid: u32) -> HashMap<String, AckEntry> {
            let path = self.ack_path(ppid);
            match fs::read_to_string(&path) {
                Ok(c) => serde_json::from_str(&c).unwrap_or_default(),
                Err(_) => HashMap::new(),
            }
        }

        fn find_cross_session(
            &self,
            rel: &str,
            target_hash: &str,
            rendered_hash: &str,
        ) -> Option<u32> {
            let entries = fs::read_dir(&self.acks).ok()?;
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(ppid_str) = name.strip_suffix(".json") {
                    if let Ok(ppid) = ppid_str.parse::<u32>() {
                        let acks = self.read_acks_for(ppid);
                        if let Some(ack) = acks.get(rel) {
                            if ack.target_hash == target_hash && ack.rendered_hash == rendered_hash
                            {
                                return Some(ppid);
                            }
                        }
                    }
                }
            }
            None
        }

        fn prune_dead(&self) {
            let entries = match fs::read_dir(&self.acks) {
                Ok(e) => e,
                Err(_) => return,
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(ppid_str) = name.strip_suffix(".json") {
                    if let Ok(pid) = ppid_str.parse::<u32>() {
                        if !is_pid_alive(pid) {
                            let _ = fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Sync-base tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_sync_base_write_and_read() {
        let env = TestEnv::new();
        env.write_base(".zshenv", "export PATH=/usr/bin\n");
        let content = env.read_base(".zshenv");
        assert_eq!(content, Some("export PATH=/usr/bin\n".to_string()));
    }

    #[test]
    fn test_sync_base_read_nonexistent() {
        let env = TestEnv::new();
        assert_eq!(env.read_base(".nonexistent"), None);
    }

    #[test]
    fn test_sync_base_overwrite() {
        let env = TestEnv::new();
        env.write_base(".zshenv", "old content\n");
        env.write_base(".zshenv", "new content\n");
        assert_eq!(env.read_base(".zshenv"), Some("new content\n".to_string()));
    }

    #[test]
    fn test_detect_drift_no_base() {
        let env = TestEnv::new();
        // No sync-base exists → no drift (first deploy)
        let drift = env.detect_drift(".zshenv", "some content\n");
        assert!(drift.is_none());
    }

    #[test]
    fn test_detect_drift_no_change() {
        let env = TestEnv::new();
        let content = "export PATH=/usr/bin\n";
        env.write_base(".zshenv", content);
        let drift = env.detect_drift(".zshenv", content);
        assert!(drift.is_none());
    }

    #[test]
    fn test_detect_drift_with_change() {
        let env = TestEnv::new();
        env.write_base(".zshenv", "line1\nline2\n");
        let drift = env.detect_drift(".zshenv", "line1\nline2\nline3\n");
        assert!(drift.is_some());
        let diff = drift.unwrap();
        assert!(diff.contains("+line3"));
    }

    #[test]
    fn test_detect_drift_deletion() {
        let env = TestEnv::new();
        env.write_base(".zshenv", "line1\nline2\nline3\n");
        let drift = env.detect_drift(".zshenv", "line1\nline3\n");
        assert!(drift.is_some());
        let diff = drift.unwrap();
        assert!(diff.contains("-line2"));
    }

    #[test]
    fn test_save_and_read_drift() {
        let env = TestEnv::new();
        let diff = "+line3\n-line2\n";
        env.save_drift(".zshenv", diff);
        assert_eq!(env.read_drift(".zshenv"), Some(diff.to_string()));
    }

    #[test]
    fn test_read_drift_nonexistent() {
        let env = TestEnv::new();
        assert_eq!(env.read_drift(".nonexistent"), None);
    }

    #[test]
    fn test_dismiss_drift() {
        let env = TestEnv::new();
        let diff = "+added line\n";
        env.save_drift(".zshenv", diff);
        let dismissed = env.dismiss_drift(".zshenv").unwrap();
        assert_eq!(dismissed, diff);
        // Should be gone after dismiss
        assert_eq!(env.read_drift(".zshenv"), None);
    }

    #[test]
    fn test_dismiss_drift_nonexistent() {
        let env = TestEnv::new();
        let result = env.dismiss_drift(".nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_drifted_files_empty() {
        let env = TestEnv::new();
        assert!(env.list_drifted().is_empty());
    }

    #[test]
    fn test_list_drifted_files_multiple() {
        let env = TestEnv::new();
        env.save_drift(".zshenv", "+drift1\n");
        env.save_drift(".gitconfig", "+drift2\n");
        env.save_drift(".zprofile", "+drift3\n");
        let drifted = env.list_drifted();
        assert_eq!(drifted, vec![".gitconfig", ".zprofile", ".zshenv"]);
    }

    // -----------------------------------------------------------------------
    // Ack system tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_ack_write_and_read() {
        let env = TestEnv::new();
        let t_hash = hash_content("target content");
        let r_hash = hash_content("rendered content");
        env.write_ack_entry(12345, ".zshenv", &t_hash, &r_hash);

        let acks = env.read_acks_for(12345);
        assert!(acks.contains_key(".zshenv"));
        let entry = &acks[".zshenv"];
        assert_eq!(entry.target_hash, t_hash);
        assert_eq!(entry.rendered_hash, r_hash);
        assert!(!entry.timestamp.is_empty());
    }

    #[test]
    fn test_ack_read_nonexistent_ppid() {
        let env = TestEnv::new();
        let acks = env.read_acks_for(99999);
        assert!(acks.is_empty());
    }

    #[test]
    fn test_ack_per_ppid_isolation() {
        let env = TestEnv::new();
        let hash_a = hash_content("content A");
        let hash_b = hash_content("content B");
        let rendered = hash_content("rendered");

        env.write_ack_entry(11111, ".zshenv", &hash_a, &rendered);
        env.write_ack_entry(22222, ".zshenv", &hash_b, &rendered);

        let acks_a = env.read_acks_for(11111);
        let acks_b = env.read_acks_for(22222);

        assert_eq!(acks_a[".zshenv"].target_hash, hash_a);
        assert_eq!(acks_b[".zshenv"].target_hash, hash_b);
    }

    #[test]
    fn test_ack_multiple_files_same_ppid() {
        let env = TestEnv::new();
        let h1 = hash_content("c1");
        let h2 = hash_content("c2");
        let r = hash_content("r");

        env.write_ack_entry(12345, ".zshenv", &h1, &r);
        env.write_ack_entry(12345, ".gitconfig", &h2, &r);

        let acks = env.read_acks_for(12345);
        assert_eq!(acks.len(), 2);
        assert!(acks.contains_key(".zshenv"));
        assert!(acks.contains_key(".gitconfig"));
    }

    #[test]
    fn test_ack_atomic_write_produces_valid_json() {
        let env = TestEnv::new();
        let h = hash_content("test");
        env.write_ack_entry(12345, ".zshenv", &h, &h);

        // Read raw file and verify it's valid JSON
        let raw = fs::read_to_string(env.ack_path(12345)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(parsed.is_object());
        assert!(parsed.get(".zshenv").is_some());
    }

    #[test]
    fn test_cross_session_ack_found() {
        let env = TestEnv::new();
        let t_hash = hash_content("target");
        let r_hash = hash_content("rendered");

        env.write_ack_entry(11111, ".zshenv", &t_hash, &r_hash);

        let found = env.find_cross_session(".zshenv", &t_hash, &r_hash);
        assert_eq!(found, Some(11111));
    }

    #[test]
    fn test_cross_session_ack_not_found_different_target_hash() {
        let env = TestEnv::new();
        let t_hash = hash_content("target");
        let r_hash = hash_content("rendered");

        env.write_ack_entry(11111, ".zshenv", &t_hash, &r_hash);

        let different = hash_content("different target");
        let found = env.find_cross_session(".zshenv", &different, &r_hash);
        assert_eq!(found, None);
    }

    #[test]
    fn test_cross_session_ack_not_found_different_rendered_hash() {
        let env = TestEnv::new();
        let t_hash = hash_content("target");
        let r_hash = hash_content("rendered");

        env.write_ack_entry(11111, ".zshenv", &t_hash, &r_hash);

        let different = hash_content("different rendered");
        let found = env.find_cross_session(".zshenv", &t_hash, &different);
        assert_eq!(found, None);
    }

    #[test]
    fn test_cross_session_ack_empty_dir() {
        let env = TestEnv::new();
        let found = env.find_cross_session(".zshenv", "h1", "h2");
        assert_eq!(found, None);
    }

    #[test]
    fn test_prune_dead_acks() {
        let env = TestEnv::new();
        let h = hash_content("test");

        // Use current process PID (always alive and accessible)
        let my_pid = std::process::id();
        env.write_ack_entry(my_pid, ".zshenv", &h, &h);
        // PID 99999999 almost certainly does not exist
        env.write_ack_entry(99999999, ".gitconfig", &h, &h);

        env.prune_dead();

        // Our own PID's ack should survive
        assert!(env.ack_path(my_pid).exists());
        // PID 99999999's ack should be pruned
        assert!(!env.ack_path(99999999).exists());
    }

    #[test]
    fn test_hash_content_deterministic() {
        let h1 = hash_content("hello world");
        let h2 = hash_content("hello world");
        assert_eq!(h1, h2);
        assert!(h1.starts_with("sha256:"));
    }

    #[test]
    fn test_hash_content_different_inputs() {
        let h1 = hash_content("hello");
        let h2 = hash_content("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_content_empty_string() {
        let h = hash_content("");
        assert!(h.starts_with("sha256:"));
        // SHA256 of empty string is well-known
        assert_eq!(
            h,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // -----------------------------------------------------------------------
    // Integration: public API tests (use real paths via nit_data_dir)
    // These use unique suffixes to avoid interfering with real data.
    // -----------------------------------------------------------------------

    #[test]
    fn test_public_api_nit_data_dir() {
        let dir = nit_data_dir();
        assert!(dir.to_string_lossy().contains(".local/share/nit"));
    }

    #[test]
    fn test_public_api_sync_base_dir() {
        let dir = sync_base_dir();
        assert!(dir.to_string_lossy().ends_with("sync-base"));
    }

    #[test]
    fn test_public_api_drift_dir() {
        let dir = drift_dir();
        assert!(dir.to_string_lossy().ends_with("drift"));
    }

    #[test]
    fn test_public_api_acks_dir() {
        let dir = acks_dir();
        assert!(dir.to_string_lossy().ends_with("acks"));
    }

    #[test]
    fn test_get_ppid_returns_nonzero() {
        let ppid = get_ppid();
        assert!(ppid > 0);
    }

    #[test]
    fn test_ack_entry_serialization_roundtrip() {
        let entry = AckEntry {
            target_hash: "sha256:abc123".to_string(),
            rendered_hash: "sha256:def456".to_string(),
            timestamp: "2026-03-24T10:30:00Z".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: AckEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, parsed);
    }
}
