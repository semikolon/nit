//! Drift triage — is a drifted file NEW WORK, or a STALE worktree?
//!
//! `sync_status::detect_pre_pull_drift` answers *whether* tracked files differ
//! from HEAD. It deliberately does not ask *why*, and for years that was fine:
//! drift meant someone had edited something and had not committed it yet.
//!
//! MERIAN disproved that on 2026-09-03. It showed 18 modified tracked files
//! plus one deletion, which reads as five weeks of unreconciled work on a
//! travel laptop. Measured, every one of those files carried the *identical*
//! mtime — `2026-06-28 03:00:02`, the nightly sync hour, to the second — and
//! every one's exact content already existed in the object store. Not one
//! unique byte. The worktree was not ahead of HEAD; it was five weeks BEHIND
//! it, and the reflog's last `pull: Fast-forward` was 2026-06-27.
//!
//! That produces a deadlock, and it is self-sustaining:
//!
//! > The pre-pull abort refuses to pull while tracked files are dirty. Those
//! > files can only become clean by pulling. A machine that enters this state
//! > can never leave it on its own, and every subsequent night re-proves the
//! > same abort.
//!
//! MERIAN sat there for 67 consecutive nights and fell 536 commits behind.
//! The safety invariant was correct every single time — it was protecting a
//! worktree whose contents were already safe in git. It was simply never
//! heard, because an abort writes a status file and stops.
//!
//! So this module answers two questions the abort could not:
//!
//! 1. **Is this file stale or is it real work?** Deterministic, no judgement:
//!    does the worktree content match this path at some ancestor of HEAD?
//!    Content already committed is by definition not new work, whatever it
//!    looks like. This runs BEFORE any meaningful-vs-cosmetic classification
//!    and short-circuits it — a stale file has nothing to classify.
//!
//! 2. **Is this a deadlock rather than ordinary drift?** Drift plus a last
//!    successful sync older than a few days is a different condition with the
//!    opposite remedy: *discard and pull*, never *commit*. It deserves its own
//!    name, its own message, and an escalation path off the machine.
//!
//! Generic lesson worth carrying beyond nit: **a guard that fires correctly,
//! repeatedly and silently is indistinguishable from a guard that never
//! fires.** A recurring successful refusal needs an escalation path, or the
//! protection it provides becomes the outage it was meant to prevent.
//!
//! Design + the full MERIAN post-mortem:
//! `docs/forward_only_drift_steward_design_2026-07-11.md` § 10.

use crate::config::GitStrategy;
use crate::git;

/// A drifted file is called a deadlock rather than ordinary drift once the
/// last successful sync is this old. One or two nights is a person with
/// uncommitted work; three is a machine that cannot recover by itself.
pub const DEADLOCK_AFTER_DAYS: i64 = 3;

/// While a deadlock persists, re-notify at most this often. Notifying on every
/// nightly run would train the alert to be ignored, which is the failure this
/// whole module exists to prevent; going silent again would repeat it exactly.
pub const RENOTIFY_AFTER_DAYS: i64 = 7;

/// How far back through a single path's history to look for a matching blob.
/// Path-scoped history is short for dotfiles (tens of commits), and a stale
/// worktree normally matches within the first few, so this is a runaway guard
/// rather than a real limit.
const HISTORY_SEARCH_LIMIT: usize = 200;

/// Seconds before the notification attempt is abandoned. The notification is a
/// courtesy; the nightly sync must never hang waiting for it, least of all on
/// a travel machine whose VPN may be down.
const NOTIFY_TIMEOUT_SECS: &str = "5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftKind {
    /// The worktree content matches this path as of an ancestor commit. The
    /// worktree is behind, not ahead. Discarding loses nothing.
    Stale { commit: String, date: String },
    /// The content matches nothing in this path's history — genuinely new,
    /// uncommitted work. Never discard without a human deciding.
    Unique,
    /// A tracked file deleted from the worktree. Deliberate deletions and
    /// accidents look identical here, so this is always reported, never acted
    /// on.
    Deleted,
    /// A git call failed, or the search hit `HISTORY_SEARCH_LIMIT` without a
    /// match. Deliberately distinct from `Unique`: both mean "do not discard",
    /// but only one of them means "we actually looked all the way back".
    Unknown,
}

