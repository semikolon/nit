//! Drift queue — the evidence a drift steward needs, gathered without judgement.
//!
//! The steward is Claude in a live session, deciding per edited file what to do:
//! commit it, discard it, leave it, push it, or ask Fredrik (his ruling of
//! 2026-10-15: *"Claude is the steward … should be able to find out, in talking
//! with me and inspecting the state of the session that made edits, whether the
//! changes are unfinished or not ready to spread"*). nit gets no judgement and no
//! model; it writes down what can be known mechanically, to
//! `~/.local/share/nit/drift-queue.json`, and a hook shows that to a session.
//!
//! Per edited tracked file the queue records:
//! * whether its bytes are already in git history (stale, safe to discard) or new;
//! * when the path was last committed, and when the file was last written;
//! * which agent sessions touched the path since that commit, from their
//!   transcripts, and whether each session is still running.
//!
//! The session evidence is Claude Code's: transcripts under
//! `~/.claude/projects/**/*.jsonl` and the running-session records in
//! `~/.claude/sessions/<pid>.json`. A tool call that edits the path
//! (`Write`, `Edit`, `MultiEdit`, `NotebookEdit` with that `file_path`) is an
//! edit. Any other tool call whose input names the path (a shell command, an
//! edits file written for `apply-edits`) is recorded as "named", because it
//! may be an edit or only a read, and the steward reads the transcript to tell.
//! Measured on the 2026-10-15 Mac deadlock: of three blocking files, one had an
//! edit before its last commit and nothing after, two were named only by shell
//! commands. So "no session found" is a normal answer, and it means: edited by
//! hand, by a program, or older than the search window.
//!
//! Codex sessions are read too (`~/.codex/sessions`, since 2026-10-15): their
//! shell commands are searched for the path, written absolute, `~/…`, or
//! relative to the session's cwd. Codex edits through Python and shell, so its
//! touches are "named in Codex shell", or "edited? (Codex shell)" when that
//! command visibly writes; whether the thread is still open is unknown.
//!
//! Design: `docs/forward_only_drift_steward_design_2026-07-11.md` § 13 and § 10.4.

use crate::config::{GitStrategy, NitConfig};
use crate::drift_triage::{self, DriftKind};
use crate::git;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How far back the transcript search reaches, at most. A path last committed
/// months ago would otherwise mean reading every transcript on the machine
/// (7 GB on the Mac in October 2026); edits older than this are reported as
/// "not searched", never as "none".
pub const SEARCH_WINDOW_DAYS: u64 = 21;

/// At most this many sessions are listed per file, newest first.
const WRITERS_PER_FILE: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Writer {
    pub session_id: String,
    /// The session's title: a running Claude Code session's name, or a Codex
    /// thread's name.
    pub session_name: Option<String>,
    pub running: bool,
    /// The transcript's project directory, e.g. `-Users-x-Projects-deliberus`;
    /// for a Codex session `codex:<cwd name>`. `running` is always false for
    /// Codex, whose open threads nit cannot see.
    pub project: String,
    /// When this session last touched the path (UTC, RFC 3339).
    pub last_at: String,
    /// `edited (Edit)` for an editing tool call; `named in Bash` and the like
    /// when the path only appears in a tool's input.
    pub how: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueuedFile {
    pub path: String,
    /// The two porcelain status columns, e.g. " M", "MM", " D".
    pub status: String,
    /// stale | new | deleted | undetermined
    pub verdict: String,
    /// For a stale file: the commit whose content it matches.
    pub stale_commit: Option<String>,
    pub last_commit_at: Option<String>,
    pub modified_at: Option<String>,
    /// From when sessions were searched (the later of the last commit and the
    /// search window's start).
    pub searched_since: String,
    pub writers: Vec<Writer>,
    /// `git hash-object` of the work-tree file ("deleted" when absent). A
    /// dismissal holds only while this stays the same.
    pub blob: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Queue {
    pub generated_at: String,
    pub machine: String,
    /// The last nightly result, so a reader can tell "blocked" from "pulled around".
    pub last_sync_result: Option<String>,
    pub last_success_at: Option<String>,
    pub files: Vec<QueuedFile>,
    /// Edited files left out because they were dismissed with these exact bytes.
    pub dismissed: usize,
}

pub fn queue_path() -> PathBuf {
    nit_data_dir().join("drift-queue.json")
}

fn dismissed_path() -> PathBuf {
    nit_data_dir().join("drift-dismissed.json")
}

fn nit_data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".local/share/nit")
}

