//! End to end: the built `nit` against a throwaway `$HOME`.
//!
//! `nit commit` used to drop git's exit status, so a commit git refused was
//! reported as "committed N path(s)" and the session's record of what it had
//! staged was erased. On 2026-10-11 that hid a refusal for two days: a staged
//! file name with non-ASCII letters reached git C-quoted, git rejected the
//! pathspec, nit said the files were committed, and the files stayed staged
//! with no session owning them. The unit tests could not see any of it, since
//! every one of them stops before git runs. These run git.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Sandbox {
    _dir: tempfile::TempDir,
    home: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // macOS hands out /var/…, which is a link to /private/var/…; git and
        // nit must agree on one spelling of the work tree.
        let home = dir.path().canonicalize().unwrap();
        fs::create_dir_all(home.join("dotfiles")).unwrap();
        fs::create_dir_all(home.join(".config/nit")).unwrap();
        fs::write(
            home.join("dotfiles/fleet.toml"),
            "[machines.testbox]\nssh_host = \"localhost\"\nrole = [\"dev\"]\n",
        )
        .unwrap();
        fs::write(
            home.join(".config/nit/local.toml"),
            "machine = \"testbox\"\n",
        )
        .unwrap();
        let sandbox = Sandbox { _dir: dir, home };
        let repo = sandbox.repo();
        fs::create_dir_all(repo.parent().unwrap()).unwrap();
        let init = sandbox
            .base("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&repo)
            .output()
            .unwrap();
        assert!(init.status.success(), "git init: {}", text(&init.stderr));
        sandbox
    }

    fn repo(&self) -> PathBuf {
        self.home.join(".local/share/nit/repo.git")
    }

    /// A command that sees only the sandbox: its own `$HOME`, no user or
    /// system git config, and a PATH without wrapper directories that resolve
    /// themselves through the real `$HOME`.
    fn base(&self, program: &str) -> Command {
        let path = std::env::var("PATH").unwrap_or_default();
        let kept: Vec<&str> = path.split(':').filter(|p| !p.contains("/shims")).collect();
        let mut cmd = Command::new(program);
        cmd.current_dir(&self.home)
            .env("HOME", &self.home)
            .env("PATH", kept.join(":"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid");
        cmd
    }

    fn nit(&self, args: &[&str]) -> Output {
        self.base(env!("CARGO_BIN_EXE_nit"))
            .args(args)
            .output()
            .unwrap()
    }

    /// `nit` as a named session: two names are two sessions working side by side.
    fn nit_as(&self, session: &str, args: &[&str]) -> Output {
        self.base(env!("CARGO_BIN_EXE_nit"))
            .env("NIT_SESSION_ANCHOR", session)
            .env("NIT_SESSION_STARTED", "t0")
            .args(args)
            .output()
            .unwrap()
    }

    fn ok_as(&self, session: &str, args: &[&str]) -> String {
        let out = self.nit_as(session, args);
        assert!(
            out.status.success(),
            "nit {:?} as {}: {}",
            args,
            session,
            said(&out)
        );
        said(&out)
    }

    /// What this (default) session has staged, as `nit diff --cached` shows it.
    fn my_staged(&self) -> Vec<String> {
        let out = self.nit(&["diff", "--cached", "--name-only", "-z"]);
        assert!(out.status.success(), "{}", said(&out));
        text(&out.stdout)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect()
    }

    /// A first commit made outside nit, so HEAD exists.
    fn base_commit(&self, files: &[(&str, &str)]) {
        for (name, content) in files {
            self.write(name, content);
            self.git_ok(&["add", name]);
        }
        self.git_ok(&["commit", "-q", "-m", "base"]);
    }

    fn show(&self, rev_path: &str) -> String {
        self.git_ok(&["show", rev_path])
    }

    /// Raw git on the sandbox repository: how "another session" or a hand-run
    /// command touches the index without telling nit.
    fn git(&self, args: &[&str]) -> Output {
        self.base("git")
            .arg("--git-dir")
            .arg(self.repo())
            .arg("--work-tree")
            .arg(&self.home)
            .args(args)
            .output()
            .unwrap()
    }

    fn git_ok(&self, args: &[&str]) -> String {
        let out = self.git(args);
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            text(&out.stderr)
        );
        text(&out.stdout)
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.home.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// Staged paths as their real names.
    fn staged(&self) -> Vec<String> {
        self.git_ok(&["diff", "--cached", "--name-only", "-z"])
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect()
    }

    /// Paths of the newest commit as their real names.
    fn committed(&self) -> Vec<String> {
        self.git_ok(&[
            "show",
            "--name-only",
            "--format=",
            "-z",
            "--no-renames",
            "HEAD",
        ])
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
    }

    fn commit_count(&self) -> usize {
        let out = self.git(&["rev-list", "--count", "HEAD"]);
        if !out.status.success() {
            return 0; // no commit yet
        }
        text(&out.stdout).trim().parse().unwrap()
    }

    fn subject(&self) -> String {
        self.git_ok(&["log", "-1", "--format=%s"])
            .trim()
            .to_string()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn said(out: &Output) -> String {
    format!("{}{}", text(&out.stdout), text(&out.stderr))
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_file_name_with_non_ascii_letters_is_committed() {
    let sb = Sandbox::new();
    sb.write("docs/räksmörgås 2019.txt", "a\n");
    sb.write("docs/plain.txt", "b\n");

    let add = sb.nit(&["add", "docs/räksmörgås 2019.txt", "docs/plain.txt"]);
    assert!(add.status.success(), "{}", said(&add));
    let commit = sb.nit(&["commit", "-m", "two files"]);

    assert!(commit.status.success(), "{}", said(&commit));
    assert_eq!(
        sb.committed(),
        names(&["docs/plain.txt", "docs/räksmörgås 2019.txt"])
    );
    assert!(
        sb.staged().is_empty(),
        "nothing may be left staged: {:?}",
        sb.staged()
    );
}

#[test]
fn a_commit_git_refuses_is_reported_as_a_failure_and_the_record_is_kept() {
    let sb = Sandbox::new();
    let hook = sb.repo().join("hooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\necho 'hook says no' >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    sb.write("a.txt", "a\n");
    assert!(sb.nit(&["add", "a.txt"]).status.success());

    let refused = sb.nit(&["commit", "-m", "x"]);

    assert!(
        !refused.status.success(),
        "git refused, so nit must fail: {}",
        said(&refused)
    );
    assert!(
        !said(&refused).contains("nit: committed"),
        "must not claim a commit: {}",
        said(&refused)
    );
    assert_eq!(sb.commit_count(), 0);
    assert_eq!(sb.my_staged(), names(&["a.txt"]));

    // The record of what this session staged survived the refusal: once git
    // is willing, the same `nit commit` goes through with no new `nit add`.
    fs::remove_file(&hook).unwrap();
    let retried = sb.nit(&["commit", "-m", "x"]);
    assert!(retried.status.success(), "{}", said(&retried));
    assert_eq!(sb.committed(), names(&["a.txt"]));
}

#[test]
fn a_session_that_staged_nothing_is_refused_and_can_claim_paths_by_naming_them() {
    let sb = Sandbox::new();
    sb.write("theirs.txt", "t\n");
    sb.git_ok(&["add", "theirs.txt"]); // staged outside nit: no session owns it

    let refused = sb.nit(&["commit", "-m", "x"]);

    assert!(
        !refused.status.success(),
        "the whole index must not be committed for a session that staged nothing: {}",
        said(&refused)
    );
    assert!(
        said(&refused).contains("theirs.txt"),
        "the refusal names what is staged: {}",
        said(&refused)
    );
    assert_eq!(sb.commit_count(), 0);

    // Naming an already staged path claims it for this session.
    assert!(sb.nit(&["add", "theirs.txt"]).status.success());
    let claimed = sb.nit(&["commit", "-m", "claimed"]);
    assert!(claimed.status.success(), "{}", said(&claimed));
    assert_eq!(sb.committed(), names(&["theirs.txt"]));
}

#[test]
fn another_sessions_staged_path_is_left_staged_and_named() {
    let sb = Sandbox::new();
    sb.write("theirs.txt", "t\n");
    sb.git_ok(&["add", "theirs.txt"]);
    sb.write("mine.txt", "m\n");
    assert!(sb.nit(&["add", "mine.txt"]).status.success());

    let commit = sb.nit(&["commit", "-m", "mine"]);

    assert!(commit.status.success(), "{}", said(&commit));
    assert_eq!(sb.committed(), names(&["mine.txt"]));
    assert_eq!(sb.staged(), names(&["theirs.txt"]));
    assert!(said(&commit).contains("theirs.txt"), "{}", said(&commit));
}

#[test]
fn adding_a_path_git_rejects_fails() {
    let sb = Sandbox::new();
    let add = sb.nit(&["add", "no-such-file.txt"]);
    assert!(
        !add.status.success(),
        "git could not add it, so nit add must fail: {}",
        said(&add)
    );
}

/// Leaves a merge in progress with `f.txt` in conflict.
fn conflicting_merge(sb: &Sandbox) {
    sb.write("f.txt", "base\n");
    sb.git_ok(&["add", "f.txt"]);
    sb.git_ok(&["commit", "-q", "-m", "base"]);
    sb.git_ok(&["checkout", "-q", "-b", "other"]);
    sb.write("f.txt", "theirs\n");
    sb.git_ok(&["commit", "-q", "-am", "theirs"]);
    sb.git_ok(&["checkout", "-q", "main"]);
    sb.write("f.txt", "ours\n");
    sb.git_ok(&["commit", "-q", "-am", "ours"]);
    let merge = sb.git(&["merge", "-q", "other"]);
    assert!(!merge.status.success(), "the merge must stop on a conflict");
}

#[test]
fn a_merge_with_a_conflict_left_is_not_concluded() {
    let sb = Sandbox::new();
    conflicting_merge(&sb);

    let commit = sb.nit(&["commit", "-m", "merge"]);

    assert!(!commit.status.success(), "{}", said(&commit));
    assert!(said(&commit).contains("f.txt"), "{}", said(&commit));
    assert!(sb.repo().join("MERGE_HEAD").exists());
}

#[test]
fn a_resolved_merge_is_concluded() {
    let sb = Sandbox::new();
    conflicting_merge(&sb);
    sb.write("f.txt", "both\n");
    assert!(sb.nit(&["add", "f.txt"]).status.success());

    let commit = sb.nit(&["commit", "-m", "merge other"]);

    assert!(commit.status.success(), "{}", said(&commit));
    assert!(
        !sb.repo().join("MERGE_HEAD").exists(),
        "the merge must be over"
    );
    let parents = sb.git_ok(&["log", "-1", "--format=%P"]);
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "a merge commit has two parents"
    );
    assert_eq!(sb.subject(), "merge other");
}

#[test]
fn amend_rewords_the_last_commit_and_leaves_staged_changes_out() {
    let sb = Sandbox::new();
    sb.write("a.txt", "a\n");
    assert!(sb.nit(&["add", "a.txt"]).status.success());
    assert!(sb.nit(&["commit", "-m", "frist"]).status.success());
    sb.write("theirs.txt", "t\n");
    sb.git_ok(&["add", "theirs.txt"]);

    let amend = sb.nit(&["commit", "--amend", "-m", "first"]);

    assert!(amend.status.success(), "{}", said(&amend));
    assert_eq!(sb.commit_count(), 1);
    assert_eq!(sb.subject(), "first");
    assert_eq!(sb.committed(), names(&["a.txt"]));
    assert_eq!(sb.staged(), names(&["theirs.txt"]));
}

#[test]
fn amend_adds_this_sessions_paths_and_keeps_the_message() {
    let sb = Sandbox::new();
    sb.write("a.txt", "a\n");
    assert!(sb.nit(&["add", "a.txt"]).status.success());
    assert!(sb.nit(&["commit", "-m", "first"]).status.success());
    sb.write("b.txt", "b\n");
    assert!(sb.nit(&["add", "b.txt"]).status.success());

    let amend = sb.nit(&["commit", "--amend"]);

    assert!(amend.status.success(), "{}", said(&amend));
    assert_eq!(sb.commit_count(), 1);
    assert_eq!(sb.subject(), "first");
    assert_eq!(sb.committed(), names(&["a.txt", "b.txt"]));
    assert!(sb.staged().is_empty());
}

#[test]
fn amend_is_refused_once_the_commit_is_on_the_remote() {
    let sb = Sandbox::new();
    let remote: &Path = &sb.home.join("remote.git");
    let init = sb
        .base("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(remote)
        .output()
        .unwrap();
    assert!(init.status.success());
    sb.write("a.txt", "a\n");
    assert!(sb.nit(&["add", "a.txt"]).status.success());
    assert!(sb.nit(&["commit", "-m", "first"]).status.success());
    sb.git_ok(&["remote", "add", "origin", remote.to_str().unwrap()]);
    sb.git_ok(&["push", "-q", "-u", "origin", "main"]);

    let amend = sb.nit(&["commit", "--amend", "-m", "reworded"]);

    assert!(!amend.status.success(), "{}", said(&amend));
    assert_eq!(
        sb.subject(),
        "first",
        "a pushed commit must not be rewritten"
    );
}

#[test]
fn a_path_list_commits_only_those_paths_and_keeps_the_rest_staged() {
    // The decision logger commits its own line from inside a session that
    // may have other work staged; that work must stay staged and stay the
    // session's (2026-10-15, drift steward).
    let sb = Sandbox::new();
    sb.write(".claude/log.jsonl", "{\"a\":1}\n");
    sb.write("docs/draft.md", "half done\n");
    assert!(
        sb.nit(&["add", ".claude/log.jsonl", "docs/draft.md"])
            .status
            .success()
    );

    let abs = sb.home.join(".claude/log.jsonl");
    let commit = sb.nit(&["commit", "-m", "log line", "--", abs.to_str().unwrap()]);

    assert!(commit.status.success(), "{}", said(&commit));
    assert_eq!(sb.committed(), names(&[".claude/log.jsonl"]));
    assert_eq!(sb.my_staged(), names(&["docs/draft.md"]));
    // The rest is still this session's: a plain commit now takes it.
    let rest = sb.nit(&["commit", "-m", "the draft"]);
    assert!(rest.status.success(), "{}", said(&rest));
    assert_eq!(sb.committed(), names(&["docs/draft.md"]));
}

#[test]
fn a_path_list_naming_a_path_this_session_did_not_stage_is_refused() {
    let sb = Sandbox::new();
    sb.write("a.md", "a\n");
    sb.write("b.md", "b\n");
    assert!(sb.nit(&["add", "a.md"]).status.success());
    sb.git_ok(&["add", "b.md"]); // staged without nit: nobody's

    let commit = sb.nit(&["commit", "-m", "x", "--", "b.md"]);

    assert!(!commit.status.success(), "{}", said(&commit));
    assert_eq!(sb.commit_count(), 0);
}

// ─── A private staging area per session (2026-10-18) ──────────────────────
// docs/session_private_index_design_2026-10-18.md

#[test]
fn two_sessions_each_commit_only_their_own_and_the_shared_index_stays_clean() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    sb.write("a.txt", "a\n");
    sb.write("b.txt", "b\n");
    sb.ok_as("A", &["add", "a.txt"]);
    sb.ok_as("B", &["add", "b.txt"]);
    assert!(
        sb.staged().is_empty(),
        "staging never reaches the shared index: {:?}",
        sb.staged()
    );

    sb.ok_as("A", &["commit", "-m", "A's"]);
    assert_eq!(sb.committed(), names(&["a.txt"]));
    sb.ok_as("B", &["commit", "-m", "B's"]);
    assert_eq!(sb.committed(), names(&["b.txt"]));
    assert!(sb.staged().is_empty(), "{:?}", sb.staged());
    assert!(
        sb.git_ok(&["status", "--porcelain", "--untracked-files=no"])
            .trim()
            .is_empty()
    );
}

#[test]
fn a_commit_landing_between_add_and_commit_is_kept() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    sb.write("a.txt", "a\n");
    sb.ok_as("A", &["add", "a.txt"]);
    sb.write("b.txt", "b\n");
    sb.ok_as("B", &["add", "b.txt"]);
    sb.ok_as("B", &["commit", "-m", "B's"]);

    sb.ok_as("A", &["commit", "-m", "A's"]);

    assert_eq!(sb.committed(), names(&["a.txt"]));
    assert_eq!(
        sb.git_ok(&["log", "-1", "--format=%s", "HEAD~1"]).trim(),
        "B's"
    );
    assert_eq!(sb.show("HEAD:b.txt"), "b\n", "B's file survives A's commit");
}

#[test]
fn a_path_another_session_committed_since_staging_is_refused() {
    let sb = Sandbox::new();
    sb.base_commit(&[("x.txt", "base\n")]);
    sb.write("x.txt", "A's version\n");
    sb.ok_as("A", &["add", "x.txt"]);
    sb.write("x.txt", "B's version\n");
    sb.ok_as("B", &["add", "x.txt"]);
    sb.ok_as("B", &["commit", "-m", "B's"]);

    let refused = sb.nit_as("A", &["commit", "-m", "A's"]);

    assert!(!refused.status.success(), "{}", said(&refused));
    assert!(said(&refused).contains("x.txt"), "{}", said(&refused));
    assert_eq!(sb.subject(), "B's", "B's commit stands");
    assert_eq!(sb.show("HEAD:x.txt"), "B's version\n");
}

#[test]
fn a_file_committed_twice_while_another_stays_staged_is_not_blamed_on_someone_else() {
    // 2026-10-25: with connections.md left staged, every commit of a session put the paths it
    // had just committed back into the session's record, based on the commit before. The next
    // commit of the same file was refused as "committed by someone else", naming the session's
    // own commit.
    let sb = Sandbox::new();
    sb.base_commit(&[("doc.md", "v1\n"), ("other.md", "o1\n")]);
    sb.write("doc.md", "v2\n");
    sb.write("other.md", "o2\n");
    sb.ok_as("A", &["add", "doc.md", "other.md"]);
    sb.ok_as("A", &["commit", "-m", "doc v2", "--", "doc.md"]);
    assert_eq!(sb.show("HEAD:doc.md"), "v2\n");

    sb.write("doc.md", "v3\n");
    sb.ok_as("A", &["add", "doc.md"]);
    let second = sb.nit_as("A", &["commit", "-m", "doc v3", "--", "doc.md"]);

    assert!(second.status.success(), "{}", said(&second));
    assert_eq!(sb.show("HEAD:doc.md"), "v3\n");
    assert_eq!(
        sb.show("HEAD:other.md"),
        "o1\n",
        "the other staged file stays out"
    );
}

#[test]
fn what_was_staged_is_committed_not_a_later_edit() {
    let sb = Sandbox::new();
    sb.write("a.txt", "staged\n");
    assert!(sb.nit(&["add", "a.txt"]).status.success());
    sb.write("a.txt", "edited after staging\n");

    let commit = sb.nit(&["commit", "-m", "x"]);

    assert!(commit.status.success(), "{}", said(&commit));
    assert_eq!(sb.show("HEAD:a.txt"), "staged\n");
    assert_eq!(
        fs::read_to_string(sb.home.join("a.txt")).unwrap(),
        "edited after staging\n"
    );
}

#[test]
fn a_removal_staged_with_nit_rm_is_this_sessions() {
    let sb = Sandbox::new();
    sb.base_commit(&[("old.txt", "o\n"), ("keep.txt", "k\n")]);
    let rm = sb.nit(&["rm", "--cached", "-q", "old.txt"]);
    assert!(rm.status.success(), "{}", said(&rm));

    let commit = sb.nit(&["commit", "-m", "untrack old.txt"]);

    assert!(commit.status.success(), "{}", said(&commit));
    assert!(
        sb.git_ok(&["ls-files", "old.txt"]).trim().is_empty(),
        "no longer tracked"
    );
    assert!(sb.home.join("old.txt").exists(), "the file stays on disk");
    assert_eq!(sb.show("HEAD:keep.txt"), "k\n");
}

#[test]
fn untracking_a_rendered_template_target_records_the_removal() {
    // spela's config, 2026-10-18: the commit deployed the template first, and
    // the pathspec commit then took the fresh render over the staged removal.
    let sb = Sandbox::new();
    sb.base_commit(&[(".config/app/config.toml", "a = 1\n")]);
    sb.write("dotfiles/templates/.config/app/config.toml.tmpl", "a = 1\n");
    sb.write(".gitignore", ".config/app/config.toml\n");
    assert!(
        sb.nit(&["rm", "--cached", "-q", ".config/app/config.toml"])
            .status
            .success()
    );
    assert!(
        sb.nit(&[
            "add",
            "dotfiles/templates/.config/app/config.toml.tmpl",
            ".gitignore"
        ])
        .status
        .success()
    );

    let mut commit = sb.nit(&["commit", "-m", "template it"]);
    if !commit.status.success() && said(&commit).contains("re-run nit commit") {
        commit = sb.nit(&["commit", "-m", "template it"]);
    }

    assert!(commit.status.success(), "{}", said(&commit));
    assert!(
        sb.git_ok(&["ls-files", ".config/app/config.toml"])
            .trim()
            .is_empty(),
        "the rendered target must not be tracked"
    );
    assert!(
        sb.home.join(".config/app/config.toml").exists(),
        "the target is rendered"
    );
}

#[test]
fn amending_another_sessions_commit_is_refused() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    sb.write("a.txt", "a\n");
    sb.ok_as("A", &["add", "a.txt"]);
    sb.ok_as("A", &["commit", "-m", "A's"]);
    sb.write("b.txt", "b\n");
    sb.ok_as("B", &["add", "b.txt"]);
    sb.ok_as("B", &["commit", "-m", "B's"]);

    let amend = sb.nit_as("A", &["commit", "--amend", "-m", "A's, reworded"]);

    assert!(!amend.status.success(), "{}", said(&amend));
    assert_eq!(sb.subject(), "B's", "B's commit is untouched");
    assert_eq!(sb.commit_count(), 3);
}

#[test]
fn a_reused_session_id_does_not_inherit_staging() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    sb.write("a.txt", "a\n");
    sb.ok_as("7", &["add", "a.txt"]);

    // Same id, another start time: a new process that got the same PID.
    let out = sb
        .base(env!("CARGO_BIN_EXE_nit"))
        .env("NIT_SESSION_ANCHOR", "7")
        .env("NIT_SESSION_STARTED", "t1")
        .args(["commit", "-m", "x"])
        .output()
        .unwrap();

    assert!(!out.status.success(), "{}", said(&out));
    assert_eq!(
        sb.commit_count(),
        1,
        "nothing of the ended session was committed"
    );
}

#[test]
fn staging_done_by_the_old_nit_is_adopted() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    sb.write("a.txt", "a\n");
    sb.git_ok(&["add", "a.txt"]);
    let list = sb.home.join(".local/share/nit/staged/42.json");
    fs::create_dir_all(list.parent().unwrap()).unwrap();
    fs::write(&list, "[\"a.txt\"]").unwrap();

    let commit = sb.nit_as("42", &["commit", "-m", "from before the upgrade"]);

    assert!(commit.status.success(), "{}", said(&commit));
    assert_eq!(sb.committed(), names(&["a.txt"]));
    assert!(sb.staged().is_empty(), "{:?}", sb.staged());
}

