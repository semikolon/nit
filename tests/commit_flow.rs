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
    assert_eq!(sb.staged(), names(&["a.txt"]));

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
