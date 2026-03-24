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
pub fn git_dir_for(strategy: &GitStrategy) -> PathBuf {
    match strategy {
        GitStrategy::Bare => bare_git_dir(),
        GitStrategy::Home => home_git_dir(),
    }
}

/// Build a git Command with the right --git-dir/--work-tree flags
fn git_command(strategy: &GitStrategy) -> Command {
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

/// Execute a git command with Strategy B (bare) — used when no config loaded yet
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
pub fn git_output(args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    git_output_with(&GitStrategy::Bare, args)
}

/// Fall through to git for unrecognized subcommands (default bare strategy)
pub fn fall_through(args: &[String]) -> ! {
    fall_through_with(&GitStrategy::Bare, args)
}

/// Fall through to git for unrecognized subcommands with specific strategy
pub fn fall_through_with(strategy: &GitStrategy, args: &[String]) -> ! {
    let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let status = git_command(strategy)
        .args(&str_args)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("nit: failed to exec git: {}", e);
            std::process::exit(1);
        });

    std::process::exit(status.code().unwrap_or(1));
}