#[test]
fn diff_cached_shows_only_this_sessions_staging() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    sb.write("a.txt", "a\n");
    sb.write("b.txt", "b\n");
    sb.ok_as("A", &["add", "a.txt"]);
    sb.ok_as("B", &["add", "b.txt"]);

    let a = sb.ok_as("A", &["diff", "--cached", "--name-only"]);

    assert!(a.contains("a.txt") && !a.contains("b.txt"), "{a}");
}

#[test]
fn staging_part_of_a_file_on_the_session_index_is_committed_as_staged() {
    // 2026-09-30: a session staged two rows of a file other sessions were also
    // editing; the pathspec commit then took the whole work-tree file.
    let sb = Sandbox::new();
    sb.base_commit(&[("rows.md", "one\ntwo\n")]);
    sb.write(
        "rows.md",
        "one\nTWO\nthree (another session's, unfinished)\n",
    );
    let index = sb.ok_as("A", &["session-index"]);
    let index = index.lines().last().unwrap().trim().to_string();
    let patch = "--- a/rows.md\n+++ b/rows.md\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n";
    sb.write("p.patch", patch);
    let apply = sb
        .base("git")
        .env("GIT_INDEX_FILE", &index)
        .args(["--git-dir"])
        .arg(sb.repo())
        .args(["--work-tree"])
        .arg(&sb.home)
        .args(["apply", "--cached", "p.patch"])
        .output()
        .unwrap();
    assert!(apply.status.success(), "{}", said(&apply));

    sb.ok_as("A", &["commit", "-m", "TWO only"]);

    assert_eq!(sb.show("HEAD:rows.md"), "one\nTWO\n");
}

