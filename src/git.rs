//! Bare git repo helpers and fall-through execution
//!
//! Supports two strategies:
//! - **Bare** (default): ~/.local/share/nit/repo.git with work tree = $HOME
//! - **Home**: ~/.git as a regular repo (GIT_CEILING_DIRECTORIES prevents walk-up)

use crate::config::GitStrategy;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

/// Get git-dir for Strategy B (bare repo)
pub fn bare_git_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".local/share/nit/repo.git")
}

/// Get git-dir for Strategy A (home dir)
#[allow(dead_code)]
pub fn home_git_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".git")
}

/// Work tree is always $HOME (for both strategies)
pub fn work_tree() -> PathBuf {
    dirs::home_dir().expect("cannot determine home directory")
}

/// Get the git-dir for a given strategy
#[allow(dead_code)]
pub fn git_dir_for(strategy: &GitStrategy) -> PathBuf {
    match strategy {
        GitStrategy::Bare => bare_git_dir(),
        GitStrategy::Home => home_git_dir(),
    }
}

/// Build a git Command with the right --git-dir/--work-tree flags
pub(crate) fn git_command(strategy: &GitStrategy) -> Command {
    let mut cmd = Command::new("git");
    match strategy {
        GitStrategy::Bare => {
            cmd.arg("--git-dir")
                .arg(bare_git_dir())
                .arg("--work-tree")
                .arg(work_tree());
        }
        GitStrategy::Home => {
            // Strategy A: git is in $HOME, but we set ceiling to prevent walk-up
            cmd.env("GIT_CEILING_DIRECTORIES", work_tree());
        }
    }
    cmd
}

/// Execute a git command with proper flags for the given strategy
pub fn exec_git_with(strategy: &GitStrategy, args: &[&str]) -> Result<ExitStatus, std::io::Error> {
    git_command(strategy).args(args).status()
}

/// Run a git command and fail when git does.
///
/// `exec_git_with` hands back the exit status, and a caller that writes
/// `exec_git_with(..)?` has only handled "git could not be started". On
/// 2026-10-11 that reported a commit git had refused as "committed 18
/// path(s)". Use this wherever the next line assumes git did what was asked.
pub fn run_git_with(
    strategy: &GitStrategy,
    args: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let status = exec_git_with(strategy, args)?;
    check_status(
        args.first().copied().unwrap_or("git"),
        status.code(),
        status.success(),
    )
}

/// The verdict half of `run_git_with`, pure so the refusal is testable.
fn check_status(
    subcommand: &str,
    code: Option<i32>,
    success: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if success {
        return Ok(());
    }
    let how = match code {
        Some(c) => format!("exit {c}"),
        None => "ended by a signal".to_string(),
    };
    Err(format!("git {subcommand} failed ({how})").into())
}

/// Execute a git command with Strategy B (bare) — used when no config loaded yet
#[allow(dead_code)]
pub fn exec_git(args: &[&str]) -> Result<ExitStatus, std::io::Error> {
    exec_git_with(&GitStrategy::Bare, args)
}

/// Execute a git command and capture stdout
pub fn git_output_with(
    strategy: &GitStrategy,
    args: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let output = git_command(strategy).args(args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {}: {}", args.join(" "), stderr).into());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Execute a git command and capture stdout using default bare strategy
#[allow(dead_code)]
pub fn git_output(args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    git_output_with(&GitStrategy::Bare, args)
}

/// Fall through to git for unrecognized subcommands (default bare strategy)
#[allow(dead_code)]
pub fn fall_through(args: &[String]) -> ! {
    fall_through_with(&GitStrategy::Bare, args)
}

/// Fall through to git for unrecognized subcommands with specific strategy
pub fn fall_through_with(strategy: &GitStrategy, args: &[String]) -> ! {
    let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    // Since 2026-10-18 each session stages into its own index
    // (`crate::session`). A fall-through that stages (`rm`, `mv`, `restore`,
    // path-form `reset`) runs against that index and records what it changed;
    // `diff --cached` shows it. Everything else, and anything while a merge is
    // under way (git keeps the merge in the shared index), uses the shared one.
    let merging =
        crate::git::git_output_with(strategy, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .is_ok();
    if !merging {
        let session = crate::session::Session::current();
        match session_route(&str_args) {
            Route::Stage => {
                session.adopt_old_staging(strategy);
                let code = match session.stage(strategy, &str_args) {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("nit: {e}");
                        1
                    }
                };
                std::process::exit(code);
            }
            Route::ReadStaged => {
                session.adopt_old_staging(strategy);
                if session.staged(strategy).is_empty() {
                    std::process::exit(0); // this session has staged nothing
                }
                let status = git_command(strategy)
                    .env("GIT_INDEX_FILE", session.index_path())
                    .args(&str_args)
                    .status()
                    .unwrap_or_else(|e| {
                        eprintln!("nit: failed to exec git: {}", e);
                        std::process::exit(1);
                    });
                std::process::exit(status.code().unwrap_or(1));
            }
            Route::Shared => {}
        }
    }

    let status = git_command(strategy)
        .args(&str_args)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("nit: failed to exec git: {}", e);
            std::process::exit(1);
        });

    std::process::exit(status.code().unwrap_or(1));
}