impl DriftKind {
    /// Is it proven safe to discard this file's local state? Only `Stale`
    /// qualifies, and only because the exact bytes are recoverable from the
    /// named commit. Everything else — including `Unknown` — is a no.
    pub fn is_safe_to_discard(&self) -> bool {
        matches!(self, DriftKind::Stale { .. })
    }

    pub fn label(&self) -> String {
        match self {
            DriftKind::Stale { commit, date } => {
                format!("stale — matches this file as of {} ({})", commit, date)
            }
            DriftKind::Unique => "NEW CONTENT — never committed".to_string(),
            DriftKind::Deleted => "deleted locally".to_string(),
            DriftKind::Unknown => "undetermined — treat as new".to_string(),
        }
    }
}

/// One drifted path plus what we found out about it.
#[derive(Debug, Clone)]
pub struct TriagedFile {
    /// The two-character porcelain status, e.g. " M" or " D".
    pub status: String,
    pub path: String,
    pub kind: DriftKind,
}

/// Split a `git status --porcelain` line into its status and path.
///
/// Renames arrive as `R  old -> new`; the path that matters for triage is the
/// destination, since that is what sits in the worktree now.
pub fn parse_porcelain_entry(line: &str) -> Option<(String, String)> {
    if line.len() < 4 {
        return None;
    }
    let status = line.get(0..2)?.to_string();
    let rest = line.get(3..)?.trim();
    if rest.is_empty() {
        return None;
    }
    let path = match rest.split_once(" -> ") {
        Some((_, dest)) => dest,
        None => rest,
    };
    Some((status, path.trim_matches('"').to_string()))
}

/// Given the worktree blob and this path's history (newest first, as
/// `(commit, date, blob)`), decide whether the worktree is merely behind.
///
/// Returns the FIRST match walking back from HEAD, which is also the most
/// recent — that is the commit whose content the worktree is sitting on, and
/// therefore the honest answer to "how far behind is this file".
pub fn match_blob_in_history(
    worktree_blob: &str,
    history: &[(String, String, String)],
) -> DriftKind {
    if worktree_blob.is_empty() {
        return DriftKind::Unknown;
    }
    for (commit, date, blob) in history {
        if blob == worktree_blob {
            return DriftKind::Stale {
                commit: commit.clone(),
                date: date.clone(),
            };
        }
    }
    if history.len() >= HISTORY_SEARCH_LIMIT {
        // We stopped looking before the history ran out, so "no match" is not
        // the same as "nothing matches".
        return DriftKind::Unknown;
    }
    DriftKind::Unique
}

/// The cwd-independent pathspec for a work-tree-relative path.
///
/// Extracted so it is testable and so the `:/` cannot be "simplified" away: a
/// bare pathspec is resolved against the process directory, and `nit update`
/// run from anywhere but $HOME then finds no history for any file. Same form
/// `cmd_commit` uses, for the same reason.
pub fn root_pathspec(path: &str) -> String {
    format!(":/{}", path)
}

/// Whole-number days from an RFC3339 timestamp until `now`. `None` when the
/// timestamp is absent or unparseable — the caller must not read that as zero.
pub fn days_since_rfc3339(ts: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(ts?).ok()?;
    Some((now - parsed.with_timezone(&chrono::Utc)).num_days())
}

/// Is this abort a deadlock rather than ordinary drift?
///
/// A machine that has never synced successfully has no `last_success_at`. That
/// is a fresh bootstrap, not a deadlock, so it answers `false` — a new machine
/// should not greet its owner with an incident.
pub fn is_deadlock(last_success_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    days_since_rfc3339(last_success_at, now).is_some_and(|d| d >= DEADLOCK_AFTER_DAYS)
}

/// Should a notification go out now? True on first entry into the deadlock
/// (no prior notification) and thereafter no more often than
/// `RENOTIFY_AFTER_DAYS`.
pub fn should_notify(last_notified_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    match days_since_rfc3339(last_notified_at, now) {
        None => true,
        Some(d) => d >= RENOTIFY_AFTER_DAYS,
    }
}

/// Triage every drifted porcelain line. Pure classification; nothing is
/// written, moved or discarded here.
pub fn triage(strategy: &GitStrategy, drift_lines: &[String]) -> Vec<TriagedFile> {
    drift_lines
        .iter()
        .filter_map(|line| parse_porcelain_entry(line))
        .map(|(status, path)| {
            let kind = if status.contains('D') {
                DriftKind::Deleted
            } else {
                classify_path(strategy, &path)
            };
            TriagedFile { status, path, kind }
        })
        .collect()
}