#[test]
fn a_path_already_committed_exactly_as_staged_is_not_a_conflict() {
    // The nightly commits an always_commit file; a session that had staged the
    // same content must not be refused for it.
    let sb = Sandbox::new();
    sb.base_commit(&[("x.txt", "base\n")]);
    sb.write("x.txt", "same\n");
    sb.write("y.txt", "y\n");
    sb.ok_as("A", &["add", "x.txt", "y.txt"]);
    sb.ok_as("B", &["add", "x.txt"]);
    sb.ok_as("B", &["commit", "-m", "B's"]);

    let said = sb.ok_as("A", &["commit", "-m", "A's"]);

    assert!(said.contains("already in HEAD"), "{said}");
    assert_eq!(sb.committed(), names(&["y.txt"]));
    // And with nothing else staged, it is a quiet success, not a failure.
    sb.write("z.txt", "z\n");
    sb.ok_as("C", &["add", "z.txt"]);
    sb.ok_as("D", &["add", "z.txt"]);
    sb.ok_as("D", &["commit", "-m", "D's"]);
    let quiet = sb.ok_as("C", &["commit", "-m", "C's"]);
    assert!(quiet.contains("nothing left to commit"), "{quiet}");
    assert_eq!(sb.subject(), "D's");
}

// ---------------------------------------------------------------------------
// The review gate asks only when a deployed file holds something to lose
// (2026-10-18). Until then every source edit after a review blocked, and the
// review a session wrote before a deploy outlived it, so the next commit of the
// same template was refused with nothing drifted.
// ---------------------------------------------------------------------------

