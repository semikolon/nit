//! Pull gate — may `nit update` pull while tracked files are edited?
//!
//! Until 2026-10-15 the answer was "never": any modified or deleted tracked
//! file aborted the nightly sync before `git pull` (`164513c3`). That kept the
//! sacred promise (nit never clobbers a local edit) but it also meant one
//! session's unfinished edit to one doc stopped the whole machine from
//! syncing. On the Mac the nightly sat deadlocked for three days on three
//! files, none of which the pull would have touched.
//!
//! The promise is about the files, not about pulling. Git keeps it on its own
//! for most of the cases, which was measured in scratch repositories before
//! this module was written:
//!
//! * A pull whose incoming commits change an edited file refuses
//!   ("Your local changes … would be overwritten by merge"). Nothing is lost,
//!   but the sync fails, so the gate still blocks that case itself and says
//!   which files, rather than leaving it to a git error.
//! * An edited file the pull does not touch is left exactly as it was, in a
//!   fast-forward and in a true merge alike.
//! * STAGED changes are different: a true merge (local commits on both sides)
//!   refuses when anything at all is staged, even files it does not touch. A
//!   fast-forward carries the staged changes through. So staged entries block
//!   only when the pull cannot fast-forward.
//!
//! When git's view of what is coming cannot be had (no upstream, the fetch
//! failed) the gate falls back to the old rule and blocks on any edit. Not
//! knowing is never read as "nothing is coming".
//!
//! Ruled by Fredrik 2026-10-15 ("Sensible, go ahead"). Design:
//! `docs/forward_only_drift_steward_design_2026-07-11.md` § 10.4.

use crate::config::GitStrategy;
use crate::drift_triage::{parse_porcelain_entry, root_pathspec};
use crate::git;
use std::collections::BTreeSet;

/// What the pull would bring in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    /// Every path the incoming commits add, change or remove (both sides of a
    /// rename, since a rename removes the old path from the worktree).
    pub paths: Vec<String>,
    /// True when HEAD is an ancestor of the upstream, so the pull is a
    /// fast-forward and creates no merge commit.
    pub fast_forward: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// The incoming commits change files that are edited here.
    WouldOverwrite,
    /// Something is staged and the pull needs a true merge, which git refuses
    /// with a non-empty index.
    StagedBlocksMerge,
    /// What is coming could not be determined, so the old rule applies.
    IncomingUnknown,
}

