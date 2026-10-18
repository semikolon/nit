//! End to end: `nit update` against a real remote, with edits in the way.
//!
//! Until 2026-10-15 any edited tracked file stopped the nightly sync before
//! the pull. The Mac sat three days that way on three files the pull never
//! touched. Now the pull goes around edits it does not touch and still stops
//! for the ones it would. These tests run real git, because the whole
//! question is what git does to a dirty work tree.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fleet {
    _dir: tempfile::TempDir,
    root: PathBuf,
    /// This machine's $HOME, with nit's bare repo inside it.
    home: PathBuf,
    /// A plain clone standing in for another machine that pushes.
    other: PathBuf,
}

impl Fleet {
    fn new() -> Self {
        Self::with_sync("")
    }

    /// A fleet whose committed fleet.toml carries a `[sync]` section.
    fn with_sync(sync: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        let remote = root.join("remote.git");
        let other = root.join("other");
        fs::create_dir_all(home.join("dotfiles")).unwrap();
        fs::create_dir_all(home.join(".config/nit")).unwrap();
        fs::write(
            home.join("dotfiles/fleet.toml"),
            format!("[machines.testbox]\nssh_host = \"localhost\"\nrole = [\"dev\"]\n{sync}"),
        )
        .unwrap();
        fs::write(
            home.join(".config/nit/local.toml"),
            "machine = \"testbox\"\n",
        )
        .unwrap();
        let f = Fleet {
            _dir: dir,
            root,
            home,
            other,
        };
        f.run(
            "git",
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        let repo = f.repo();
        fs::create_dir_all(repo.parent().unwrap()).unwrap();
        f.run(
            "git",
            &["init", "-q", "--bare", "-b", "main", repo.to_str().unwrap()],
        );
        f.write("notes.md", "one\n");
        f.write(".hammerspoon/init.lua", "-- base\n");
        f.write(
            "dotfiles/fleet.toml",
            &fs::read_to_string(f.home.join("dotfiles/fleet.toml")).unwrap(),
        );
        f.git(&[
            "add",
            "notes.md",
            ".hammerspoon/init.lua",
            "dotfiles/fleet.toml",
        ]);
        f.git(&["commit", "-q", "-m", "base"]);
        f.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
        f.git(&["push", "-q", "-u", "origin", "main"]);
        f.git(&["config", "branch.main.rebase", "false"]);
        f.run(
            "git",
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                f.other.to_str().unwrap(),
            ],
        );
        f
    }

    fn repo(&self) -> PathBuf {
        self.home.join(".local/share/nit/repo.git")
    }