// ─── transcript search (pure over its inputs) ──────────────────────────────

/// One tool call touching a path: (session, transcript project, when, how).
type Touch = (String, String, String, String);

const EDIT_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// The ways a session writes a home-relative path in a command: absolute,
/// `~/…` and `$HOME/…`. The first version searched only the absolute form and
/// found no session for two of the three files that blocked the Mac on
/// 2026-10-15; the sessions had written `~/.claude/glossary/…`.
pub fn spellings(abs_path: &str, home: &Path) -> Vec<String> {
    let mut out = vec![abs_path.to_string()];
    if let Ok(rel) = Path::new(abs_path).strip_prefix(home) {
        let rel = rel.to_string_lossy();
        out.push(format!("~/{}", rel));
        out.push(format!("$HOME/{}", rel));
    }
    out
}

/// Every touch of `abs_path` on one transcript line, if the line is an
/// assistant message with tool calls. An editing tool must name the absolute
/// path; any other tool input counts when it holds any of `spellings`.
/// `project` is the transcript's directory.
pub fn touches_on_line(
    line: &str,
    abs_path: &str,
    spellings: &[String],
    project: &str,
) -> Vec<Touch> {
    if !line.contains("tool_use") || !spellings.iter().any(|s| line.contains(s.as_str())) {
        return Vec::new();
    }
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let session = v
        .get("sessionId")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let when = v
        .get("timestamp")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let blocks = match v.pointer("/message/content").and_then(|c| c.as_array()) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for b in blocks {
        if b.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
            continue;
        }
        let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        let input = match b.get("input") {
            Some(i) => i,
            None => continue,
        };
        let input_text = input.to_string();
        let named = spellings.iter().any(|s| input_text.contains(s.as_str()));
        let edits = EDIT_TOOLS.contains(&name)
            && (input.get("file_path").and_then(|p| p.as_str()) == Some(abs_path)
                || input.get("notebook_path").and_then(|p| p.as_str()) == Some(abs_path));
        let how = if edits {
            format!("edited ({})", name)
        } else if named {
            format!("named in {}", name)
        } else {
            continue;
        };
        out.push((session.clone(), project.to_string(), when.clone(), how));
    }
    out
}

/// Fold touches into one writer per session: the latest time, and "edited"
/// winning over "named". Only touches at or after `since` count. Newest first.
pub fn fold_writers(
    touches: &[Touch],
    since: &str,
    running: &HashMap<String, String>,
) -> Vec<Writer> {
    let mut by_session: BTreeMap<String, Writer> = BTreeMap::new();
    for (sid, project, when, how) in touches {
        if sid.is_empty() || when.as_str() < since {
            continue;
        }
        let entry = by_session.entry(sid.clone()).or_insert_with(|| Writer {
            session_id: sid.clone(),
            session_name: running.get(sid).cloned(),
            running: running.contains_key(sid),
            project: project.clone(),
            last_at: when.clone(),
            how: how.clone(),
        });
        let edited = how.starts_with("edited");
        let had_edit = entry.how.starts_with("edited");
        if when > &entry.last_at {
            entry.last_at = when.clone();
            if edited || !had_edit {
                entry.how = how.clone();
            }
        } else if edited && !had_edit {
            entry.how = how.clone();
        }
    }
    let mut writers: Vec<Writer> = by_session.into_values().collect();
    writers.sort_by(|a, b| b.last_at.cmp(&a.last_at));
    writers.truncate(WRITERS_PER_FILE);
    writers
}