/// Classify one path by asking git whether its current bytes were ever
/// committed for this path.
///
/// Exactly three git calls regardless of how deep the history runs: hash the
/// worktree file, list the commits touching the path, then resolve every one
/// of those commits' blobs in a SINGLE multi-rev `rev-parse`. An earlier
/// version looped one `rev-parse` per commit with an early exit; that was
/// cheaper in the lucky case and unbounded in the unlucky one, and worse, it
/// re-implemented the comparison that `match_blob_in_history` already owns and
/// tests. The comparison lives in one place now, and it is the tested one.
///
/// `--diff-filter=d` drops commits that DELETED the path, since `<commit>:<path>`
/// does not resolve there and one bad rev fails the whole batch.
fn classify_path(strategy: &GitStrategy, path: &str) -> DriftKind {
    // BOTH git calls below are cwd-sensitive, and `nit update` runs from
    // wherever the user or launchd happens to be. `hash-object` resolves its
    // argument against the PROCESS directory, ignoring `--work-tree`, and a
    // bare pathspec is likewise interpreted relative to the cwd. Run from
    // anywhere but $HOME, both silently return nothing and every file gets
    // classified `Unknown` — which looks exactly like a cautious answer rather
    // than a broken one. Caught by an end-to-end check on a file known to be
    // stale; all eighteen unit tests passed throughout.
    //
    // So: an absolute path for `hash-object`, and the `:/` root-relative
    // pathspec for `log`, matching the CWD-independent form `cmd_commit`
    // already uses.
    let abs = git::work_tree().join(path);
    let abs = abs.to_string_lossy().to_string();
    let worktree_blob = match git::git_output_with(strategy, &["hash-object", &abs]) {
        Ok(s) => s.trim().to_string(),
        Err(_) => return DriftKind::Unknown,
    };
    if worktree_blob.is_empty() {
        return DriftKind::Unknown;
    }

    // No `--diff-filter=d` here, however tempting. It looks like the clean way
    // to skip commits that DELETED the path (where `<commit>:<path>` will not
    // resolve), and measured against this very repo it returns ZERO commits for
    // a path with fourteen: the filter needs diff generation that `git log`
    // does not perform under default history simplification. A resolution
    // failure per commit is handled below instead, which costs nothing.
    let limit = format!("--max-count={}", HISTORY_SEARCH_LIMIT);
    let pathspec = root_pathspec(path);
    let log = match git::git_output_with(
        strategy,
        &[
            "log",
            &limit,
            "--format=%H %ad",
            "--date=short",
            "HEAD",
            "--",
            &pathspec,
        ],
    ) {
        Ok(s) => s,
        Err(_) => return DriftKind::Unknown,
    };

    let commits: Vec<(String, String)> = log
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(c, d)| (c.trim().to_string(), d.trim().to_string()))
        .collect();
    if commits.is_empty() {
        return DriftKind::Unique;
    }
    let searched = commits.len();

    // One `rev-parse` per commit. A commit where the path does not resolve
    // (it deleted the file, or the path was renamed into place later) simply
    // contributes nothing rather than failing the whole classification.
    let history: Vec<(String, String, String)> = commits
        .into_iter()
        .filter_map(|(commit, date)| {
            let spec = format!("{}:{}", commit, path);
            let blob = git::git_output_with(strategy, &["rev-parse", &spec]).ok()?;
            let short = commit.get(0..8).unwrap_or(&commit).to_string();
            Some((short, date, blob.trim().to_string()))
        })
        .collect();

    match match_blob_in_history(&worktree_blob, &history) {
        // `match_blob_in_history` judges truncation by the length it was
        // given; unresolvable commits were dropped above, so the honest
        // measure of "did we look all the way back" is the pre-filter count.
        DriftKind::Unique if searched >= HISTORY_SEARCH_LIMIT => DriftKind::Unknown,
        other => other,
    }
}