const SRC: &str = "dotfiles/templates/.config/app/conf.toml.tmpl";
const TARGET: &str = ".config/app/conf.toml";

impl Sandbox {
    fn target(&self) -> String {
        fs::read_to_string(self.home.join(TARGET)).unwrap_or_default()
    }

    fn commit_template_once(&self, session: &str, value: &str, stage: &str, msg: &str) -> Output {
        self.write(SRC, value);
        self.ok_as(session, &["add", stage]);
        self.nit_as(session, &["commit", "-m", msg])
    }
}

#[test]
fn an_untouched_template_commits_in_one_call_every_time() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    for (n, value, stage) in [
        (1, "a = 1\n", SRC),
        (2, "a = 2\n", SRC),
        (3, "a = 3\n", TARGET),
    ] {
        let out = sb.commit_template_once("A", value, stage, &format!("edit {n}"));
        assert!(out.status.success(), "commit {n}: {}", said(&out));
        assert!(
            sb.target().contains(value.trim()),
            "commit {n} deployed: {:?}\n{}",
            sb.target(),
            said(&out)
        );
    }
}

#[test]
fn a_hand_edited_target_is_shown_once_then_the_template_wins() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    assert!(
        sb.commit_template_once("A", "a = 1\n", SRC, "first")
            .status
            .success()
    );
    sb.write(TARGET, &format!("{}b = 9\n", sb.target()));

    let first = sb.commit_template_once("A", "a = 2\n", SRC, "second");
    assert!(!first.status.success(), "a hand edit must be shown first");
    assert!(said(&first).contains("b = 9"), "{}", said(&first));
    assert!(sb.target().contains("b = 9"), "nothing deployed yet");

    let second = sb.nit_as("A", &["commit", "-m", "second"]);
    assert!(second.status.success(), "{}", said(&second));
    assert!(sb.target().contains("a = 2") && !sb.target().contains("b = 9"));
    let diff = fs::read_to_string(
        sb.home
            .join(".local/share/nit/drift/.config/app/conf.toml.diff"),
    );
    assert!(
        diff.unwrap_or_default().contains("b = 9"),
        "the hand edit is kept as a diff"
    );
}