impl BlockReason {
    pub fn explain(&self) -> &'static str {
        match self {
            BlockReason::WouldOverwrite => {
                "the pull would change these edited files, and git would refuse to overwrite them"
            }
            BlockReason::StagedBlocksMerge => {
                "these files are staged and the pull needs a merge, which git refuses with anything staged"
            }
            BlockReason::IncomingUnknown => {
                "could not tell what the pull would bring in (no upstream, or the fetch failed), so any edit blocks it"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullGate {
    /// Pull. `pulled_around` are the edited files left as they are.
    Proceed { pulled_around: Vec<String> },
    /// Do not pull. `files` are the porcelain lines that block it.
    Block {
        reason: BlockReason,
        files: Vec<String>,
    },
}

/// Both paths of a porcelain line: the destination, and for a rename also
/// the source, since the incoming side may touch either.
fn porcelain_paths(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some((_, dest)) = parse_porcelain_entry(line) {
        out.push(dest);
    }
    if let Some((src, _)) = line.get(3..).and_then(|r| r.trim().split_once(" -> ")) {
        out.push(src.trim_matches('"').to_string());
    }
    out
}

/// Is anything staged on this porcelain line? The first column is the index.
fn is_staged(line: &str) -> bool {
    matches!(line.chars().next(), Some(c) if c != ' ' && c != '?' && c != '!')
}

/// The decision itself, with no IO. `drift` is every `git status --porcelain`
/// line for a modified, deleted or staged tracked file.
pub fn decide(drift: &[String], incoming: Option<&Incoming>) -> PullGate {
    if drift.is_empty() {
        return PullGate::Proceed {
            pulled_around: Vec::new(),
        };
    }
    let incoming = match incoming {
        Some(i) => i,
        None => {
            return PullGate::Block {
                reason: BlockReason::IncomingUnknown,
                files: drift.to_vec(),
            };
        }
    };
    let coming: BTreeSet<&str> = incoming.paths.iter().map(String::as_str).collect();
    let overlap: Vec<String> = drift
        .iter()
        .filter(|l| {
            porcelain_paths(l)
                .iter()
                .any(|p| coming.contains(p.as_str()))
        })
        .cloned()
        .collect();
    if !overlap.is_empty() {
        return PullGate::Block {
            reason: BlockReason::WouldOverwrite,
            files: overlap,
        };
    }
    if !incoming.paths.is_empty() && !incoming.fast_forward {
        let staged: Vec<String> = drift.iter().filter(|l| is_staged(l)).cloned().collect();
        if !staged.is_empty() {
            return PullGate::Block {
                reason: BlockReason::StagedBlocksMerge,
                files: staged,
            };
        }
    }
    PullGate::Proceed {
        pulled_around: drift.to_vec(),
    }
}

/// Split NUL-separated `git diff -z --name-only` output.
pub fn parse_name_list(out: &str) -> Vec<String> {
    out.split('\0')
        .map(|s| s.trim_matches('\n'))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Fetch, then ask git what the pull would bring. `None` whenever any step
/// fails: the caller must treat that as "unknown", never as "nothing".
pub fn incoming(strategy: &GitStrategy) -> Option<Incoming> {
    git::git_output_with(strategy, &["fetch", "--quiet"]).ok()?;
    let upstream = git::git_output_with(
        strategy,
        &["rev-parse", "--verify", "--quiet", "@{upstream}"],
    )
    .ok()?
    .trim()
    .to_string();
    if upstream.is_empty() {
        return None;
    }
    // Three dots: what the upstream changed since the merge base, which is
    // exactly what a merge (or fast-forward) writes into the worktree.
    let range = "HEAD...@{upstream}".to_string();
    let names = git::git_output_with(
        strategy,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            &range,
            "--",
            &root_pathspec(""),
        ],
    )
    .ok()?;
    let fast_forward = git::exec_git_with(
        strategy,
        &["merge-base", "--is-ancestor", "HEAD", "@{upstream}"],
    )
    .ok()?
    .success();
    Some(Incoming {
        paths: parse_name_list(&names),
        fast_forward,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn inc(paths: &[&str], ff: bool) -> Incoming {
        Incoming {
            paths: s(paths),
            fast_forward: ff,
        }
    }

    #[test]
    fn no_edits_proceeds_with_nothing_pulled_around() {
        assert_eq!(
            decide(&[], None),
            PullGate::Proceed {
                pulled_around: vec![]
            }
        );
    }

    #[test]
    fn unknown_incoming_falls_back_to_blocking_on_any_edit() {
        let drift = s(&[" M dotfiles/docs/connections.md"]);
        match decide(&drift, None) {
            PullGate::Block { reason, files } => {
                assert_eq!(reason, BlockReason::IncomingUnknown);
                assert_eq!(files, drift);
            }
            other => panic!("expected block, got {:?}", other),
        }
    }

    // The 2026-10-15 Mac deadlock, as it stood: three edited files, a pull
    // that touched none of them.
    #[test]
    fn unrelated_edits_are_pulled_around() {
        let drift = s(&[
            " M .claude/glossary/deliberus.md",
            " M .hammerspoon/init.lua",
            " M dotfiles/docs/connections.md",
        ]);
        let incoming = inc(&["dotfiles/TODO.md", ".claude/hooks/x.py"], true);
        assert_eq!(
            decide(&drift, Some(&incoming)),
            PullGate::Proceed {
                pulled_around: drift.clone()
            }
        );
    }

    #[test]
    fn an_edited_file_the_pull_changes_blocks_and_only_it_is_named() {
        let drift = s(&[" M .zshrc", " M dotfiles/TODO.md"]);
        let incoming = inc(&["dotfiles/TODO.md"], true);
        assert_eq!(
            decide(&drift, Some(&incoming)),
            PullGate::Block {
                reason: BlockReason::WouldOverwrite,
                files: s(&[" M dotfiles/TODO.md"])
            }
        );
    }

    #[test]
    fn a_locally_deleted_file_the_pull_changes_blocks() {
        let drift = s(&[" D .zshrc"]);
        let incoming = inc(&[".zshrc"], true);
        assert!(matches!(
            decide(&drift, Some(&incoming)),
            PullGate::Block {
                reason: BlockReason::WouldOverwrite,
                ..
            }
        ));
    }

    #[test]
    fn a_rename_blocks_when_the_pull_touches_its_old_path() {
        let drift = s(&["R  old/name.md -> new/name.md"]);
        let incoming = inc(&["old/name.md"], true);
        assert!(matches!(
            decide(&drift, Some(&incoming)),
            PullGate::Block {
                reason: BlockReason::WouldOverwrite,
                ..
            }
        ));
    }

    // Measured 2026-10-15: a true merge refuses with ANY staged entry, even
    // one it does not touch; a fast-forward carries it through.
    #[test]
    fn staged_entries_block_a_true_merge() {
        let drift = s(&[
            "MM dotfiles/docs/connections.md",
            " M .hammerspoon/init.lua",
        ]);
        let incoming = inc(&["dotfiles/TODO.md"], false);
        assert_eq!(
            decide(&drift, Some(&incoming)),
            PullGate::Block {
                reason: BlockReason::StagedBlocksMerge,
                files: s(&["MM dotfiles/docs/connections.md"])
            }
        );
    }

    #[test]
    fn staged_entries_ride_through_a_fast_forward() {
        let drift = s(&["MM dotfiles/docs/connections.md"]);
        let incoming = inc(&["dotfiles/TODO.md"], true);
        assert!(matches!(
            decide(&drift, Some(&incoming)),
            PullGate::Proceed { .. }
        ));
    }

    #[test]
    fn staged_entries_do_not_block_when_nothing_is_coming() {
        let drift = s(&["M  dotfiles/docs/connections.md"]);
        let incoming = inc(&[], false);
        assert!(matches!(
            decide(&drift, Some(&incoming)),
            PullGate::Proceed { .. }
        ));
    }

    #[test]
    fn a_path_that_merely_shares_a_prefix_is_not_an_overlap() {
        let drift = s(&[" M dotfiles/TODO.md.bak"]);
        let incoming = inc(&["dotfiles/TODO.md"], true);
        assert!(matches!(
            decide(&drift, Some(&incoming)),
            PullGate::Proceed { .. }
        ));
    }

    #[test]
    fn name_list_parses_nul_separated_output() {
        assert_eq!(
            parse_name_list("a.md\0dir/b c.txt\0"),
            s(&["a.md", "dir/b c.txt"])
        );
        assert!(parse_name_list("").is_empty());
    }
}