/// Read every transcript modified at or after `oldest` once, collecting the
/// touches of each wanted path.
fn search_transcripts(
    projects_dir: &Path,
    home: &Path,
    wanted: &[String],
    oldest: SystemTime,
) -> HashMap<String, Vec<Touch>> {
    let forms: Vec<Vec<String>> = wanted.iter().map(|w| spellings(w, home)).collect();
    let mut found: HashMap<String, Vec<Touch>> = HashMap::new();
    if wanted.is_empty() || !projects_dir.exists() {
        return found;
    }
    for entry in walkdir::WalkDir::new(projects_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let fresh = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|m| m >= oldest);
        if !fresh {
            continue;
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let text = String::from_utf8_lossy(&bytes);
        let present: Vec<(&String, &Vec<String>)> = wanted
            .iter()
            .zip(forms.iter())
            .filter(|(_, f)| f.iter().any(|s| text.contains(s.as_str())))
            .collect();
        if present.is_empty() {
            continue;
        }
        // The project is the first directory below projects_dir, also for a
        // subagent transcript further down.
        let project = path
            .strip_prefix(projects_dir)
            .ok()
            .and_then(|r| r.components().next())
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_default();
        for line in text.lines() {
            for (w, f) in &present {
                let t = touches_on_line(line, w, f, &project);
                if !t.is_empty() {
                    found.entry((*w).clone()).or_default().extend(t);
                }
            }
        }
    }
    found
}

// ─── Codex transcripts ──────────────────────────────────────────────────────
//
// A Codex rollout (`~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`) is one
// session: line 1 is `session_meta` (id, cwd), `turn_context` lines carry the cwd
// as it changes, and shell runs as `response_item` → `custom_tool_call` named
// `exec`, whose `input` is a small JS script calling
// `tools.exec_command({cmd:"<shell>"})`. Codex has no edit tool in these
// transcripts: it changes files with Python or shell inside those commands, often
// building the path in pieces (`Path.home()/'.hammerspoon'/'init.lua'`). So a
// Codex touch is "named in Codex shell", and "edited? (Codex shell)" only when the
// command naming the path also visibly writes. Read one with `codexdistill <id>`.

/// Command fragments that write a file. Evidence for a human, never a verdict.
const CODEX_WRITE_MARKERS: &[&str] = &[
    "write_text(",
    ".write(",
    "sed -i",
    "perl -pi",
    "apply_patch",
    "tee ",
    "cp ",
    "mv ",
    "install ",
    "shutil.copy",
    "rsync ",
];

/// The shell commands inside a Codex `exec` script: every JSON string literal
/// that follows `cmd:` or `"cmd":`.
pub fn codex_commands(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = script;
    while let Some(i) = rest.find("cmd") {
        let after = &rest[i + 3..];
        let after = after.strip_prefix('"').unwrap_or(after);
        let after = after.trim_start();
        if let Some(lit) = after.strip_prefix(':').map(str::trim_start)
            && lit.starts_with('"')
            && let Some(Ok(s)) = serde_json::Deserializer::from_str(lit)
                .into_iter::<String>()
                .next()
        {
            out.push(s);
        }
        rest = &rest[i + 3..];
    }
    out
}

/// A path as a Codex command may write it: the home spellings, plus relative to
/// the session's working directory when the path lies inside it.
pub fn codex_spellings(abs_path: &str, home: &Path, cwd: &str) -> Vec<String> {
    let mut out = spellings(abs_path, home);
    if !cwd.is_empty()
        && let Ok(rel) = Path::new(abs_path).strip_prefix(cwd)
    {
        let rel = rel.to_string_lossy().to_string();
        if !rel.is_empty() {
            out.push(rel);
        }
    }
    out
}