/// Find the ntfy bearer token. Environment first, then the deployed secret
/// tiers, which is the more reliable of the two: nit itself deploys those files
/// to every fleet machine, whereas a launchd job inherits almost no environment.
///
/// The value is returned for immediate use and never logged, printed or placed
/// on a command line.
/// Pull `NAME=value` out of one line of a shell env file.
///
/// The `export ` prefix is the whole reason this is a named, tested function
/// rather than a `strip_prefix` inline. The fleet's tier files write
/// `export NTFY_TOKEN=…`, a bare `strip_prefix("NTFY_TOKEN=")` matched
/// nothing, and the only symptom was an alert that never arrived — which is
/// indistinguishable from having nothing to alert about.
pub fn parse_env_line(line: &str, name: &str) -> Option<String> {
    let line = line.trim();
    let line = line.strip_prefix("export ").unwrap_or(line);
    let value = line.strip_prefix(name)?.strip_prefix('=')?;
    let value = value.trim().trim_matches('"').trim_matches('\'');
    (!value.is_empty()).then(|| value.to_string())
}

fn ntfy_token() -> Option<String> {
    if let Some(t) = std::env::var("NTFY_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        return Some(t);
    }
    let secrets = dirs::home_dir()?.join(".secrets");
    let entries = std::fs::read_dir(secrets).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "env") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(v) = content
            .lines()
            .find_map(|line| parse_env_line(line, "NTFY_TOKEN"))
        {
            return Some(v);
        }
    }
    None
}

/// Push a message to ntfy, best effort.
///
/// LIFETIME: scope-bound to this call. `curl --max-time` bounds it at
/// `NOTIFY_TIMEOUT_SECS`; nothing is spawned, backgrounded or left running,
/// and every failure is swallowed. A machine that cannot reach ntfy — a travel
/// laptop off the VPN, which is exactly the machine most likely to be in this
/// state — must still complete its sync and still write its status file.
///
/// Shelling out to curl rather than adding an HTTP client keeps nit's
/// dependency set unchanged; nit already shells out to git for everything.
///
/// The server requires auth (a bare post returns 403, measured), so a missing
/// token would make every alert a silent no-op — the precise failure this
/// module exists to end. Credentials go to curl through a config file on
/// STDIN rather than argv, so the token never appears in the process list.
pub fn notify_ntfy(url: &str, title: &str, body: &str) -> bool {
    use std::io::Write;

    if url.trim().is_empty() {
        return false;
    }
    let Some(token) = ntfy_token() else {
        return false;
    };

    let mut child = match std::process::Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            NOTIFY_TIMEOUT_SECS,
            "-K",
            "-",
            "-H",
            &format!("Title: {}", title),
            "-H",
            "Priority: high",
            "-H",
            "Tags: warning",
            "-d",
            body,
            url,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "header = \"Authorization: Bearer {}\"", token);
    }

    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// The human-readable body of the deadlock alert.