    fn cmd(&self, program: &str) -> Command {
        let path = std::env::var("PATH").unwrap_or_default();
        let kept: Vec<&str> = path.split(':').filter(|p| !p.contains("/shims")).collect();
        let mut c = Command::new(program);
        c.current_dir(&self.root)
            .env("HOME", &self.home)
            .env("PATH", kept.join(":"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid");
        c
    }

    fn run(&self, program: &str, args: &[&str]) -> String {
        let out = self.cmd(program).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{} {:?}: {}",
            program,
            args,
            said(&out)
        );
        text(&out.stdout)
    }

    fn git(&self, args: &[&str]) -> String {
        let mut full = vec![
            "--git-dir".to_string(),
            self.repo().to_string_lossy().to_string(),
            "--work-tree".to_string(),
            self.home.to_string_lossy().to_string(),
        ];
        full.extend(args.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = full.iter().map(String::as_str).collect();
        self.run("git", &refs)
    }

    /// Another machine changes `name` and pushes.
    fn other_pushes(&self, name: &str, content: &str) {
        let o = self.other.to_str().unwrap();
        let path = self.other.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        self.run("git", &["-C", o, "add", name]);
        self.run(
            "git",
            &["-C", o, "commit", "-q", "-m", "from the other machine"],
        );
        self.run("git", &["-C", o, "push", "-q", "origin", "main"]);
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.home.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.home.join(name)).unwrap()
    }

    fn update(&self) -> Output {
        self.cmd(env!("CARGO_BIN_EXE_nit"))
            .arg("update")
            .output()
            .unwrap()
    }

    fn status_json(&self) -> serde_json::Value {
        let p = self.home.join(".local/share/nit/last-sync.json");
        serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

fn said(o: &Output) -> String {
    format!("{}{}", text(&o.stdout), text(&o.stderr))
}

#[test]
fn the_pull_goes_around_an_edit_it_does_not_touch() {
    let f = Fleet::new();
    f.other_pushes("notes.md", "two\n");
    f.write(".hammerspoon/init.lua", "-- half-finished edit\n");

    let out = f.update();

    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(f.read("notes.md"), "two\n", "the pull came in");
    assert_eq!(
        f.read(".hammerspoon/init.lua"),
        "-- half-finished edit\n",
        "the edit is untouched"
    );
    let st = f.status_json();
    assert_eq!(st["result"], "ok");
    assert_eq!(st["pulled_around"].as_array().unwrap().len(), 1);
    assert!(
        !f.home
            .join(".local/share/nit/pulled-around/.hammerspoon/init.lua")
            .exists(),
        "the safety copy goes once the edit is shown untouched"
    );
}

#[test]
fn the_pull_stops_for_an_edit_it_would_change() {
    let f = Fleet::new();
    f.other_pushes("notes.md", "two\n");
    f.write("notes.md", "my own edit\n");

    let out = f.update();

    assert!(!out.status.success(), "{}", said(&out));
    assert!(
        said(&out).contains("would change these edited files"),
        "{}",
        said(&out)
    );
    assert_eq!(f.read("notes.md"), "my own edit\n");
    let st = f.status_json();
    assert_eq!(st["result"], "aborted_drift");
    assert_eq!(st["drift_files"].as_array().unwrap().len(), 1);
}

#[test]
fn a_staged_edit_stops_a_pull_that_needs_a_merge() {
    let f = Fleet::new();
    f.other_pushes("notes.md", "two\n");
    // A local commit makes the pull a true merge.
    f.write("local.md", "only here\n");
    f.git(&["add", "local.md"]);
    f.git(&["commit", "-q", "-m", "local"]);
    f.write(".hammerspoon/init.lua", "-- staged edit\n");
    f.git(&["add", ".hammerspoon/init.lua"]);

    let out = f.update();

    assert!(!out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("staged"), "{}", said(&out));
    assert_eq!(f.read(".hammerspoon/init.lua"), "-- staged edit\n");
}

#[test]
fn a_staged_edit_rides_through_a_fast_forward() {
    let f = Fleet::new();
    f.other_pushes("notes.md", "two\n");
    f.write(".hammerspoon/init.lua", "-- staged edit\n");
    f.git(&["add", ".hammerspoon/init.lua"]);

    let out = f.update();

    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(f.read("notes.md"), "two\n");
    assert_eq!(f.read(".hammerspoon/init.lua"), "-- staged edit\n");
}

#[test]
fn without_an_upstream_any_edit_still_blocks() {
    let f = Fleet::new();
    f.git(&["branch", "--unset-upstream"]);
    f.write(".hammerspoon/init.lua", "-- edit\n");

    let out = f.update();

    assert!(!out.status.success(), "{}", said(&out));
    assert!(
        said(&out).contains("could not tell what the pull"),
        "{}",
        said(&out)
    );
}

// ─── always_commit: Fredrik's standing ruling, committed by the nightly (2026-10-18) ───

const ALWAYS: &str =
    "\n[sync]\nalways_commit = [\n  \".hammerspoon/init.lua\",   # app settings\n]\n";

#[test]
fn the_nightly_commits_an_edited_always_commit_file_and_still_pulls() {
    let f = Fleet::with_sync(ALWAYS);
    f.other_pushes("notes.md", "two\n");
    f.write(".hammerspoon/init.lua", "-- the app rewrote its settings\n");

    let out = f.update();

    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(f.read("notes.md"), "two\n", "the pull came in");
    assert_eq!(
        f.read(".hammerspoon/init.lua"),
        "-- the app rewrote its settings\n"
    );
    let committed = f.git(&["show", "HEAD:.hammerspoon/init.lua"]);
    assert_eq!(
        committed, "-- the app rewrote its settings\n",
        "the edit is in git"
    );
    let log = f.git(&["log", "--format=%s", "-3"]);
    assert!(log.contains("always_commit"), "{log}");
    assert!(
        f.git(&["status", "--porcelain", "--untracked-files=no"])
            .trim()
            .is_empty(),
        "nothing tracked is left edited"
    );
}

#[test]
fn a_file_not_on_the_list_is_not_committed() {
    let f = Fleet::with_sync(ALWAYS);
    f.write("notes.md", "a session's half-finished edit\n");

    let out = f.update();

    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(f.git(&["show", "HEAD:notes.md"]), "one\n");
    assert!(!f.git(&["log", "--format=%s"]).contains("always_commit"));
}

#[test]
fn a_staged_always_commit_file_is_left_to_the_session_staging_it() {
    let f = Fleet::with_sync(ALWAYS);
    f.write(".hammerspoon/init.lua", "-- staged by a session\n");
    f.git(&["add", ".hammerspoon/init.lua"]);

    let out = f.update();

    assert!(out.status.success(), "{}", said(&out));
    assert!(!f.git(&["log", "--format=%s"]).contains("always_commit"));
    assert!(
        f.git(&["diff", "--cached", "--name-only"])
            .contains(".hammerspoon/init.lua")
    );
}

#[test]
fn another_sessions_staged_file_stays_out_of_the_nightly_commit() {
    let f = Fleet::with_sync(ALWAYS);
    f.write("notes.md", "staged by another session\n");
    f.git(&["add", "notes.md"]);
    f.write(".hammerspoon/init.lua", "-- the app rewrote its settings\n");

    let out = f.update();

    assert!(out.status.success(), "{}", said(&out));
    let files = f.git(&["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(
        files.trim(),
        ".hammerspoon/init.lua",
        "only the listed file"
    );
    assert!(
        f.git(&["diff", "--cached", "--name-only"])
            .contains("notes.md"),
        "still staged"
    );
}