/// Every touch of `abs_path` in one Codex rollout's text. The project is
/// recorded as `codex:<cwd name>` so a reader can tell the harness apart.
pub fn codex_touches(text: &str, abs_path: &str, home: &Path) -> Vec<Touch> {
    let mut out = Vec::new();
    let mut session = String::new();
    let mut cwd = String::new();
    for line in text.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let p = match v.get("payload") {
            Some(p) => p,
            None => continue,
        };
        if kind == "session_meta" {
            session = p
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
        }
        if (kind == "session_meta" || kind == "turn_context")
            && let Some(c) = p.get("cwd").and_then(|x| x.as_str())
        {
            cwd = c.to_string();
        }
        if kind != "response_item"
            || p.get("type").and_then(|t| t.as_str()) != Some("custom_tool_call")
        {
            continue;
        }
        let script = p.get("input").and_then(|x| x.as_str()).unwrap_or("");
        let forms = codex_spellings(abs_path, home, &cwd);
        let mut how: Option<&str> = None;
        for cmd in codex_commands(script) {
            if !forms.iter().any(|f| cmd.contains(f.as_str())) {
                continue;
            }
            if CODEX_WRITE_MARKERS.iter().any(|m| cmd.contains(m)) {
                how = Some("edited? (Codex shell)");
                break;
            }
            how = Some("named in Codex shell");
        }
        if let Some(how) = how {
            let when = v.get("timestamp").and_then(|x| x.as_str()).unwrap_or("");
            let project = format!(
                "codex:{}",
                Path::new(&cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            );
            out.push((session.clone(), project, when.to_string(), how.to_string()));
        }
    }
    out
}

/// Read every Codex rollout modified at or after `oldest` once.
fn search_codex(
    codex_dirs: &[PathBuf],
    home: &Path,
    wanted: &[String],
    oldest: SystemTime,
) -> HashMap<String, Vec<Touch>> {
    let mut found: HashMap<String, Vec<Touch>> = HashMap::new();
    if wanted.is_empty() {
        return found;
    }
    for dir in codex_dirs.iter().filter(|d| d.exists()) {
        for entry in walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            let fresh = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .is_some_and(|m| m >= oldest);
            if !fresh {
                continue;
            }
            let text = match std::fs::read(path) {
                Ok(b) => String::from_utf8_lossy(&b).to_string(),
                Err(_) => continue,
            };
            for w in wanted {
                // Cheap pre-filter on the file name, which every spelling ends with.
                let base = Path::new(w)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string());
                if base.is_some_and(|b| !text.contains(b.as_str())) {
                    continue;
                }
                let t = codex_touches(&text, w, home);
                if !t.is_empty() {
                    found.entry(w.clone()).or_default().extend(t);
                }
            }
        }
    }
    found
}

/// Codex thread names (`~/.codex/session_index.jsonl`, the last entry wins).
fn codex_titles(codex_home: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(codex_home.join("session_index.jsonl")) {
        for line in s.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
                && let (Some(id), Some(name)) = (
                    v.get("id").and_then(|x| x.as_str()),
                    v.get("thread_name").and_then(|x| x.as_str()),
                )
            {
                out.insert(id.to_string(), name.to_string());
            }
        }
    }
    out
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

fn pid_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        pid > 0 && unsafe { kill(pid, 0) } == 0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Running Claude Code sessions: session id → title (empty when untitled).
/// A record whose process is gone is a session that ended without cleaning up.
pub fn running_sessions(sessions_dir: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let entries = match std::fs::read_dir(sessions_dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let v: serde_json::Value = match std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => continue,
        };
        let pid = v
            .get("pid")
            .and_then(|x| x.as_str().and_then(|s| s.parse().ok()).or(x.as_i64()))
            .unwrap_or(0) as i32;
        let sid = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("");
        if sid.is_empty() || !pid_alive(pid) {
            continue;
        }
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        out.insert(sid.to_string(), name);
    }
    out
}