pub fn deadlock_message(machine: &str, days: i64, files: &[TriagedFile]) -> String {
    let stale = files.iter().filter(|f| f.kind.is_safe_to_discard()).count();
    let total = files.len();
    let mut msg = format!(
        "{} has not synced for {} days. {} tracked file(s) block the pull.",
        machine, days, total
    );
    if stale == total && total > 0 {
        msg.push_str(
            " All of them are stale copies already in git — nothing would be lost by discarding.",
        );
    } else if stale > 0 {
        msg.push_str(&format!(
            " {} are stale; {} carry content never committed and need a look.",
            stale,
            total - stale
        ));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    // Porcelain lines copied verbatim from MERIAN's own `git status
    // --porcelain` on 2026-09-03, not invented. The deletion is real too.
    const MERIAN_PORCELAIN: &[&str] = &[
        " M .Brewfile",
        " M .claude/CLAUDE.md",
        " M .local/share/presence-service/presence_service.py",
        " M dotfiles/system/darwin/opt/AdGuardHome/AdGuardHome.yaml",
        " D .config/crush/crush.json",
    ];

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-09-03T12:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn parses_real_merian_porcelain_lines() {
        let parsed: Vec<_> = MERIAN_PORCELAIN
            .iter()
            .filter_map(|l| parse_porcelain_entry(l))
            .collect();
        assert_eq!(parsed.len(), 5);
        assert_eq!(parsed[0], (" M".to_string(), ".Brewfile".to_string()));
        assert_eq!(
            parsed[3].1,
            "dotfiles/system/darwin/opt/AdGuardHome/AdGuardHome.yaml"
        );
        assert_eq!(parsed[4].0, " D");
    }

    #[test]
    fn parses_rename_to_its_destination() {
        // The destination is what sits in the worktree, so that is what gets
        // classified.
        let (status, path) = parse_porcelain_entry("R  .oldrc -> .newrc").unwrap();
        assert_eq!(status, "R ");
        assert_eq!(path, ".newrc");
    }

    #[test]
    fn ignores_lines_too_short_to_carry_a_path() {
        assert!(parse_porcelain_entry("").is_none());
        assert!(parse_porcelain_entry(" M ").is_none());
        assert!(parse_porcelain_entry(" M").is_none());
    }

    #[test]
    fn blob_found_in_history_is_stale_and_names_the_newest_match() {
        let history = vec![
            ("aaaa1111".into(), "2026-07-29".into(), "blobNEW".into()),
            ("bbbb2222".into(), "2026-06-27".into(), "blobOLD".into()),
            ("cccc3333".into(), "2026-05-01".into(), "blobOLD".into()),
        ];
        let kind = match_blob_in_history("blobOLD", &history);
        // Newest match wins: it answers "how far behind", not "how old is this
        // content".
        assert_eq!(
            kind,
            DriftKind::Stale {
                commit: "bbbb2222".into(),
                date: "2026-06-27".into()
            }
        );
        assert!(kind.is_safe_to_discard());
    }

    #[test]
    fn blob_absent_from_history_is_unique_and_never_discardable() {
        let history = vec![("aaaa1111".into(), "2026-07-29".into(), "blobNEW".into())];
        let kind = match_blob_in_history("blobUNSEEN", &history);
        assert_eq!(kind, DriftKind::Unique);
        assert!(!kind.is_safe_to_discard());
    }

    #[test]
    fn a_truncated_search_is_unknown_rather_than_unique() {
        // Hitting the limit means we stopped looking, which must never be
        // reported as "nothing matches" — that would license a discard.
        let history: Vec<(String, String, String)> = (0..HISTORY_SEARCH_LIMIT)
            .map(|i| (format!("c{}", i), "2026-01-01".into(), format!("blob{}", i)))
            .collect();
        let kind = match_blob_in_history("blobNOWHERE", &history);
        assert_eq!(kind, DriftKind::Unknown);
        assert!(!kind.is_safe_to_discard());
    }

    #[test]
    fn empty_worktree_blob_is_unknown() {
        assert_eq!(match_blob_in_history("", &[]), DriftKind::Unknown);
    }

    #[test]
    fn deleted_and_unknown_are_never_safe_to_discard() {
        assert!(!DriftKind::Deleted.is_safe_to_discard());
        assert!(!DriftKind::Unknown.is_safe_to_discard());
    }

    #[test]
    fn merian_at_67_days_is_a_deadlock() {
        // Its last successful pull was 2026-06-27; the abort fired 2026-09-03.
        assert!(is_deadlock(Some("2026-06-27T01:00:00+00:00"), now()));
    }

    #[test]
    fn one_night_of_uncommitted_work_is_not_a_deadlock() {
        assert!(!is_deadlock(Some("2026-09-02T01:00:00+00:00"), now()));
    }

    #[test]
    fn the_threshold_is_inclusive() {
        assert!(!is_deadlock(Some("2026-09-01T00:00:00+00:00"), now())); // 2 days
        assert!(is_deadlock(Some("2026-08-31T00:00:00+00:00"), now())); // 3 days
    }

    #[test]
    fn a_machine_that_never_synced_is_not_a_deadlock() {
        // Fresh bootstrap. It should not greet its owner with an incident.
        assert!(!is_deadlock(None, now()));
        assert!(!is_deadlock(Some("not-a-timestamp"), now()));
    }

    #[test]
    fn first_deadlock_notifies_then_falls_silent_for_a_week() {
        assert!(should_notify(None, now()));
        assert!(!should_notify(Some("2026-09-02T12:00:00+00:00"), now()));
        assert!(!should_notify(Some("2026-08-30T12:00:00+00:00"), now())); // 4 days
        assert!(should_notify(Some("2026-08-27T12:00:00+00:00"), now())); // 7 days
    }

    #[test]
    fn an_unparseable_notify_stamp_errs_toward_telling_the_user() {
        // Silence is the failure mode this module exists to prevent, so a
        // corrupt stamp notifies rather than suppresses.
        assert!(should_notify(Some(""), now()));
        assert!(should_notify(Some("garbage"), now()));
    }

    #[test]
    fn days_since_is_none_rather_than_zero_when_absent() {
        assert_eq!(days_since_rfc3339(None, now()), None);
        assert_eq!(days_since_rfc3339(Some("nonsense"), now()), None);
        assert_eq!(
            days_since_rfc3339(Some("2026-09-01T12:00:00+00:00"), now()),
            Some(2)
        );
    }

    #[test]
    fn all_stale_message_says_nothing_would_be_lost() {
        let files: Vec<TriagedFile> = MERIAN_PORCELAIN
            .iter()
            .filter_map(|l| parse_porcelain_entry(l))
            .map(|(status, path)| TriagedFile {
                status,
                path,
                kind: DriftKind::Stale {
                    commit: "bbbb2222".into(),
                    date: "2026-06-27".into(),
                },
            })
            .collect();
        let msg = deadlock_message("merian", 67, &files);
        assert!(msg.contains("67 days"));
        assert!(msg.contains("nothing would be lost"));
    }

    #[test]
    fn mixed_message_counts_both_sides() {
        let files = vec![
            TriagedFile {
                status: " M".into(),
                path: ".Brewfile".into(),
                kind: DriftKind::Stale {
                    commit: "bbbb2222".into(),
                    date: "2026-06-27".into(),
                },
            },
            TriagedFile {
                status: " M".into(),
                path: ".zshrc".into(),
                kind: DriftKind::Unique,
            },
        ];
        let msg = deadlock_message("merian", 5, &files);
        assert!(msg.contains("1 are stale"));
        assert!(msg.contains("1 carry content never committed"));
        assert!(!msg.contains("nothing would be lost"));
    }

    #[test]
    fn env_lines_are_read_with_or_without_the_export_prefix() {
        // The fleet's tier files use `export NAME=value`. A bare
        // strip_prefix("NTFY_TOKEN=") matched none of them, and the only
        // symptom was an alert that silently never arrived.
        assert_eq!(
            parse_env_line("export NTFY_TOKEN=abc123", "NTFY_TOKEN").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            parse_env_line("NTFY_TOKEN=abc123", "NTFY_TOKEN").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            parse_env_line("export NTFY_TOKEN=\"abc123\"", "NTFY_TOKEN").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            parse_env_line("  export NTFY_TOKEN='abc123'  ", "NTFY_TOKEN").as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn env_line_parsing_rejects_comments_empties_and_near_misses() {
        // A commented mention must not be read as a value: tier-edge.env
        // carries exactly such a line about NTFY_TOKEN.
        assert_eq!(
            parse_env_line(
                "# NTFY_TOKEN added May 5, 2026 — copied from tier-servers",
                "NTFY_TOKEN"
            ),
            None
        );
        assert_eq!(parse_env_line("NTFY_TOKEN=", "NTFY_TOKEN"), None);
        assert_eq!(parse_env_line("NTFY_TOKEN=\"\"", "NTFY_TOKEN"), None);
        assert_eq!(parse_env_line("OTHER=x", "NTFY_TOKEN"), None);
        // A name that merely starts the same must not match.
        assert_eq!(parse_env_line("NTFY_TOKEN_OLD=x", "NTFY_TOKEN"), None);
    }

    #[test]
    fn pathspec_is_root_relative_so_cwd_cannot_change_the_answer() {
        // Regression guard. Both git calls in `classify_path` are cwd-sensitive,
        // and `nit update` runs from wherever launchd or the user happens to be.
        // With a bare pathspec, every file came back `Unknown` from any
        // directory other than $HOME — a broken answer wearing a cautious one's
        // clothes. Measured against this repo: 0 commits found instead of 14.
        assert_eq!(root_pathspec(".gitignore"), ":/.gitignore");
        assert_eq!(
            root_pathspec("dotfiles/system/darwin/opt/AdGuardHome/AdGuardHome.yaml"),
            ":/dotfiles/system/darwin/opt/AdGuardHome/AdGuardHome.yaml"
        );
        assert!(root_pathspec(".Brewfile").starts_with(":/"));
    }

    #[test]
    fn notify_with_an_empty_url_is_a_no_op() {
        assert!(!notify_ntfy("", "t", "b"));
        assert!(!notify_ntfy("   ", "t", "b"));
    }
}
