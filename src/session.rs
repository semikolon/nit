//! A private staging area per session (2026-10-18).
//!
//! Every session on a machine shared one git index, and nit kept a side list
//! of which staged entries were whose. Each git command that touched the index
//! without updating the list became its own bug (23 add/commit fixes April to
//! October 2026). Here each session stages into its own index file
//! (`GIT_INDEX_FILE`), and the record of what it staged lives beside it, so a
//! commit takes exactly what this session staged, from its own index, onto
//! whatever HEAD is now. Design and the history behind it:
//! `docs/session_private_index_design_2026-10-18.md`.
//!
//! Files, in `~/.local/share/nit/sessions/`:
//! - `<id>.index`: the session's index, kept equal to HEAD plus its staging;
//! - `<id>.json`: when the session started, and for each staged path the blob
//!   HEAD had there when the session first staged it (the compare-and-swap
//!   base), plus the newest commit the session made (what `--amend` may touch).
//!
//! Lifetime: created by the first staging command, the index removed once a
//! commit leaves nothing staged. A file whose start time is not the live
//! session's belongs to an ended session that had the same id (PID reuse); it
//! moves to `ended/`, is never inherited, and is deleted after 30 days.

use crate::config::GitStrategy;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const ZERO: &str = "0000000000000000000000000000000000000000";
const ENDED_KEEP_DAYS: u64 = 30;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Meta {
    #[serde(default)]
    pub started: String,
    /// Staged path → the blob HEAD had at that path when this session first
    /// staged it (`None`: the path did not exist in HEAD).
    #[serde(default)]
    pub staged: BTreeMap<String, Option<String>>,
    /// The newest commit this session made.
    #[serde(default)]
    pub last_commit: Option<String>,
    /// HEAD when the index was last rebuilt: what lets a later rebuild tell
    /// staging done with raw git on this index (`GIT_INDEX_FILE=$(nit
    /// session-index) git apply --cached`) apart from commits made since.
    #[serde(default)]
    pub built_from: Option<String>,
}

/// One index entry: mode and blob.
pub type Entry = (String, String);

pub struct Session {
    pub id: String,
    pub started: String,
}

impl Session {
    /// The session this nit process belongs to. `NIT_SESSION_ANCHOR` (with
    /// `NIT_SESSION_STARTED`) overrides the process-tree walk: tests, and
    /// scripts that want an explicit identity.
    pub fn current() -> Session {
        if let Ok(id) = std::env::var("NIT_SESSION_ANCHOR")
            && !id.trim().is_empty()
        {
            return Session {
                id: id.trim().to_string(),
                started: std::env::var("NIT_SESSION_STARTED").unwrap_or_default(),
            };
        }
        let pid = crate::syncbase::get_session_anchor();
        Session {
            id: pid.to_string(),
            started: start_time(pid),
        }
    }

    pub fn dir() -> PathBuf {
        crate::syncbase::nit_data_dir().join("sessions")
    }

    pub fn index_path(&self) -> PathBuf {
        Self::dir().join(format!("{}.index", self.id))
    }

    fn meta_path(&self) -> PathBuf {
        Self::dir().join(format!("{}.json", self.id))
    }

    /// This session's record. A record left by an ended session with the same
    /// id is moved aside first, so its staging is never inherited.
    pub fn load(&self) -> Meta {
        let fresh = Meta {
            started: self.started.clone(),
            ..Meta::default()
        };
        let Ok(text) = std::fs::read_to_string(self.meta_path()) else {
            return fresh;
        };
        let Ok(meta) = serde_json::from_str::<Meta>(&text) else {
            self.retire("unreadable");
            return fresh;
        };
        if meta.started != self.started {
            if !meta.staged.is_empty() {
                eprintln!(
                    "nit: an ended session with id {} left {} staged path(s); they are not \
                     this session's, so they were set aside in {}",
                    self.id,
                    meta.staged.len(),
                    Self::dir().join("ended").display()
                );
            }
            self.retire("ended");
            return fresh;
        }
        meta
    }