/// Which index a fall-through git command works on.
#[derive(Debug, PartialEq)]
enum Route {
    /// Writes staging: this session's index.
    Stage,
    /// Shows staging (`diff --cached`): this session's index.
    ReadStaged,
    Shared,
}

fn session_route(args: &[&str]) -> Route {
    let Some(sub) = args.first() else {
        return Route::Shared;
    };
    let rest = &args[1..];
    match *sub {
        "rm" | "mv" | "restore" => Route::Stage,
        "reset" => {
            const MODES: [&str; 5] = ["--soft", "--hard", "--mixed", "--keep", "--merge"];
            let moves_head = rest.iter().any(|a| MODES.contains(a));
            let names_paths = rest.contains(&"--")
                || rest
                    .iter()
                    .any(|a| !a.starts_with('-') && std::path::Path::new(a).exists());
            if !moves_head && names_paths {
                Route::Stage
            } else {
                Route::Shared
            }
        }
        "diff" if rest.iter().any(|a| *a == "--cached" || *a == "--staged") => Route::ReadStaged,
        _ => Route::Shared,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_commands_go_to_the_session_and_the_rest_to_the_shared_index() {
        assert_eq!(session_route(&["rm", "--cached", "x"]), Route::Stage);
        assert_eq!(session_route(&["mv", "a", "b"]), Route::Stage);
        assert_eq!(session_route(&["reset", "--", "x"]), Route::Stage);
        assert_eq!(session_route(&["reset", "--soft", "HEAD~1"]), Route::Shared);
        assert_eq!(session_route(&["reset", "--hard"]), Route::Shared);
        assert_eq!(
            session_route(&["diff", "--cached", "--stat"]),
            Route::ReadStaged
        );
        assert_eq!(session_route(&["diff", "HEAD~1"]), Route::Shared);
        assert_eq!(session_route(&["log", "-3"]), Route::Shared);
    }

    #[test]
    fn a_git_that_exits_non_zero_is_an_error_and_names_the_command() {
        assert!(check_status("commit", Some(0), true).is_ok());
        let refused = check_status("commit", Some(128), false)
            .unwrap_err()
            .to_string();
        assert_eq!(refused, "git commit failed (exit 128)");
        let killed = check_status("add", None, false).unwrap_err().to_string();
        assert_eq!(killed, "git add failed (ended by a signal)");
    }

    #[test]
    fn test_bare_git_dir_location() {
        let dir = bare_git_dir();
        let home = dirs::home_dir().unwrap();
        assert_eq!(dir, home.join(".local/share/nit/repo.git"));
    }

    #[test]
    fn test_home_git_dir_location() {
        let dir = home_git_dir();
        let home = dirs::home_dir().unwrap();
        assert_eq!(dir, home.join(".git"));
    }

    #[test]
    fn test_work_tree_is_home() {
        let wt = work_tree();
        let home = dirs::home_dir().unwrap();
        assert_eq!(wt, home);
    }

    #[test]
    fn test_git_dir_for_strategy() {
        let bare = git_dir_for(&GitStrategy::Bare);
        let home_strategy = git_dir_for(&GitStrategy::Home);

        // Bare repo is NOT in $HOME directly
        assert!(bare.to_string_lossy().contains(".local/share/nit/repo.git"));
        // Home strategy IS at $HOME/.git
        assert!(home_strategy.to_string_lossy().ends_with("/.git"));

        // They should be different paths
        assert_ne!(bare, home_strategy);
    }

    #[test]
    fn test_git_command_bare_has_flags() {
        // Verify the command builder includes --git-dir and --work-tree for bare strategy
        let cmd = git_command(&GitStrategy::Bare);
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert!(args.contains(&std::ffi::OsStr::new("--git-dir")));
        assert!(args.contains(&std::ffi::OsStr::new("--work-tree")));
    }

    #[test]
    fn test_git_command_home_has_ceiling() {
        // Home strategy sets GIT_CEILING_DIRECTORIES env var
        let cmd = git_command(&GitStrategy::Home);
        let envs: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> = cmd.get_envs().collect();
        let has_ceiling = envs.iter().any(|(k, _)| k == &"GIT_CEILING_DIRECTORIES");
        assert!(has_ceiling);

        // Should NOT have --git-dir flag
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert!(!args.contains(&std::ffi::OsStr::new("--git-dir")));
    }
}