#[test]
fn a_target_edited_again_after_it_was_shown_is_shown_again() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    assert!(
        sb.commit_template_once("A", "a = 1\n", SRC, "first")
            .status
            .success()
    );
    sb.write(TARGET, &format!("{}b = 9\n", sb.target()));
    assert!(
        !sb.commit_template_once("A", "a = 2\n", SRC, "second")
            .status
            .success()
    );
    sb.write(TARGET, &format!("{}c = 7\n", sb.target()));

    let again = sb.nit_as("A", &["commit", "-m", "second"]);
    assert!(!again.status.success(), "the newer edit has not been seen");
    assert!(said(&again).contains("c = 7"), "{}", said(&again));
    assert!(sb.nit_as("A", &["commit", "-m", "second"]).status.success());
}

#[test]
fn another_sessions_look_at_a_hand_edit_does_not_count() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    assert!(
        sb.commit_template_once("A", "a = 1\n", SRC, "first")
            .status
            .success()
    );
    sb.write(TARGET, &format!("{}b = 9\n", sb.target()));
    sb.write(SRC, "a = 2\n");
    sb.ok_as("B", &["add", SRC]);
    assert!(
        !sb.nit_as("B", &["commit", "-m", "B looks"])
            .status
            .success()
    );

    sb.ok_as("A", &["add", SRC]);
    let a = sb.nit_as("A", &["commit", "-m", "A commits"]);
    assert!(
        !a.status.success(),
        "A has not seen the hand edit: {}",
        said(&a)
    );
}

#[test]
fn a_file_nit_never_wrote_is_shown_before_it_is_replaced() {
    let sb = Sandbox::new();
    sb.base_commit(&[("seed.txt", "s\n")]);
    sb.write(TARGET, "mine = 1\n");

    let first = sb.commit_template_once("A", "a = 1\n", SRC, "template it");
    assert!(
        !first.status.success(),
        "an existing file must be shown first"
    );
    assert!(said(&first).contains("mine = 1"), "{}", said(&first));
    assert_eq!(sb.target(), "mine = 1\n", "nothing replaced yet");

    assert!(
        sb.nit_as("A", &["commit", "-m", "template it"])
            .status
            .success()
    );
    assert!(sb.target().contains("a = 1"));
    let diff = fs::read_to_string(
        sb.home
            .join(".local/share/nit/drift/.config/app/conf.toml.diff"),
    );
    assert!(
        diff.unwrap_or_default().contains("mine = 1"),
        "the replaced content is kept as a diff"
    );
}