    pub fn save(&self, meta: &Meta) {
        let _ = std::fs::create_dir_all(Self::dir());
        if meta.staged.is_empty() {
            let _ = std::fs::remove_file(self.index_path());
        }
        if meta.staged.is_empty() && meta.last_commit.is_none() && meta.built_from.is_none() {
            let _ = std::fs::remove_file(self.meta_path());
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(meta) {
            let tmp = self.meta_path().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, self.meta_path());
            }
        }
    }

    fn retire(&self, why: &str) {
        let ended = Self::dir().join("ended");
        let _ = std::fs::create_dir_all(&ended);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        for (from, ext) in [(self.meta_path(), "json"), (self.index_path(), "index")] {
            if from.exists() {
                let _ = std::fs::rename(
                    &from,
                    ended.join(format!("{}-{why}-{stamp}.{ext}", self.id)),
                );
            }
        }
    }

    /// Make the session's index HEAD plus this session's staged entries, so it
    /// is never behind what other sessions committed meanwhile. Entries staged
    /// on this index with raw git since the last rebuild join the record.
    pub fn rebuild(&self, strategy: &GitStrategy, meta: &mut Meta) -> Res<()> {
        let _ = std::fs::create_dir_all(Self::dir());
        let index = self.index_path();
        if index.exists()
            && let Some(built) = meta.built_from.clone()
        {
            let out = run_in(
                strategy,
                Some(&index),
                &[
                    "diff-index",
                    "--cached",
                    "--name-only",
                    "-z",
                    "--no-renames",
                    &built,
                ],
            )?;
            let extra: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .split('\0')
                .filter(|p| !p.is_empty() && !meta.staged.contains_key(*p))
                .map(String::from)
                .collect();
            if !extra.is_empty() {
                let base = blobs_at(strategy, &built, &extra)?;
                for p in extra {
                    let b = base.get(&p).cloned();
                    meta.staged.insert(p, b);
                }
            }
        }
        let next = index.with_extension("index.new");
        seed(strategy, &next)?;
        if !meta.staged.is_empty() && index.exists() {
            let paths: Vec<String> = meta.staged.keys().cloned().collect();
            let mine = index_entries(strategy, &index, Some(&paths))?;
            copy_entries(strategy, &next, &paths, &mine)?;
        }
        std::fs::rename(&next, &index)?;
        meta.built_from = head(strategy);
        Ok(())
    }

    /// Run a staging git command (`add`, `rm`, `mv`, `reset <paths>`,
    /// `restore`) against this session's index, and record what it changed.
    pub fn stage(&self, strategy: &GitStrategy, args: &[&str]) -> Res<()> {
        let mut meta = self.load();
        self.rebuild(strategy, &mut meta)?;
        let index = self.index_path();
        let before = index_entries(strategy, &index, None)?;
        let status = crate::git::git_command(strategy)
            .env("GIT_INDEX_FILE", &index)
            .args(args)
            .status()?;
        let after = index_entries(strategy, &index, None).unwrap_or_else(|_| before.clone());
        note_changes(&mut meta, &before, &after);
        self.save(&meta);
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "git {} failed ({})",
                args.first().copied().unwrap_or("git"),
                status
                    .code()
                    .map(|c| format!("exit {c}"))
                    .unwrap_or_else(|| "signal".into())
            )
            .into())
        }
    }

    /// Bring in what this session staged with the nit before 2026-10-18: the
    /// paths its side list names that are still staged in the shared index.
    pub fn adopt_old_staging(&self, strategy: &GitStrategy) {
        let Ok(anchor) = self.id.parse::<u32>() else {
            return;
        };
        let listed = crate::syncbase::read_session_staged(anchor);
        if listed.is_empty() {
            return;
        }
        let shared = crate::syncbase::staged_paths(strategy).unwrap_or_default();
        let paths: Vec<String> = listed.into_iter().filter(|p| shared.contains(p)).collect();
        crate::syncbase::clear_session_staged(anchor);
        if paths.is_empty() {
            return;
        }
        let mut meta = self.load();
        if self.rebuild(strategy, &mut meta).is_err() {
            return;
        }
        let head = head(strategy);
        let base = head
            .as_deref()
            .map(|h| blobs_at(strategy, h, &paths).unwrap_or_default())
            .unwrap_or_default();
        let entries = shared_entries(strategy, &paths).unwrap_or_default();
        if copy_entries(strategy, &self.index_path(), &paths, &entries).is_err() {
            return;
        }
        for p in &paths {
            meta.staged
                .entry(p.clone())
                .or_insert_with(|| base.get(p).cloned());
        }
        self.save(&meta);
        eprintln!(
            "nit: {} path(s) staged with the earlier nit are now this session's",
            paths.len()
        );
    }

    /// The paths this session has staged (raw-git staging on its index included).
    pub fn staged(&self, strategy: &GitStrategy) -> Vec<String> {
        let mut meta = self.load();
        if self.index_path().exists() && self.rebuild(strategy, &mut meta).is_ok() {
            self.save(&meta);
        }
        meta.staged.keys().cloned().collect()
    }

    /// This session's index, current, for `GIT_INDEX_FILE` (seeded if new).
    pub fn ready_index(&self, strategy: &GitStrategy) -> Res<PathBuf> {
        let mut meta = self.load();
        self.rebuild(strategy, &mut meta)?;
        // Kept even with nothing staged yet, so raw staging on it is noticed.
        let _ = std::fs::create_dir_all(Self::dir());
        if let Ok(json) = serde_json::to_string_pretty(&meta) {
            std::fs::write(self.meta_path(), json)?;
        }
        Ok(self.index_path())
    }

    /// Commit `scope` (paths this session staged) onto HEAD as it is now.
    /// `commit_args` is the full `git commit …` argument list without any
    /// pathspec. Returns the new HEAD and the paths it committed; `None` when
    /// HEAD already held everything staged (another commit put it there).
    pub fn commit(
        &self,
        strategy: &GitStrategy,
        scope: &[String],
        commit_args: &[String],
        amend: bool,
    ) -> Res<(Option<String>, Vec<String>)> {
        let _lock = commit_lock()?;
        let mut meta = self.load();
        self.rebuild(strategy, &mut meta)?;
        let before = head(strategy);
        let mine = index_entries(strategy, &self.index_path(), Some(scope))?;

        // Compare-and-swap per path: what HEAD holds must still be what it held
        // when this session staged the path. HEAD holding exactly what this
        // session staged is no conflict either: there is nothing left to do for
        // that path (2026-10-18: the nightly commits an always_commit file a
        // session may have staged identically).
        let mut scope: Vec<String> = scope.to_vec();
        if let Some(h) = before.as_deref() {
            let now = blobs_at(strategy, h, &scope)?;
            let mut already = Vec::new();
            let mut moved = Vec::new();
            for p in &scope {
                let at_head = now.get(p).cloned();
                if meta.staged.get(p).cloned().flatten() == at_head {
                    continue;
                }
                if mine.get(p).map(|(_, sha)| sha.clone()) == at_head {
                    already.push(p.clone());
                } else {
                    moved.push(p.clone());
                }
            }
            if !moved.is_empty() {
                for p in &moved {
                    let by = crate::git::git_output_with(
                        strategy,
                        &["log", "-1", "--format=%h %s", "--", &format!(":/{p}")],
                    )
                    .unwrap_or_default();
                    eprintln!("nit:     {p}  (changed by {})", by.trim());
                }
                return Err(format!(
                    "{} path(s) were committed by someone else since this session staged \
                     them. Look at the new version, then `nit add` them again",
                    moved.len()
                )
                .into());
            }
            if !already.is_empty() {
                eprintln!(
                    "nit: {} path(s) are already in HEAD exactly as staged; nothing to commit for them:",
                    already.len()
                );
                for p in &already {
                    eprintln!("nit:     {p}");
                    meta.staged.remove(p);
                }
                scope.retain(|p| !already.contains(p));
                if scope.is_empty() && !amend {
                    if !meta.staged.is_empty() {
                        self.rebuild(strategy, &mut meta)?;
                    }
                    self.save(&meta);
                    return Ok((None, Vec::new()));
                }
            }
        }

        // The commit's index: HEAD plus exactly the scope's entries.
        let temp = Self::dir().join(format!("{}.commit-index", self.id));
        seed(strategy, &temp)?;
        copy_entries(strategy, &temp, &scope, &mine)?;
        let refs: Vec<&str> = commit_args.iter().map(String::as_str).collect();
        let status = crate::git::git_command(strategy)
            .env("GIT_INDEX_FILE", &temp)
            .args(&refs)
            .status();
        let _ = std::fs::remove_file(&temp);
        let status = status?;
        if !status.success() {
            return Err(format!(
                "git commit failed ({}). Nothing was committed: what this session staged \
                 is still staged",
                status
                    .code()
                    .map(|c| format!("exit {c}"))
                    .unwrap_or_else(|| "signal".into())
            )
            .into());
        }
        let after = head(strategy).ok_or("no HEAD after the commit")?;

        // HEAD's parent must be the HEAD read above (for an amend, its parent).
        let expected = if amend {
            before
                .as_deref()
                .and_then(|h| rev(strategy, &format!("{h}^")))
        } else {
            before.clone()
        };
        let parent = rev(strategy, &format!("{after}^"));
        if expected.is_some() && parent != expected {
            eprintln!(
                "nit: \u{26a0} another commit landed while this one was made; check \
                 `nit log -3` (expected parent {}, got {})",
                expected.as_deref().unwrap_or("?"),
                parent.as_deref().unwrap_or("none")
            );
        }

        // The shared index: the committed paths now equal the new HEAD.
        if !scope.is_empty() {
            let mut args = vec!["reset".to_string(), "-q".to_string(), "--".to_string()];
            args.extend(scope.iter().map(|p| format!(":/{p}")));
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let _ = crate::git::git_command(strategy)
                .current_dir(crate::git::work_tree())
                .args(&refs)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        for p in &scope {
            meta.staged.remove(p);
        }
        meta.last_commit = Some(after.clone());
        // The index now matches the new commit for the paths just committed. Rebuilding against
        // the commit before it read them as fresh staging, based on that older commit, and the
        // next commit of the same file was refused as someone else's (2026-10-25).
        meta.built_from = Some(after.clone());
        if !meta.staged.is_empty() {
            self.rebuild(strategy, &mut meta)?;
        }
        self.save(&meta);
        Ok((Some(after), scope))
    }
}