// ─── dismissals ─────────────────────────────────────────────────────────────

pub fn load_dismissed() -> BTreeMap<String, String> {
    std::fs::read_to_string(dismissed_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_dismissed(map: &BTreeMap<String, String>) {
    let p = dismissed_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(map).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

// ─── building the queue ─────────────────────────────────────────────────────

fn rfc3339_of(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn worktree_blob(strategy: &GitStrategy, home: &Path, path: &str) -> String {
    let abs = home.join(path);
    if !abs.exists() {
        return "deleted".to_string();
    }
    git::git_output_with(strategy, &["hash-object", abs.to_string_lossy().as_ref()])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn last_commit_epoch(strategy: &GitStrategy, path: &str) -> Option<u64> {
    git::git_output_with(
        strategy,
        &[
            "log",
            "-1",
            "--format=%ct",
            "--",
            &drift_triage::root_pathspec(path),
        ],
    )
    .ok()?
    .trim()
    .parse()
    .ok()
}

/// Gather the queue from the current work tree. Writes nothing.
pub fn build(config: &NitConfig) -> Queue {
    let strategy = config.git_strategy();
    let home = dirs::home_dir().expect("cannot determine home directory");
    let porcelain = git::git_output_with(strategy, &["status", "--porcelain"]).unwrap_or_default();
    let all = crate::sync_status::detect_pre_pull_drift(&porcelain);
    let triaged = drift_triage::triage(strategy, &all);
    let dismissed = load_dismissed();

    let now = SystemTime::now();
    let window_start = now - Duration::from_secs(SEARCH_WINDOW_DAYS * 86_400);
    let mut pending: Vec<(drift_triage::TriagedFile, String, SystemTime)> = Vec::new();
    let mut n_dismissed = 0usize;
    for t in triaged {
        let blob = worktree_blob(strategy, &home, &t.path);
        if dismissed.get(&t.path) == Some(&blob) {
            n_dismissed += 1;
            continue;
        }
        let since = last_commit_epoch(strategy, &t.path)
            .map(|s| UNIX_EPOCH + Duration::from_secs(s))
            .filter(|s| *s > window_start)
            .unwrap_or(window_start);
        pending.push((t, blob, since));
    }

    let oldest = pending.iter().map(|p| p.2).min().unwrap_or(now);
    let wanted: Vec<String> = pending
        .iter()
        .map(|(t, _, _)| home.join(&t.path).to_string_lossy().to_string())
        .collect();
    let mut touches = search_transcripts(&home.join(".claude/projects"), &home, &wanted, oldest);
    let codex_home = home.join(".codex");
    let codex_dirs = [
        codex_home.join("sessions"),
        codex_home.join("archived_sessions"),
    ];
    for (path, t) in search_codex(&codex_dirs, &home, &wanted, oldest) {
        touches.entry(path).or_default().extend(t);
    }
    let codex_names = codex_titles(&codex_home);
    let running = running_sessions(&home.join(".claude/sessions"));

    let files = pending
        .into_iter()
        .map(|(t, blob, since)| {
            let abs = home.join(&t.path).to_string_lossy().to_string();
            let since_s = rfc3339_of(since);
            let mut writers = fold_writers(
                touches.get(&abs).map(Vec::as_slice).unwrap_or(&[]),
                &since_s,
                &running,
            );
            for w in writers
                .iter_mut()
                .filter(|w| w.project.starts_with("codex:"))
            {
                w.session_name = codex_names.get(&w.session_id).cloned();
            }
            let modified_at = std::fs::metadata(home.join(&t.path))
                .and_then(|m| m.modified())
                .ok()
                .map(rfc3339_of);
            let last_commit_at = last_commit_epoch(strategy, &t.path)
                .map(|s| rfc3339_of(UNIX_EPOCH + Duration::from_secs(s)));
            let (verdict, stale_commit) = match &t.kind {
                DriftKind::Stale { commit, .. } => ("stale", Some(commit.clone())),
                DriftKind::Unique => ("new", None),
                DriftKind::Deleted => ("deleted", None),
                DriftKind::Unknown => ("undetermined", None),
            };
            QueuedFile {
                path: t.path,
                status: t.status,
                verdict: verdict.to_string(),
                stale_commit,
                last_commit_at,
                modified_at,
                searched_since: since_s,
                writers,
                blob,
            }
        })
        .collect();

    let last = crate::sync_status::load_status();
    Queue {
        generated_at: rfc3339_of(now),
        machine: config.machine_name.clone(),
        last_sync_result: last
            .as_ref()
            .and_then(|s| serde_json::to_value(&s.result).ok())
            .and_then(|v| v.as_str().map(str::to_string)),
        last_success_at: last.and_then(|s| s.last_success_at),
        files,
        dismissed: n_dismissed,
    }
}

pub fn write(queue: &Queue) {
    let p = queue_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(
        &tmp,
        serde_json::to_string_pretty(queue).unwrap_or_default(),
    )
    .is_ok()
    {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Build and write the queue, quietly. For callers whose own result must not
/// depend on it (the end of `nit update`).
pub fn refresh(config: &NitConfig) {
    write(&build(config));
}

/// `nit drift-triage`: build, write, and print one line per file.
pub fn cmd(
    config: &NitConfig,
    dismiss: &[String],
    quiet: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !dismiss.is_empty() {
        let strategy = config.git_strategy();
        let home = dirs::home_dir().expect("cannot determine home directory");
        let mut map = load_dismissed();
        for p in dismiss {
            let rel = Path::new(p)
                .strip_prefix(&home)
                .map(|r| r.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.clone());
            let blob = worktree_blob(strategy, &home, &rel);
            eprintln!(
                "nit: dismissed {} as it is now (it comes back if it changes)",
                rel
            );
            map.insert(rel, blob);
        }
        save_dismissed(&map);
    }
    let q = build(config);
    write(&q);
    if quiet {
        return Ok(());
    }
    if q.files.is_empty() {
        eprintln!(
            "nit drift-triage: nothing waiting ({} dismissed)",
            q.dismissed
        );
        return Ok(());
    }
    eprintln!(
        "nit drift-triage: {} edited file(s) waiting:",
        q.files.len()
    );
    for f in &q.files {
        eprintln!("  {} {}  [{}]", f.status, f.path, f.verdict);
        if f.writers.is_empty() {
            eprintln!(
                "      no session touched it since {} (by hand, by a program, or older)",
                &f.searched_since[..10.min(f.searched_since.len())]
            );
        }
        for w in &f.writers {
            // Whether a Codex thread is still open is not recorded anywhere nit reads.
            let state = if w.project.starts_with("codex:") {
                "codex"
            } else if w.running {
                "running"
            } else {
                "ended"
            };
            let name = w.session_name.clone().unwrap_or_default();
            eprintln!(
                "      {} {} {} {} {}",
                &w.session_id[..8.min(w.session_id.len())],
                state,
                w.how,
                w.last_at,
                name
            );
        }
    }
    eprintln!("queue: {}", queue_path().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Copied from a real transcript line shape (2026-10-15), trimmed.
    const EDIT_LINE: &str = r#"{"type":"assistant","sessionId":"1a58d50c-3310-4e15-a14c-92d711b86190","timestamp":"2026-10-10T13:00:38.634Z","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/Users/u/.hammerspoon/init.lua","old_string":"a","new_string":"b"}}]}}"#;
    const BASH_LINE: &str = r#"{"type":"assistant","sessionId":"69180500-a653-4765-8cc0-4a30c51a8f65","timestamp":"2026-10-15T11:26:53.646Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls -l /Users/u/.hammerspoon/init.lua","description":"x"}}]}}"#;
    const RESULT_LINE: &str = r#"{"type":"user","sessionId":"s","timestamp":"2026-10-15T11:27:00Z","message":{"content":[{"type":"tool_result","content":"/Users/u/.hammerspoon/init.lua"}]}}"#;

    // Real Codex rollout lines (session 01a121e2, 2026-10-09/11), trimmed to the
    // fields read; home rewritten to /Users/u.
    const CODEX_META: &str = r#"{"timestamp":"2026-10-09T18:17:58.064Z","type":"session_meta","payload":{"id":"01a121e2-d5ef-7ac3-a381-2d640fad1546","cwd":"/Users/u/Projects/spela"}}"#;
    const CODEX_TAIL: &str = r#"{"timestamp":"2026-10-11T19:30:00.000Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"text(await tools.exec_command({cmd:\"sed -n '60,88p' src/state.rs; tail -25 ~/.hammerspoon/init.lua\",\"max_output_tokens\":1800}));\n"}}"#;
    const CODEX_WRITE: &str = r#"{"timestamp":"2026-10-11T19:31:00.000Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"text(await tools.exec_command({cmd:\"python3 - <<'PY'\\nfrom pathlib import Path\\np=Path('static/remote.html');s=p.read_text()\\np.write_text(s)\\nPY\"}));\n"}}"#;

    #[test]
    fn codex_commands_parse_escaped_strings() {
        let script =
            r#"text(await tools.exec_command({cmd:"rg -n \"a b\" x.rs","max_output_tokens":10}));"#;
        assert_eq!(
            codex_commands(script),
            vec![r#"rg -n "a b" x.rs"#.to_string()]
        );
        let quoted = r#"tools.exec_command({"cmd":"ls ~/x"})"#;
        assert_eq!(codex_commands(quoted), vec!["ls ~/x".to_string()]);
    }

    #[test]
    fn codex_tilde_path_is_named_with_session_and_project() {
        let text = format!("{}\n{}\n", CODEX_META, CODEX_TAIL);
        let t = codex_touches(
            &text,
            "/Users/u/.hammerspoon/init.lua",
            Path::new("/Users/u"),
        );
        assert_eq!(t.len(), 1, "{:?}", t);
        assert_eq!(t[0].0, "01a121e2-d5ef-7ac3-a381-2d640fad1546");
        assert_eq!(t[0].1, "codex:spela");
        assert_eq!(t[0].3, "named in Codex shell");
    }

    #[test]
    fn codex_relative_path_with_a_write_is_a_likely_edit() {
        let text = format!("{}\n{}\n", CODEX_META, CODEX_WRITE);
        let t = codex_touches(
            &text,
            "/Users/u/Projects/spela/static/remote.html",
            Path::new("/Users/u"),
        );
        assert_eq!(t.len(), 1, "{:?}", t);
        assert_eq!(t[0].3, "edited? (Codex shell)");
        // The same relative name under another cwd is not this file.
        let other = text.replace("/Users/u/Projects/spela", "/Users/u/Projects/other");
        assert!(
            codex_touches(
                &other,
                "/Users/u/Projects/spela/static/remote.html",
                Path::new("/Users/u")
            )
            .is_empty()
        );
    }

    fn forms() -> Vec<String> {
        spellings("/Users/u/.hammerspoon/init.lua", Path::new("/Users/u"))
    }

    #[test]
    fn a_tilde_path_in_a_command_is_found() {
        let line = BASH_LINE.replace("/Users/u/.hammerspoon/init.lua", "~/.hammerspoon/init.lua");
        let t = touches_on_line(&line, "/Users/u/.hammerspoon/init.lua", &forms(), "proj");
        assert_eq!(t.len(), 1, "{:?}", t);
        assert_eq!(t[0].3, "named in Bash");
    }

    #[test]
    fn spellings_cover_tilde_and_home_variable() {
        assert_eq!(
            forms(),
            vec![
                "/Users/u/.hammerspoon/init.lua".to_string(),
                "~/.hammerspoon/init.lua".to_string(),
                "$HOME/.hammerspoon/init.lua".to_string(),
            ]
        );
    }

    #[test]
    fn an_editing_tool_call_is_an_edit() {
        let t = touches_on_line(
            EDIT_LINE,
            "/Users/u/.hammerspoon/init.lua",
            &forms(),
            "proj",
        );
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].0, "1a58d50c-3310-4e15-a14c-92d711b86190");
        assert_eq!(t[0].3, "edited (Edit)");
    }

    #[test]
    fn a_path_named_in_a_command_is_only_named() {
        let t = touches_on_line(
            BASH_LINE,
            "/Users/u/.hammerspoon/init.lua",
            &forms(),
            "proj",
        );
        assert_eq!(t[0].3, "named in Bash");
    }

    #[test]
    fn a_tool_result_is_not_a_touch() {
        assert!(
            touches_on_line(RESULT_LINE, "/Users/u/.hammerspoon/init.lua", &forms(), "p")
                .is_empty()
        );
    }

    #[test]
    fn a_longer_path_sharing_the_prefix_is_not_an_edit_of_this_one() {
        let line = EDIT_LINE.replace("init.lua\"", "init.lua.bak\"");
        let t = touches_on_line(&line, "/Users/u/.hammerspoon/init.lua", &forms(), "proj");
        assert_eq!(t[0].3, "named in Edit", "{:?}", t);
    }

    fn touch(sid: &str, when: &str, how: &str) -> Touch {
        (sid.into(), "proj".into(), when.into(), how.into())
    }

    #[test]
    fn touches_before_the_last_commit_do_not_count() {
        let t = vec![touch("a", "2026-10-10T13:00:00Z", "edited (Edit)")];
        assert!(fold_writers(&t, "2026-10-10T15:05:26Z", &HashMap::new()).is_empty());
    }

    #[test]
    fn one_writer_per_session_newest_first_and_an_edit_outranks_a_mention() {
        let t = vec![
            touch("a", "2026-10-15T10:00:00Z", "edited (Edit)"),
            touch("a", "2026-10-15T11:00:00Z", "named in Bash"),
            touch("b", "2026-10-15T12:00:00Z", "named in Bash"),
        ];
        let mut running = HashMap::new();
        running.insert("b".to_string(), "Deliberus: claims".to_string());
        let w = fold_writers(&t, "2026-10-15T00:00:00Z", &running);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].session_id, "b");
        assert!(w[0].running);
        assert_eq!(w[0].session_name.as_deref(), Some("Deliberus: claims"));
        assert_eq!(w[1].session_id, "a");
        assert_eq!(w[1].last_at, "2026-10-15T11:00:00Z");
        assert_eq!(
            w[1].how, "edited (Edit)",
            "an edit is kept over a later mention"
        );
        assert!(!w[1].running);
    }

    #[test]
    fn running_sessions_skips_records_whose_process_is_gone() {
        let dir = tempfile::TempDir::new().unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.path().join(format!("{}.json", me)),
            format!(r#"{{"pid":"{}","sessionId":"live","name":"this one"}}"#, me),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("999999.json"),
            r#"{"pid":"999999","sessionId":"gone","name":"old"}"#,
        )
        .unwrap();
        let r = running_sessions(dir.path());
        assert_eq!(r.get("live").map(String::as_str), Some("this one"));
        assert!(!r.contains_key("gone"));
    }

    #[test]
    fn transcript_search_attributes_subagent_transcripts_to_their_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let sub = dir.path().join("-Users-u-dotfiles/abc/subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("agent-1.jsonl"), format!("{}\n", EDIT_LINE)).unwrap();
        let wanted = vec!["/Users/u/.hammerspoon/init.lua".to_string()];
        let found = search_transcripts(dir.path(), Path::new("/Users/u"), &wanted, UNIX_EPOCH);
        let t = &found[&wanted[0]];
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].1, "-Users-u-dotfiles");
    }
}