/// Delete ended sessions' files older than 30 days. Housekeeping; never fails.
pub fn prune_ended() {
    let ended = Session::dir().join("ended");
    let Ok(rd) = std::fs::read_dir(&ended) else {
        return;
    };
    let limit = std::time::Duration::from_secs(ENDED_KEEP_DAYS * 86_400);
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > limit);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Held while a commit is assembled and while `nit update` pulls, so two nit
/// processes never move HEAD at once.
pub fn commit_lock() -> Res<std::fs::File> {
    let path = crate::syncbase::nit_data_dir().join("commit.lock");
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    if f.try_lock().is_err() {
        eprintln!("nit: waiting for another nit commit or update to finish…");
        f.lock()?;
    }
    Ok(f)
}

/// What changed between two listings of an index, recorded as staging.
/// A path entering the record keeps the blob it had before (the CAS base);
/// a path whose entry is back to that blob leaves the record.
pub fn note_changes(
    meta: &mut Meta,
    before: &BTreeMap<String, Entry>,
    after: &BTreeMap<String, Entry>,
) {
    let mut changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .filter(|p| before.get(*p) != after.get(*p))
        .collect();
    changed.sort();
    changed.dedup();
    for p in changed {
        let base = meta
            .staged
            .entry(p.clone())
            .or_insert_with(|| before.get(p).map(|(_, sha)| sha.clone()))
            .clone();
        if after.get(p).map(|(_, sha)| sha.clone()) == base {
            meta.staged.remove(p);
        }
    }
}

fn start_time(pid: u32) -> String {
    Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn run_in(strategy: &GitStrategy, index: Option<&Path>, args: &[&str]) -> std::io::Result<Output> {
    let mut cmd = crate::git::git_command(strategy);
    cmd.current_dir(crate::git::work_tree());
    if let Some(i) = index {
        cmd.env("GIT_INDEX_FILE", i);
    }
    cmd.args(args).output()
}

pub fn head(strategy: &GitStrategy) -> Option<String> {
    rev(strategy, "HEAD")
}

fn rev(strategy: &GitStrategy, spec: &str) -> Option<String> {
    run_in(strategy, None, &["rev-parse", "-q", "--verify", spec])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A fresh index holding HEAD (empty when there is no commit yet).
fn seed(strategy: &GitStrategy, index: &Path) -> Res<()> {
    let _ = std::fs::remove_file(index);
    let args: &[&str] = if head(strategy).is_some() {
        &["read-tree", "HEAD"]
    } else {
        &["read-tree", "--empty"]
    };
    let out = run_in(strategy, Some(index), args)?;
    if !out.status.success() {
        return Err(format!(
            "git read-tree failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(())
}

fn pathspecs(paths: &[String]) -> Vec<String> {
    paths.iter().map(|p| format!(":(top,literal){p}")).collect()
}

/// Entries of an index file, by work-tree-root path. `paths` limits it.
pub fn index_entries(
    strategy: &GitStrategy,
    index: &Path,
    paths: Option<&[String]>,
) -> Res<BTreeMap<String, Entry>> {
    let specs = match paths {
        Some([]) => return Ok(BTreeMap::new()),
        Some(p) => pathspecs(p),
        None => vec![":/".to_string()],
    };
    let mut args = vec!["ls-files", "-s", "-z", "--full-name", "--"];
    args.extend(specs.iter().map(String::as_str));
    let out = run_in(strategy, Some(index), &args)?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(parse_ls_files(&String::from_utf8_lossy(&out.stdout)))
}

fn shared_entries(strategy: &GitStrategy, paths: &[String]) -> Res<BTreeMap<String, Entry>> {
    let mut args = vec!["ls-files", "-s", "-z", "--full-name", "--"];
    let specs = pathspecs(paths);
    args.extend(specs.iter().map(String::as_str));
    let out = run_in(strategy, None, &args)?;
    Ok(parse_ls_files(&String::from_utf8_lossy(&out.stdout)))
}

/// `mode sha stage<TAB>path` records, NUL-separated. Stage-0 entries only.
pub fn parse_ls_files(out: &str) -> BTreeMap<String, Entry> {
    out.split('\0')
        .filter_map(|rec| {
            let (meta, path) = rec.split_once('\t')?;
            let mut f = meta.split(' ');
            let (mode, sha, stage) = (f.next()?, f.next()?, f.next()?);
            (stage == "0").then(|| (path.to_string(), (mode.to_string(), sha.to_string())))
        })
        .collect()
}

/// Blobs at a commit for the given paths (absent paths are left out).
fn blobs_at(strategy: &GitStrategy, rev: &str, paths: &[String]) -> Res<BTreeMap<String, String>> {
    if paths.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut args = vec!["ls-tree", "-z", "-r", "--full-tree", rev, "--"];
    args.extend(paths.iter().map(String::as_str));
    let out = run_in(strategy, None, &args)?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter_map(|rec| {
            let (meta, path) = rec.split_once('\t')?;
            let sha = meta.split(' ').nth(2)?;
            Some((path.to_string(), sha.to_string()))
        })
        .collect())
}

/// Set `paths` in `index` to `entries`; a path missing from `entries` is removed.
fn copy_entries(
    strategy: &GitStrategy,
    index: &Path,
    paths: &[String],
    entries: &BTreeMap<String, Entry>,
) -> Res<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut input = String::new();
    for p in paths {
        match entries.get(p) {
            Some((mode, sha)) => input.push_str(&format!("{mode} {sha} 0\t{p}\0")),
            None => input.push_str(&format!("0 {ZERO} 0\t{p}\0")),
        }
    }
    let mut child = crate::git::git_command(strategy)
        .current_dir(crate::git::work_tree())
        .env("GIT_INDEX_FILE", index)
        .args(["update-index", "-z", "--index-info"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("no stdin for git update-index")?
        .write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(format!(
            "git update-index failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(sha: &str) -> Entry {
        ("100644".into(), sha.into())
    }

    #[test]
    fn a_new_staged_path_keeps_its_old_blob_as_base_and_an_unstage_forgets_it() {
        let mut meta = Meta::default();
        let base: BTreeMap<String, Entry> = [("a".to_string(), e("old"))].into();
        let staged: BTreeMap<String, Entry> =
            [("a".to_string(), e("new")), ("b".to_string(), e("b1"))].into();
        note_changes(&mut meta, &base, &staged);
        assert_eq!(meta.staged.get("a"), Some(&Some("old".to_string())));
        assert_eq!(meta.staged.get("b"), Some(&None), "b did not exist before");

        note_changes(&mut meta, &staged, &base); // back as it was
        assert!(meta.staged.is_empty(), "{:?}", meta.staged);
    }

    #[test]
    fn a_removal_is_staging() {
        let mut meta = Meta::default();
        let before: BTreeMap<String, Entry> = [("gone".to_string(), e("x"))].into();
        note_changes(&mut meta, &before, &BTreeMap::new());
        assert_eq!(meta.staged.get("gone"), Some(&Some("x".to_string())));
    }

    #[test]
    fn restaging_keeps_the_first_base() {
        let mut meta = Meta::default();
        let v0: BTreeMap<String, Entry> = [("a".to_string(), e("v0"))].into();
        let v1: BTreeMap<String, Entry> = [("a".to_string(), e("v1"))].into();
        let v2: BTreeMap<String, Entry> = [("a".to_string(), e("v2"))].into();
        note_changes(&mut meta, &v0, &v1);
        note_changes(&mut meta, &v1, &v2);
        assert_eq!(meta.staged.get("a"), Some(&Some("v0".to_string())));
    }

    #[test]
    fn ls_files_records_are_parsed_and_conflict_stages_skipped() {
        let out = [
            "100644 aaa 0\ta b.txt",
            "100644 bbb 1\tc.txt",
            "100755 ccc 0\td",
            "",
        ]
        .join("\0");
        let got = parse_ls_files(&out);
        assert_eq!(got.len(), 2);
        assert_eq!(got["a b.txt"], e("aaa"));
        assert_eq!(got["d"].0, "100755");
    }
}
