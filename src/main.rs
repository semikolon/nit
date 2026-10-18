//! nit — AI-era dotfiles manager
//!
//! git's rivet. Bare git for 547 plain files (edit in place),
//! selective tera templates (~10 files) + age encryption (4 files)
//! + hash triggers (19 scripts).

mod bootstrap;
mod config;
mod drift_queue;
mod drift_triage;
mod encrypt;
mod git;
mod permissions;
mod pick;
mod pull_gate;
mod session;
mod sync_status;
mod syncbase;
mod template;
mod trigger;

use clap::{Parser, Subcommand};
use config::NitConfig;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "nit",
    about = "AI-era dotfiles manager — git's rivet",
    // Include git SHA from build.rs so `nit --version` shows the installed
    // commit. The rebuild-nit trigger parses this output to decide whether
    // to rebuild against the SHA pinned in `dotfiles/.nit-version`.
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("NIT_GIT_SHA"), ")"),
    // Don't error on unknown subcommands — they fall through to git
    allow_external_subcommands = true,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<NitCommand>,
}

#[derive(Subcommand)]
enum NitCommand {
    /// Stage files (smart path resolution for template targets)
    Add {
        /// Files to stage (template targets redirect to source)
        #[arg(required = true)]
        paths: Vec<String>,
    },

    /// Render + deploy templates locally (no commit)
    Apply {
        /// Specific file to apply (default: all)
        file: Option<String>,
        /// Override the secrets drift check — clobber unflushed manual edits
        /// to ~/.secrets/tier-*.env. Only use when you've consciously decided
        /// to discard the target-side changes (e.g., the edit was a mistake
        /// or has already been incorporated by other means).
        #[arg(long)]
        force_drift: bool,
    },

    /// Proactive drift review ("nitpick" your templates)
    Pick {
        /// Specific file to review
        file: Option<String>,

        /// Dismiss saved drift (shows diff before removing)
        #[arg(long)]
        dismiss: bool,

        /// Print drift as a unified diff to stdout (read-only).
        /// Pipe to `git apply` or `patch` against the source template if
        /// you've reviewed the change and want to apply it manually:
        ///   nit pick --diff .zshenv | (cd ~/dotfiles && git apply -p1)
        /// Caveat: only safe to apply when the drift hunks don't touch
        /// template syntax ({{ ... }} / {% ... %}); see `--edit` for an
        /// editor-assisted workflow that handles arbitrary templates.
        #[arg(long)]
        diff: bool,

        /// Open the template source in $EDITOR with the drift shown
        /// inline beforehand (in the terminal scrollback). The editor
        /// edit is on the source TEMPLATE, not the rendered target —
        /// you incorporate the desired drift into the right
        /// conditional branch by hand. After saving and exiting the
        /// editor, run `nit commit` to deploy.
        #[arg(long)]
        edit: bool,
    },

    /// Commit what THIS session staged with `nit add` (templates in it are
    /// rendered and deployed, triggers run). Paths other sessions staged are
    /// left staged and named. During a merge it concludes the merge.
    Commit {
        /// Commit message. Repeatable like `git commit -m … -m …` —
        /// multiple values become paragraphs joined by a blank line.
        #[arg(short, long)]
        message: Vec<String>,

        /// Read the commit message from a file, or from stdin if `-`
        /// (mirrors `git commit -F`). Mutually exclusive with -m.
        #[arg(short = 'F', long = "file")]
        file: Option<String>,

        /// Fold this session's staged paths into the last commit, or with
        /// nothing staged by this session only reword it. Without -m/-F the
        /// message is kept. Refused once the commit is on the remote.
        #[arg(long)]
        amend: bool,

        /// Commit only these of the paths this session staged, after `--`
        /// (`nit commit -m … -- <path>…`). The rest stay staged and stay this
        /// session's. Lets a tool that writes one file commit just that file
        /// from inside a session with other work staged.
        #[arg(last = true)]
        paths: Vec<String>,
    },

    /// Pull + render + deploy + triggers (fleet sync, no commit)
    Update {
        /// Skip service-restarting triggers
        #[arg(long)]
        safe: bool,
    },

    /// One-line summary: template drift, triggers, git status
    Status {
        /// Also list untracked files (heavy scan — bare repo defaults to
        /// status.showuntrackedfiles=no because $HOME-as-work-tree would
        /// produce thousands of paths). Useful when you want to know what's
        /// stage-able beyond already-tracked files.
        #[arg(long)]
        show_untracked: bool,

        /// Show staged + modified file paths after the summary line (paths
        /// only; untracked is gated separately by --show-untracked because
        /// it requires a heavy scan). Without this flag, status stays the
        /// terse summary that hemma's status aggregator depends on.
        #[arg(short, long)]
        verbose: bool,
    },

    /// Push to remote (passthrough — saves typing
    /// `git --git-dir=$HOME/.local/share/nit/repo.git --work-tree=$HOME push`,
    /// the classic escape-hatch signature flagged by the global
    /// "Aesthetic-as-decision" directive).
    Push {
        /// Args passed through to `git push` (e.g. `--force-with-lease`, `origin master`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Log (passthrough to `git log` against the nit-managed bare repo).
    Log {
        /// Args passed through to `git log` (e.g. `--oneline -10`, `master..HEAD`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Diff (passthrough to `git diff` against the nit-managed bare repo).
    Diff {
        /// Args passed through to `git diff` (e.g. `--stat`, `HEAD~1`, `--cached`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Move / rename tracked files (passthrough to `git mv`).
    Mv {
        /// Args passed through to `git mv` (e.g. `old/path new/path`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Remove tracked files (passthrough to `git rm`).
    Rm {
        /// Args passed through to `git rm` (e.g. `--cached file`, `-rf dir`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Show a specific commit / object (passthrough to `git show`).
    Show {
        /// Args passed through to `git show` (e.g. `HEAD`, `<commit-sha>`, `--stat HEAD`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Unstage tracked files (passthrough to `git reset`).
    /// Most common: `nit reset <path>` to unstage a path. WARNING: `--hard`
    /// drops working-tree changes — use only when you know what you're doing.
    Reset {
        /// Args passed through to `git reset` (e.g. `<path>`, `HEAD <path>`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Inspect HEAD/ref movement history (passthrough to `git reflog`).
    /// Read-only forensic tool — use for cross-session reconciliation when a
    /// concurrent agent's commit moved the branch tip unexpectedly.
    Reflog {
        /// Args passed through to `git reflog` (e.g. `-15`, `show <ref>`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// List/inspect branches (passthrough to `git branch`).
    /// Common: `nit branch -r --contains <sha>` to confirm a commit reached
    /// the remote. WARNING: `-D` deletes a branch — use deliberately.
    Branch {
        /// Args passed through to `git branch` (e.g. `-r --contains <sha>`, `-a`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Clone bare repo + configure + initial deploy
    Bootstrap {
        /// Repository URL to clone
        url: String,
    },

    /// Encrypt a file with age (add to secrets)
    Encrypt {
        /// File to encrypt
        file: String,
    },

    /// Decrypt a secret to stdout (inspect, no deploy)
    Decrypt {
        /// Encrypted file to decrypt
        file: String,
    },

    /// Re-encrypt all secrets with current fleet.toml recipients
    Rekey,

    /// Inventory: templates, secrets, triggers with status
    List,

    /// Manually run a trigger (ignores hash state)
    Run {
        /// Trigger name
        name: String,
    },

    /// Output fleet inventory (for hemma integration)
    Fleet,

    /// Add an age pubkey to a tier's recipients list in fleet.toml.
    /// Preserves comments + formatting via toml_edit. After this, run
    /// `nit rekey` to re-encrypt all .age files for the new recipient set.
    #[command(name = "fleet-add-recipient")]
    FleetAddRecipient {
        /// Tier name (e.g., "tier-all", "tier-mac", "tier-servers", "tier-edge")
        tier: String,
        /// age public key (starts with "age1")
        pubkey: String,
        /// Optional inline comment (typically the machine name)
        #[arg(long)]
        comment: Option<String>,
    },

    /// Write the drift queue (~/.local/share/nit/drift-queue.json): every
    /// edited tracked file, whether its bytes are already in git, and which
    /// agent sessions touched it since its last commit. No judgement; a
    /// session reads the queue and decides. Also runs after `nit update`.
    DriftTriage {
        /// Leave this file out of the queue while its content stays as it is.
        #[arg(long)]
        dismiss: Vec<String>,
        /// Write the queue without printing it.
        #[arg(long)]
        quiet: bool,
    },

    /// Restore edited tracked files to HEAD, but only when nothing is lost:
    /// the file's current bytes already exist in an earlier commit (a stale
    /// copy), or it was deleted. Refuses new content and anything staged.
    Discard { paths: Vec<String> },

    /// Print this session's private staging index, current and seeded if new,
    /// for git commands nit does not wrap:
    /// `GIT_INDEX_FILE=$(nit session-index) git apply --cached fix.patch`.
    /// What is staged on it that way is this session's at the next `nit commit`.
    SessionIndex {},

    /// Any unrecognized subcommand falls through to git
    #[command(external_subcommand)]
    Git(Vec<String>),
}

/// Pure: if `argv` names a *pure-passthrough* git subcommand (verbatim
/// args, no nit-specific flags), return `(subcommand, &argv[2..])`. `main`
/// routes these to git BEFORE `Cli::parse()` so the `--` pathspec separator
/// survives verbatim — clap's `trailing_var_arg` strips the first `--`,
/// yielding a FALSE-EMPTY pathspec scope (`nit log -- <path>` →
/// `git log <path>`), which is exactly the scope-verification blindness of
/// incident sharp-edge #5. Defined nit subcommands (status, commit, add, …)
/// keep clap parsing (they have real nit flags).
fn passthrough_subcommand(argv: &[String]) -> Option<(&str, &[String])> {
    const PASSTHROUGH: &[&str] = &[
        "log", "diff", "show", "push", "mv", "rm", "reset", "reflog", "branch",
    ];
    let name = argv.get(1)?.as_str();
    if PASSTHROUGH.contains(&name) {
        Some((name, &argv[2..]))
    } else {
        None
    }
}

fn main() -> ExitCode {
    // Pure-passthrough subcommands: route to git BEFORE clap so the `--`
    // pathspec separator survives verbatim (incident sharp-edge #5 — clap's
    // trailing_var_arg strips the first `--`). Mirrors the external-subcommand
    // strategy resolution. `fall_through_with` execs git and exits (`!`).
    let argv: Vec<String> = std::env::args().collect();
    if let Some((sub, rest)) = passthrough_subcommand(&argv) {
        let strategy = config::load_config()
            .map(|c| c.local.git.strategy.clone())
            .unwrap_or(config::GitStrategy::Bare);
        let mut gitargs: Vec<String> = Vec::with_capacity(rest.len() + 1);
        gitargs.push(sub.to_string());
        gitargs.extend_from_slice(rest);
        git::fall_through_with(&strategy, &gitargs);
    }

    let cli = Cli::parse();

    match cli.command {
        Some(NitCommand::Git(args)) => {
            // Git fall-through doesn't need config — try loading for strategy,
            // fall back to bare if config not available
            let strategy = config::load_config()
                .map(|c| c.local.git.strategy.clone())
                .unwrap_or(config::GitStrategy::Bare);
            git::fall_through_with(&strategy, &args);
        }
        Some(NitCommand::Bootstrap { url }) => {
            // Bootstrap doesn't need existing config (it creates it)
            match cmd_bootstrap(&url) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("nit: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(NitCommand::Fleet) => {
            // Fleet only needs fleet.toml, not local.toml
            match cmd_fleet() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("nit: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(cmd) => {
            // All other commands need config
            let config = match config::load_config() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            };
            match run_command(cmd, &config) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("nit: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        None => {
            // No subcommand — show status as default
            let config = match config::load_config() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            };
            // Default-no-args path uses the lightweight summary (matches
            // pre-flag behavior — `nit` alone stays terse and fast).
            match cmd_status(&config, false, false) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("nit: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn run_command(cmd: NitCommand, config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        NitCommand::Add { paths } => cmd_add(&paths, config),
        NitCommand::Apply { file, force_drift } => cmd_apply(file.as_deref(), force_drift, config),
        NitCommand::Pick {
            file,
            dismiss,
            diff,
            edit,
        } => cmd_pick(file.as_deref(), dismiss, diff, edit, config),
        NitCommand::Commit {
            message,
            file,
            amend,
            paths,
        } => cmd_commit(&message, file.as_deref(), amend, &paths, config),
        NitCommand::Update { safe } => {
            let result = cmd_update(safe, config);
            // Whatever the update did, leave the drift steward an up-to-date
            // queue. Its own failure must not change the update's result.
            drift_queue::refresh(config);
            result
        }
        NitCommand::Status {
            show_untracked,
            verbose,
        } => cmd_status(config, show_untracked, verbose),
        NitCommand::Push { args } => cmd_passthrough("push", &args, config),
        NitCommand::Log { args } => cmd_passthrough("log", &args, config),
        NitCommand::Diff { args } => cmd_passthrough("diff", &args, config),
        NitCommand::Mv { args } => cmd_passthrough("mv", &args, config),
        NitCommand::Rm { args } => cmd_passthrough("rm", &args, config),
        NitCommand::Show { args } => cmd_passthrough("show", &args, config),
        NitCommand::Reset { args } => cmd_passthrough("reset", &args, config),
        NitCommand::Reflog { args } => cmd_passthrough("reflog", &args, config),
        NitCommand::Branch { args } => cmd_passthrough("branch", &args, config),
        NitCommand::Encrypt { file } => cmd_encrypt(&file, config),
        NitCommand::Decrypt { file } => cmd_decrypt(&file, config),
        NitCommand::Rekey => cmd_rekey(config),
        NitCommand::List => cmd_list(config),
        NitCommand::Run { name } => cmd_run(&name, config),
        NitCommand::DriftTriage { dismiss, quiet } => drift_queue::cmd(config, &dismiss, quiet),
        NitCommand::Discard { paths } => cmd_discard(&paths, config),
        NitCommand::SessionIndex {} => {
            let path = session::Session::current().ready_index(config.git_strategy())?;
            println!("{}", path.display());
            Ok(())
        }
        NitCommand::FleetAddRecipient {
            tier,
            pubkey,
            comment,
        } => cmd_fleet_add_recipient(&tier, &pubkey, comment.as_deref(), config),
        // Bootstrap, Fleet, and Git handled in main()
        NitCommand::Bootstrap { .. } | NitCommand::Fleet | NitCommand::Git(_) => {
            unreachable!()
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a path string to an absolute PathBuf
fn resolve_path(path_str: &str) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    resolve_path_with(path_str, &cwd, &home)
}

/// Path-resolution core, parameterized for testing.
///
/// nit's git work-tree IS `$HOME`, so a relative path that the user types may
/// be intended either cwd-relative (the default git/CLI convention) or
/// `$HOME`-relative (the work-tree convention). Pure cwd-relative resolution
/// breaks the common workflow of running `nit add .claude/CLAUDE.md` from a
/// subdirectory like `~/dotfiles` (cwd-relative would resolve to the
/// nonexistent `~/dotfiles/.claude/CLAUDE.md`).
///
/// Resolution order:
///   1. Tilde-expansion (`~/foo`) — explicit home-relative
///   2. Absolute path — passthrough
///   3. Relative + cwd-relative form exists on disk — prefer cwd
///   4. Relative + cwd-relative missing + `$HOME`-relative form exists — fall
///      back to `$HOME`-relative (rescues the common subdir workflow)
///   5. Relative + neither exists — return cwd-relative form so any
///      downstream "no such file" error is contextual to the user's invocation
fn resolve_path_with(path_str: &str, cwd: &Path, home: &Path) -> PathBuf {
    let path = Path::new(path_str);

    // Handle tilde
    if path_str.starts_with("~/") || path_str == "~" {
        return config::expand_tilde(path_str);
    }

    // Absolute path: passthrough
    if path.is_absolute() {
        return path.to_path_buf();
    }

    // Relative: try cwd first, fall back to $HOME if cwd-form doesn't exist.
    let cwd_relative = cwd.join(path);
    if cwd_relative.exists() {
        return cwd_relative;
    }
    let home_relative = home.join(path);
    if home_relative.exists() {
        return home_relative;
    }
    // Neither exists — return cwd-relative form for contextual error message.
    cwd_relative
}

/// Compute the relative target path (strip $HOME prefix) used as key in sync-base/acks.
fn target_rel_path(target: &Path) -> String {
    let home = dirs::home_dir().expect("cannot determine home directory");
    target
        .strip_prefix(&home)
        .unwrap_or(target)
        .to_string_lossy()
        .to_string()
}

/// Whether a template target carries a genuine local edit to reconcile.
///
/// Drift matters only when the target differs from BOTH the recorded sync-base
/// AND the freshly-rendered source. If `render == target` the source has caught
/// up to the local edit (e.g. the edit was incorporated into the template, or a
/// prior source-wins deploy already wrote it), so writing the render is a no-op
/// — there is nothing to preserve or clobber. Requiring `target != render` (not
/// just the historical `base != target`) is what lets a resolved drift self-heal
/// instead of phantom-reporting forever and skipping the file every nightly run.
/// Safety: when this returns false, the only target the deploy can change is one
/// that equals the sync-base (a normal update, never a local edit), or one that
/// already equals the render (a no-op) — the never-clobber guarantee is intact.
fn is_real_drift(base: Option<&str>, target: Option<&str>, render: &str) -> bool {
    matches!((base, target), (Some(b), Some(t)) if b != t && t != render)
}

/// Whether deploying `render` over the target could lose something nobody committed:
/// an edit made since nit last wrote the file (real drift), or the content of a file
/// nit never wrote, about to be replaced. Only then does a commit ask the session to
/// look first (2026-10-18). A target equal to what nit last wrote, or already equal to
/// the render, holds nothing to lose.
fn target_at_risk(base: Option<&str>, target: Option<&str>, render: &str) -> bool {
    match (base, target) {
        (_, None) => false,
        (Some(_), Some(_)) => is_real_drift(base, target, render),
        (None, Some(t)) => t != render,
    }
}

/// Prepend a warning comment to rendered content before writing to target.
/// Returns the content unchanged if no comment is appropriate (e.g., JSON).
fn prepend_warning(rendered: &str, target: &Path) -> String {
    if let Some(comment) = template::warning_comment(target) {
        format!("{}\n{}", comment, rendered)
    } else {
        rendered.to_string()
    }
}

/// Deploy a single rendered template to its target.
/// Writes the rendered content (with warning comment) to the target path.
/// Creates parent directories as needed.
fn write_target(target: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, content)?;
    Ok(())
}

/// Summarise git status as (staged_count, modified_count)
fn git_status_counts(config: &NitConfig) -> (usize, usize) {
    let strategy = config.git_strategy();
    let git_status = git::git_output_with(strategy, &["status", "--porcelain"]).unwrap_or_default();
    let staged = git_status
        .lines()
        .filter(|l| {
            let first = l.chars().next().unwrap_or(' ');
            first != ' ' && first != '?'
        })
        .count();
    let modified = git_status
        .lines()
        .filter(|l| l.starts_with(" M") || l.starts_with("M "))
        .count();
    (staged, modified)
}

/// Default log dir for triggers
fn default_log_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".local/share/nit/logs")
}

// ---------------------------------------------------------------------------
// T-3: Smart add with template target detection
// ---------------------------------------------------------------------------

fn cmd_add(paths: &[String], config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let strategy = config.git_strategy();

    // Each session stages into its own index (`session`), except while a
    // merge is under way: git keeps the merge in the shared index.
    let merging = merge_in_progress(strategy);
    let session = session::Session::current();
    if !merging {
        session.adopt_old_staging(strategy);
    }
    let stage = |args: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
        if merging {
            git::run_git_with(strategy, args)
        } else {
            session.stage(strategy, args)
        }
    };

    // Discover templates and build reverse lookup
    let mappings = template::discover_templates(config);
    let target_to_source = template::build_target_to_source_map(&mappings);

    let mut git_add_paths: Vec<PathBuf> = Vec::new();
    let mut template_redirects: Vec<(PathBuf, PathBuf)> = Vec::new(); // (target, source)

    for path_str in paths {
        // Handle "." and "-A" as bulk operations
        if path_str == "." || path_str == "-A" {
            // Stage all modified tracked files via git
            let added = stage(&["add", path_str]);

            // Scan all templates for drift awareness + write acks
            if added.is_ok() && !mappings.is_empty() {
                eprintln!("nit: scanning {} templates for drift...", mappings.len());
                for mapping in &mappings {
                    report_template_drift(mapping, config);
                }
            }
            return added;
        }

        // Resolve the path
        let resolved = resolve_path(path_str);

        // Check if this is a template target
        if let Some(source) = template::resolve_template_target(&resolved, &target_to_source) {
            template_redirects.push((resolved, source));
        } else {
            git_add_paths.push(resolved);
        }
    }

    // What the caller named, as pathspecs git resolves itself: the plain
    // paths, and for a template target the source it redirects to.
    let mut named: Vec<String> = git_add_paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    // The first thing git refused. Staging goes on past it and the record is
    // still written, so a partial `nit add` leaves nothing staged unrecorded.
    let mut refused: Option<Box<dyn std::error::Error>> = None;

    // Stage plain files via git add
    if !named.is_empty() {
        let mut args = vec!["add"];
        args.extend(named.iter().map(String::as_str));
        if let Err(e) = stage(&args) {
            refused = Some(e);
        }
    }

    // Handle template target redirects
    for (target, source) in &template_redirects {
        eprintln!(
            "nit: {} is a template target → staging source {}",
            target.display(),
            source.display()
        );

        // Stage the template source instead
        let source_str = source.to_string_lossy().to_string();
        if let Err(e) = stage(&["add", &source_str]) {
            refused.get_or_insert(e);
            continue;
        }
        named.push(source_str);

        // Write ack for this template target
        if target.exists() {
            // Find the mapping for this target
            if let Some(mapping) = mappings.iter().find(|m| m.target == *target) {
                write_ack_for_mapping(mapping, config);
            }
            eprintln!(
                "nit: drift check for {} (full review with `nit pick`)",
                target.display()
            );
        }
    }

    match refused {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Report template drift for a mapping and write ack
fn report_template_drift(mapping: &template::TemplateMapping, config: &NitConfig) {
    if mapping.target.exists() {
        write_ack_for_mapping(mapping, config);
        eprintln!(
            "  {} → {}",
            mapping.rel_source.display(),
            mapping.target.display()
        );
    }
}

/// Write an ack entry for a template mapping (current target hash + rendered hash)
fn write_ack_for_mapping(mapping: &template::TemplateMapping, config: &NitConfig) {
    let rel = target_rel_path(&mapping.target);
    let target_content = std::fs::read_to_string(&mapping.target).unwrap_or_default();
    let target_hash = syncbase::hash_content(&target_content);
    let rendered_hash = match template::render_template(mapping, config) {
        Ok(rendered) => {
            let with_comment = prepend_warning(&rendered, &mapping.target);
            syncbase::hash_content(&with_comment)
        }
        Err(_) => syncbase::hash_content(""),
    };
    syncbase::write_ack(&rel, &target_hash, &rendered_hash);
}

// ---------------------------------------------------------------------------
// T-5: cmd_apply — Render + deploy (NO commit)
//
// Note on the `force_drift` parameter: when false (default), `deploy_secrets`
// aborts the deploy of any tier-*.env file whose target diverges from the
// source-decrypt — protecting against the silent-overwrite failure mode
// documented in `~/.claude/CLAUDE.md` § "Secrets editing — `nit encrypt` is
// part of the same edit, not a follow-up step" (May 4, 2026 MANDATORY).
// When true, drift is logged and the deploy proceeds anyway.
// ---------------------------------------------------------------------------

fn cmd_apply(
    file: Option<&str>,
    force_drift: bool,
    config: &NitConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mappings = template::discover_templates(config);
    let mappings_to_process: Vec<&template::TemplateMapping> = if let Some(file_filter) = file {
        let filter_path = resolve_path(file_filter);
        mappings
            .iter()
            .filter(|m| {
                m.target == filter_path
                    || m.source == filter_path
                    || m.rel_source.to_string_lossy() == file_filter
            })
            .collect()
    } else {
        mappings.iter().collect()
    };

    if mappings_to_process.is_empty() {
        if let Some(f) = file {
            return Err(format!("no template matching '{}'", f).into());
        }
        eprintln!("nit apply: no templates found");
        return Ok(());
    }

    let mut drifted_rels: Vec<String> = Vec::new();
    let mut deployed_count: usize = 0;
    let mut error_count: usize = 0;

    for mapping in &mappings_to_process {
        let rel = target_rel_path(&mapping.target);

        // 1. Render template
        let rendered = match template::render_template(mapping, config) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "nit: ERROR rendering {}: {}",
                    mapping.rel_source.display(),
                    e
                );
                error_count += 1;
                continue;
            }
        };

        // 6. Prepend warning comment
        let rendered_with_comment = prepend_warning(&rendered, &mapping.target);

        // 2. Read sync-base
        let base_content = syncbase::read_sync_base(&rel);
        // 3. Read target
        let target_content = std::fs::read_to_string(&mapping.target).ok();

        let has_drift = is_real_drift(
            base_content.as_deref(),
            target_content.as_deref(),
            &rendered_with_comment,
        );

        if has_drift {
            // 5. base != target: save drift, deploy source-wins, update sync-base, SKIP triggers
            let drift_diff = syncbase::detect_drift(&rel, target_content.as_deref().unwrap_or(""));
            if let Some(diff) = &drift_diff {
                syncbase::save_drift(&rel, diff);
            }
            write_target(&mapping.target, &rendered_with_comment)?;
            syncbase::write_sync_base(&rel, &rendered_with_comment);
            drifted_rels.push(rel.clone());
            eprintln!(
                "nit: \u{26a0} Drift overwritten in {} — review with nit pick",
                rel
            );
        } else {
            // 4. No drift (or no base): deploy rendered, update sync-base
            write_target(&mapping.target, &rendered_with_comment)?;
            syncbase::write_sync_base(&rel, &rendered_with_comment);
            // No real drift here by definition (target == sync-base). Any
            // saved .diff for this rel is therefore stale — e.g. a drift
            // earlier resolved by editing the template source. Clear it so
            // it can't deadlock `nit status` / phantom-report forever.
            syncbase::clear_drift(&rel);
        }

        deployed_count += 1;

        // 9. Write ack for this template
        let target_hash = syncbase::hash_content(&rendered_with_comment);
        let rendered_hash = syncbase::hash_content(&rendered_with_comment);
        syncbase::write_ack(&rel, &target_hash, &rendered_hash);
    }

    // 7. Decrypt secrets (with drift-check unless --force-drift)
    let mut secrets_drift_count = 0usize;
    match encrypt::deploy_secrets(config, force_drift) {
        Ok(results) => {
            for r in &results {
                match &r.status {
                    encrypt::DeployStatus::Deployed => {
                        eprintln!("nit: secret {} → {}", r.tier, r.target);
                    }
                    encrypt::DeployStatus::Skipped(reason) => {
                        eprintln!("nit: secret {} skipped: {}", r.tier, reason);
                    }
                    encrypt::DeployStatus::Error(e) => {
                        eprintln!("nit: secret {} ERROR: {}", r.tier, e);
                    }
                    encrypt::DeployStatus::DriftDetected {
                        target_bytes,
                        source_bytes,
                        classification,
                    } => {
                        secrets_drift_count += 1;
                        eprintln!(
                            "nit: secret {} DRIFT: {} (target {}B, source-decrypt {}B) — {}",
                            r.tier,
                            r.target,
                            target_bytes,
                            source_bytes,
                            encrypt::drift_guidance(classification, &r.target)
                        );
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("nit: warning: secret deployment failed: {}", e);
        }
    }
    if secrets_drift_count > 0 {
        return Err(format!(
            "{} secret tier(s) have unflushed target edits — apply aborted to prevent data loss",
            secrets_drift_count
        )
        .into());
    }

    // 8. Run applicable triggers (skip drifted files)
    let log_dir = default_log_dir();
    let mut trigger_state = trigger::load_trigger_state();
    let trigger_results = trigger::run_applicable_triggers(
        config,
        &mut trigger_state,
        &drifted_rels,
        false,
        &log_dir,
    );
    trigger::save_trigger_state(&trigger_state);

    for tr in &trigger_results {
        match &tr.status {
            trigger::RunStatus::Success => {
                eprintln!("nit: trigger '{}' succeeded", tr.name);
            }
            trigger::RunStatus::Failed(code) => {
                eprintln!(
                    "nit: trigger '{}' failed (exit {}), log: {}",
                    tr.name,
                    code,
                    tr.log_path.display()
                );
            }
            trigger::RunStatus::Skipped(reason) => {
                eprintln!("nit: trigger '{}' skipped: {}", tr.name, reason);
            }
        }
    }

    eprintln!(
        "nit apply: {} deployed, {} errors, {} drifted",
        deployed_count,
        error_count,
        drifted_rels.len()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// T-9: cmd_pick — Proactive drift review
// ---------------------------------------------------------------------------

fn cmd_pick(
    file: Option<&str>,
    dismiss: bool,
    diff_only: bool,
    edit: bool,
    config: &NitConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mappings = template::discover_templates(config);

    // --dismiss mode
    if dismiss {
        let file_arg = file.ok_or("nit pick --dismiss requires a file argument")?;
        let rel = resolve_pick_target(file_arg, &mappings);
        let diff = syncbase::dismiss_drift(&rel)?;
        println!("Dismissed drift for {}:", rel);
        println!();
        for line in diff.lines() {
            println!("    {}", line);
        }
        // Write ack
        if let Some(mapping) = mappings.iter().find(|m| target_rel_path(&m.target) == rel) {
            write_ack_for_mapping(mapping, config);
        }
        println!();
        println!("Drift removed.");
        return Ok(());
    }

    // --diff mode: print drift as unified diff to stdout, no other output.
    // Read-only — does NOT write ack (the user hasn't reviewed yet, just
    // extracted the diff). Pipe-friendly for chaining with patch / git apply.
    if diff_only {
        let file_arg = file.ok_or("nit pick --diff requires a file argument")?;
        let rel = resolve_pick_target(file_arg, &mappings);
        let mapping = mappings
            .iter()
            .find(|m| target_rel_path(&m.target) == rel)
            .ok_or_else(|| format!("no template found for target '{}'", rel))?;
        let diff = syncbase::read_drift(&rel)
            .or_else(|| detect_live_drift(mapping, config))
            .ok_or_else(|| format!("no drift detected for {}", rel))?;
        // Print raw diff to stdout — no decoration, suitable for piping
        print!("{}", diff);
        if !diff.ends_with('\n') {
            println!();
        }
        return Ok(());
    }

    // --edit mode: print drift to stderr (visible in terminal scrollback),
    // then spawn $EDITOR on the template SOURCE. User incorporates the
    // desired drift into the right branch/conditional by hand. After the
    // editor exits, the user runs `nit commit` to deploy. Writes ack since
    // the user is actively reviewing.
    if edit {
        let file_arg = file.ok_or("nit pick --edit requires a file argument")?;
        let rel = resolve_pick_target(file_arg, &mappings);
        let mapping = mappings
            .iter()
            .find(|m| target_rel_path(&m.target) == rel)
            .ok_or_else(|| format!("no template found for target '{}'", rel))?;
        let drift = syncbase::read_drift(&rel)
            .or_else(|| detect_live_drift(mapping, config))
            .ok_or_else(|| format!("no drift detected for {}", rel))?;

        // Show drift on stderr (so it scrolls past as the editor opens)
        eprintln!();
        eprintln!("  Drift in {} (rendered target vs current target):", rel);
        eprintln!("  Opening template source in $EDITOR. Incorporate desired changes by hand.");
        eprintln!();
        for line in drift.lines() {
            eprintln!("    {}", line);
        }
        eprintln!();
        eprintln!("  Source: {}", mapping.source.display());
        eprintln!();

        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| "vi".to_string());
        let status = std::process::Command::new(&editor)
            .arg(&mapping.source)
            .status()
            .map_err(|e| format!("failed to launch editor '{}': {}", editor, e))?;
        if !status.success() {
            return Err(format!("editor '{}' exited with status {}", editor, status).into());
        }

        // Write ack — user actively reviewed
        write_ack_for_mapping(mapping, config);
        eprintln!("  Edit complete. Run `nit commit` to deploy.");
        return Ok(());
    }

    // Determine which mappings to review
    let mappings_to_review: Vec<&template::TemplateMapping> = if let Some(file_arg) = file {
        let rel = resolve_pick_target(file_arg, &mappings);
        mappings
            .iter()
            .filter(|m| target_rel_path(&m.target) == rel)
            .collect()
    } else {
        mappings.iter().collect()
    };

    let total = mappings_to_review.len();

    // Collect drifted templates
    let mut drifted: Vec<(&template::TemplateMapping, String)> = Vec::new();
    let mut clean_count: usize = 0;

    for mapping in &mappings_to_review {
        let rel = target_rel_path(&mapping.target);
        // Check for saved drift OR live drift
        if let Some(diff) = syncbase::read_drift(&rel) {
            drifted.push((mapping, diff));
        } else if let Some(diff) = detect_live_drift(mapping, config) {
            drifted.push((mapping, diff));
        } else {
            clean_count += 1;
        }
        // Write ack for every reviewed template
        write_ack_for_mapping(mapping, config);
    }

    // Output per spec
    if drifted.is_empty() {
        // Happy path
        println!();
        println!("All {} templates clean. No drift.", total);
    } else {
        // Warnings FIRST
        println!();
        println!("  \u{26a0} Drift is NEVER auto-merged. Source always wins on deploy.");
        println!("  Actions for each drifted file:");
        println!(
            "    \u{2192} Do nothing:          source wins on next nit commit (drift saved, recoverable)"
        );
        println!(
            "    \u{2192} Edit template source: incorporate changes you want, then nit commit"
        );
        println!(
            "    \u{2192} nit pick --dismiss:  acknowledge as junk, remove from drift permanently"
        );
        println!("  If drift is a valuable fix, edit the template source or it will be");
        println!("  overwritten (but always recoverable via nit pick).");
        println!();
        println!(
            "\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}"
        );
        println!();
        println!("Drift in {} of {} templates:", drifted.len(), total);
        println!();

        for (mapping, diff) in &drifted {
            let rel = target_rel_path(&mapping.target);
            println!("  {} — target has content not in template source:", rel);
            for line in diff.lines() {
                println!("    {}", line);
            }
            println!();
        }

        if clean_count > 0 {
            println!("{} templates clean.", clean_count);
        }
    }

    // Git status footer
    let (staged, modified) = git_status_counts(config);
    print!("Git status: {} staged, {} modified", staged, modified);
    if staged > 0 {
        print!(" — ready to commit");
    }
    println!(".");
    println!("Pick token written \u{2713}");

    Ok(())
}

/// Resolve a pick file argument to a target_rel path
fn resolve_pick_target(file_arg: &str, mappings: &[template::TemplateMapping]) -> String {
    // Try as-is (might be a rel path like ".zshenv")
    for m in mappings {
        let rel = target_rel_path(&m.target);
        if rel == file_arg {
            return rel;
        }
    }
    // Try resolving as a full path
    let resolved = resolve_path(file_arg);
    for m in mappings {
        if m.target == resolved || m.source == resolved {
            return target_rel_path(&m.target);
        }
    }
    // Fall back to the argument as-is
    file_arg.to_string()
}

/// Detect live drift (sync-base vs current target) for a mapping
fn detect_live_drift(mapping: &template::TemplateMapping, _config: &NitConfig) -> Option<String> {
    let rel = target_rel_path(&mapping.target);
    let target_content = std::fs::read_to_string(&mapping.target).ok()?;
    syncbase::detect_drift(&rel, &target_content)
}

// ---------------------------------------------------------------------------
// T-10: cmd_commit — Render + deploy + ack gate + git commit + triggers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Session-intent commit scoping (2026-05-17 keystone)
//
// Pure decision function — no git, no $HOME, no I/O — so the entire keystone
// contract is RED-GREEN testable in isolation (mirrors the codebase pure-fn
// idiom: resolve_path_with, add_recipient_to_toml).
//
// CONTRACT: a `nit commit` includes ONLY paths THIS session-anchor recorded
// via `nit add` (`session_staged`), intersected with what is still staged in
// the shared index (`index_staged`). A concurrent session's `nit add` entries
// live in the shared index but are absent from THIS session's record, so they
// are never committed and their templates are never rendered/deployed. This
// degrades the concurrent-session race from catastrophe (cross-workstream
// bundle + live-deploy of an in-flight template) to a benign, reversible,
// correctly-scoped local commit. Honors AC-5.2 (ack-gate keys off staged
// template sources — now session-scoped), AC-5.7 (plain-only → zero friction,
// even when another session staged/modified a template), and EARS:43 (a
// commit still renders+deploys its templates — but ONLY its own).
// ---------------------------------------------------------------------------

/// A path as the user or a tool wrote it, as the work-tree-relative name the
/// index uses: an absolute path under $HOME loses the prefix, a leading `./`
/// or `:/` goes.
fn normalize_commit_path(p: &str, home: &Path) -> String {
    let path = Path::new(p);
    let rel = path
        .strip_prefix(home)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string());
    rel.trim_start_matches(":/")
        .trim_start_matches("./")
        .to_string()
}

/// Pure: resolve the final commit message from `-m`/`-F` precedence
/// (mirrors `git commit`: `-m` and `-F` are mutually exclusive). `loaded`
/// is the content the caller already read for the `-F` source ("-" = stdin,
/// else a file). Trailing newline trimmed; empty → error (git rejects empty
/// messages). No `-m`/`-F` → the historical "nit commit" default.
fn resolve_commit_message(
    messages: &[String],
    file_arg: Option<&str>,
    loaded: Option<&str>,
) -> Result<String, String> {
    match (messages.is_empty(), file_arg) {
        (false, Some(_)) => {
            Err("cannot combine -m/--message and -F/--file (mirrors git commit)".to_string())
        }
        // git semantics: each -m is a paragraph, joined by a blank line.
        (false, None) => Ok(messages.join("\n\n")),
        (true, Some(_)) => {
            let content = loaded.ok_or_else(|| "could not read -F message source".to_string())?;
            let trimmed = content.trim_end_matches('\n');
            if trimmed.trim().is_empty() {
                Err("empty commit message (-F source had no content)".to_string())
            } else {
                Ok(trimmed.to_string())
            }
        }
        (true, None) => Ok("nit commit".to_string()),
    }
}

/// Pure: the argument list for the final `git commit`.
///
/// - A merge under way takes the whole index. git refuses a pathspec there
///   ("cannot do a partial commit during a merge"), so none is passed.
/// - No message (`None`) keeps the one git already holds: the prepared merge
///   message, or the amended commit's own.
/// - No pathspec outside a merge is `--only`, which git accepts solely with
///   `--amend` and reads as "reword, and leave what is staged out of it". So
///   an empty scope can never turn into a bare `git commit` of the whole
///   index: without `--amend` git refuses the call.
fn commit_git_args(
    message: Option<&str>,
    amend: bool,
    concluding_merge: bool,
    pathspecs: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = vec!["commit".to_string()];
    if amend {
        args.push("--amend".to_string());
    }
    match message {
        Some(m) => {
            args.push("-m".to_string());
            args.push(m.to_string());
        }
        None => args.push("--no-edit".to_string()),
    }
    if concluding_merge {
        return args;
    }
    if pathspecs.is_empty() {
        args.push("--only".to_string());
    } else {
        args.push("--".to_string());
        args.extend(pathspecs.iter().cloned());
    }
    args
}

/// Run the final `git commit` and fail when git does. The caller clears the
/// session's staged record only after this returns `Ok`, so a refused commit
/// leaves the index and the record as they were and the same `nit commit`
/// can be run again.
fn run_git_commit(
    strategy: &config::GitStrategy,
    args: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git::run_git_with(strategy, &refs).map_err(|e| {
        format!(
            "{e}. Nothing was committed: what was staged is still staged, and this \
             session's record of it is kept."
        )
        .into()
    })
}

/// Is a merge under way (a `nit update` or pull that stopped on a conflict)?
fn merge_in_progress(strategy: &config::GitStrategy) -> bool {
    git::git_output_with(strategy, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok()
}

/// Is the last commit already on the branch's remote? Read from the local
/// remote-tracking ref, so it is as fresh as the last push or fetch. A branch
/// with no upstream answers no.
fn head_is_on_upstream(strategy: &config::GitStrategy) -> bool {
    git::git_output_with(
        strategy,
        &["merge-base", "--is-ancestor", "HEAD", "@{upstream}"],
    )
    .is_ok()
}

fn cmd_commit(
    messages: &[String],
    file: Option<&str>,
    amend: bool,
    only_paths: &[String],
    config: &NitConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let strategy = config.git_strategy();
    let home_dir = dirs::home_dir().expect("cannot determine home directory");
    let only: Vec<String> = only_paths
        .iter()
        .map(|p| normalize_commit_path(p, &home_dir))
        .collect();
    if !only.is_empty() && amend {
        return Err("`--amend` and a path list do not go together; amend without paths".into());
    }

    // Resolve -m / -F (file or `-`=stdin). IO here; precedence/validation is
    // the pure `resolve_commit_message` (RED-GREEN tested).
    let loaded: Option<String> = match file {
        Some("-") => {
            use std::io::Read;
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| format!("reading commit message from stdin: {e}"))?;
            Some(s)
        }
        Some(path) => Some(
            std::fs::read_to_string(path)
                .map_err(|e| format!("reading commit message file {path}: {e}"))?,
        ),
        None => None,
    };
    // With neither -m nor -F a plain commit keeps the historical "nit commit"
    // message, while a merge or an amend keeps the message git already holds.
    let explicit_msg: Option<String> = if messages.is_empty() && file.is_none() {
        None
    } else {
        Some(resolve_commit_message(messages, file, loaded.as_deref())?)
    };

    // 1. Check what's staged (as real names: see syncbase::parse_nul_paths)
    let index_staged = syncbase::staged_paths(strategy)?;
    let my_anchor = syncbase::get_session_anchor();

    // 1a. A merge under way is concluded, whole. git accepts nothing else, so
    //     the session scope cannot apply; say so and name the size of it.
    if merge_in_progress(strategy) {
        if !only.is_empty() {
            return Err(
                "a merge is under way, and git commits a merge whole: conclude it with \
                 `nit commit` and no path list"
                    .into(),
            );
        }
        if amend {
            return Err(
                "a merge is under way: conclude it with `nit commit` before amending".into(),
            );
        }
        let unmerged = syncbase::unmerged_paths(strategy)?;
        if !unmerged.is_empty() {
            eprintln!("nit: {} path(s) still in conflict:", unmerged.len());
            for p in &unmerged {
                eprintln!("nit:     {}", p);
            }
            return Err(
                "resolve the conflicts, `nit add` the files, then `nit commit` again".into(),
            );
        }
        eprintln!(
            "nit: a merge is under way. git commits a merge whole, so all {} staged \
             path(s) go into it.",
            index_staged.len()
        );
        run_git_commit(
            strategy,
            &commit_git_args(explicit_msg.as_deref(), false, true, &[]),
        )?;
        syncbase::clear_session_staged(my_anchor);
        syncbase::prune_dead_staged();
        eprintln!(
            "nit: merge concluded. Templates and triggers were not run: `nit update` \
             finishes the sync."
        );
        return Ok(());
    }

    // Each session stages into its own index (2026-10-18,
    // docs/session_private_index_design_2026-10-18.md). What this session
    // staged is in that index; the shared index holds only what raw git or
    // the nit before that date staged.
    let session = session::Session::current();
    session.adopt_old_staging(strategy);
    session::prune_ended();
    syncbase::prune_dead_staged();
    let mine = session.staged(strategy);
    let shared_only: Vec<String> = index_staged
        .iter()
        .filter(|p| !mine.contains(p))
        .cloned()
        .collect();

    if amend {
        if head_is_on_upstream(strategy) {
            return Err(
                "the last commit is already on the remote, and amending it would need a \
                 forced push. Make a new commit instead."
                    .into(),
            );
        }
        let head = session::head(strategy);
        if head.is_none() || session.load().last_commit != head {
            let who = git::git_output_with(strategy, &["log", "-1", "--format=%h %s"])
                .unwrap_or_default();
            return Err(format!(
                "the last commit ({}) was not made by this session, so it may not be \
                 amended. Make a new commit instead.",
                who.trim()
            )
            .into());
        }
    }

    let scope: Vec<String> = if only.is_empty() {
        mine.clone()
    } else {
        let missing: Vec<&String> = only.iter().filter(|p| !mine.contains(p)).collect();
        if !missing.is_empty() {
            for p in &missing {
                eprintln!("nit:     {}", p);
            }
            return Err(format!(
                "{} named path(s) are not staged by this session: `nit add` them first",
                missing.len()
            )
            .into());
        }
        only.clone()
    };
    if scope.is_empty() && !amend {
        if !shared_only.is_empty() {
            eprintln!(
                "nit: this session has staged nothing with `nit add`. Staged by another \
                 session, or with raw git ({}):",
                shared_only.len()
            );
            for p in &shared_only {
                eprintln!("nit:     {}", p);
            }
            eprintln!(
                "nit: to commit some of them from here, name them: `nit add <paths>`, \
                 then `nit commit`."
            );
        }
        return Err(
            "nothing staged by this session to commit. Run `nit add <paths>` first.".into(),
        );
    }
    let left: Vec<&String> = mine.iter().filter(|p| !scope.contains(p)).collect();
    if !left.is_empty() {
        eprintln!(
            "nit: {} other path(s) this session staged stay staged (not named):",
            left.len()
        );
        for p in &left {
            eprintln!("nit:     {}", p);
        }
    }
    if !shared_only.is_empty() {
        eprintln!(
            "nit: \u{26a0} {} path(s) staged in the shared index are not in this commit \
             (another session, or raw git):",
            shared_only.len()
        );
        for p in &shared_only {
            eprintln!("nit:     {}", p);
        }
    }
    let msg: Option<&str> = match (explicit_msg.as_deref(), amend) {
        (Some(m), _) => Some(m),
        (None, true) => None,
        (None, false) => Some("nit commit"),
    };

    // Templates whose source this commit carries: acked before, deployed after.
    let mappings = template::discover_templates(config);
    let home = dirs::home_dir().expect("cannot determine home directory");
    let scoped_templates: Vec<&template::TemplateMapping> = mappings
        .iter()
        .filter(|m| {
            let rel = m
                .source
                .strip_prefix(&home)
                .unwrap_or(&m.source)
                .to_string_lossy()
                .to_string();
            scope.contains(&rel)
        })
        .collect();
    let mut drifted_rels: Vec<String> = Vec::new();
    let did = if amend { "amended with" } else { "committed" };

    // 5. The review gate (redesigned 2026-10-18): a commit asks the session to look
    //    first only when the deploy could lose something nobody committed — a hand
    //    edit to the deployed file, or the content of a file nit never wrote. The
    //    session's own source edits are not a reason to stop, and a review counts
    //    for exactly the version of the file it showed. Reviews stay per session
    //    anchor: one session's look never counts for another (decision 2026-04-21).
    let my_acks = syncbase::read_acks_current();

    // Prune dead-anchor ack/staged files (pure housekeeping; only own-anchor
    // state is load-bearing since cross-session reuse was removed).
    syncbase::prune_dead_acks();

    let mut blocked = false;
    let mut block_reasons: Vec<String> = Vec::new();

    for mapping in &scoped_templates {
        let rel = target_rel_path(&mapping.target);
        let rendered = template::render_template(mapping, config)?;
        let rendered_with_comment = prepend_warning(&rendered, &mapping.target);
        let target_content = std::fs::read_to_string(&mapping.target).ok();
        let base_content = syncbase::read_sync_base(&rel);

        if !target_at_risk(
            base_content.as_deref(),
            target_content.as_deref(),
            &rendered_with_comment,
        ) {
            continue;
        }
        let target_text = target_content.unwrap_or_default();
        let current_target_hash = syncbase::hash_content(&target_text);
        if my_acks
            .get(&rel)
            .is_some_and(|ack| ack.target_hash == current_target_hash)
        {
            continue; // this session has seen exactly this version
        }

        match base_content.as_deref() {
            Some(base) => {
                eprintln!(
                    "nit: {} was edited since nit last wrote it; the template would replace these edits:",
                    rel
                );
                for line in syncbase::diff_text(base, &target_text).lines() {
                    eprintln!("    {}", line);
                }
            }
            None => {
                eprintln!(
                    "nit: {} exists and nit never wrote it; the template would replace it (+ lines are in the file now):",
                    rel
                );
                for line in syncbase::diff_text(&rendered_with_comment, &target_text).lines() {
                    eprintln!("    {}", line);
                }
            }
        }
        syncbase::write_ack(
            &rel,
            &current_target_hash,
            &syncbase::hash_content(&rendered_with_comment),
        );
        blocked = true;
        block_reasons.push(format!(
            "{}: shown above — re-run nit commit to deploy the template over it (what it replaces is kept as a diff for `nit pick`)",
            rel
        ));
    }

    if blocked {
        eprintln!("nit: BLOCKED — resolve before committing:");
        for reason in &block_reasons {
            eprintln!("  {}", reason);
        }
        return Err("commit blocked by ack validation".into());
    }

    // The commit: HEAD as it is now plus exactly this session's staged
    // entries, built in a temporary index. No pathspec, so the work tree can
    // never override what was staged, and nothing is deployed before git has
    // accepted the commit.
    let mut commit_args: Vec<String> = vec!["commit".to_string()];
    if amend {
        commit_args.push("--amend".to_string());
    }
    match msg {
        Some(m) => {
            commit_args.push("-m".to_string());
            commit_args.push(m.to_string());
        }
        None => commit_args.push("--no-edit".to_string()),
    }
    let (made, scope) = session.commit(strategy, &scope, &commit_args, amend)?;
    if made.is_none() {
        eprintln!("nit: nothing left to commit: HEAD already holds what this session staged");
        return Ok(());
    }
    let scoped_templates: Vec<&template::TemplateMapping> = scoped_templates
        .into_iter()
        .filter(|m| {
            let rel = m
                .source
                .strip_prefix(&home)
                .unwrap_or(&m.source)
                .to_string_lossy()
                .to_string();
            scope.contains(&rel)
        })
        .collect();

    if scoped_templates.is_empty() {
        eprintln!(
            "nit: {} {} path(s) (plain files only, no templates)",
            did,
            scope.len()
        );
        return Ok(());
    }

    // 6. Committed: deploy ONLY this session's in-scope templates.
    //    EARS:43 honored (a commit renders+deploys its templates) but NEVER
    //    another session's in-flight template — that is the deploy-side-effect
    //    footgun this keystone closes.
    for mapping in &scoped_templates {
        let rel = target_rel_path(&mapping.target);

        let rendered = match template::render_template(mapping, config) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "nit: ERROR rendering {}: {}",
                    mapping.rel_source.display(),
                    e
                );
                continue;
            }
        };

        let rendered_with_comment = prepend_warning(&rendered, &mapping.target);

        // Check for drift (source wins)
        let base_content = syncbase::read_sync_base(&rel);
        let target_content = std::fs::read_to_string(&mapping.target).ok();

        let has_drift = is_real_drift(
            base_content.as_deref(),
            target_content.as_deref(),
            &rendered_with_comment,
        );

        if has_drift {
            let drift_diff = syncbase::detect_drift(&rel, target_content.as_deref().unwrap_or(""));
            if let Some(diff) = &drift_diff {
                syncbase::save_drift(&rel, diff);
            }
            drifted_rels.push(rel.clone());
            eprintln!("nit: \u{26a0} Drift saved for {} — source wins", rel);
        } else if let (None, Some(t)) = (base_content.as_deref(), target_content.as_deref()) {
            if t != rendered_with_comment {
                // A file nit never wrote, replaced now (the gate showed it first):
                // keep what it held, the way a hand edit is kept.
                syncbase::save_drift(&rel, &syncbase::diff_text("", t));
                eprintln!(
                    "nit: {} held content nit never wrote — kept as a diff (`nit pick`)",
                    rel
                );
            } else {
                syncbase::clear_drift(&rel);
            }
        } else {
            // No real drift: clear any stale .diff so a resolved drift (e.g.
            // incorporated into the template source) can't phantom-report forever.
            syncbase::clear_drift(&rel);
        }

        write_target(&mapping.target, &rendered_with_comment)?;
        syncbase::write_sync_base(&rel, &rendered_with_comment);
    }

    // Decrypt secrets (drift-check enforced — commit path has no force escape)
    if let Err(e) = encrypt::deploy_secrets(config, false) {
        eprintln!("nit: warning: secret deployment failed: {}", e);
    }

    // Decrypt secrets (drift-check enforced — commit path has no force escape)
    if let Err(e) = encrypt::deploy_secrets(config, false) {
        eprintln!("nit: warning: secret deployment failed: {}", e);
    }

    // Run triggers (skip drifted files)
    let log_dir = default_log_dir();
    let mut trigger_state = trigger::load_trigger_state();
    let trigger_results = trigger::run_applicable_triggers(
        config,
        &mut trigger_state,
        &drifted_rels,
        false,
        &log_dir,
    );
    trigger::save_trigger_state(&trigger_state);

    for tr in &trigger_results {
        match &tr.status {
            trigger::RunStatus::Success => {
                eprintln!("nit: trigger '{}' succeeded", tr.name);
            }
            trigger::RunStatus::Failed(code) => {
                eprintln!(
                    "nit: trigger '{}' failed (exit {}), log: {}",
                    tr.name,
                    code,
                    tr.log_path.display()
                );
            }
            trigger::RunStatus::Skipped(reason) => {
                eprintln!("nit: trigger '{}' skipped: {}", tr.name, reason);
            }
        }
    }

    eprintln!(
        "nit: {} {} path(s), {} template(s) deployed",
        did,
        scope.len(),
        scoped_templates.len()
    );

    Ok(())
}

/// What `nit discard` may do with one drifted file: restore it, or refuse
/// and say why. Pure, so the refusal rules are tested without git.
#[derive(Debug, PartialEq)]
enum DiscardVerdict {
    Restore(String),
    Refuse(String),
}

fn discard_verdict(status: &str, kind: &drift_triage::DriftKind) -> DiscardVerdict {
    let staged = matches!(status.chars().next(), Some(c) if c != ' ' && c != '?');
    if staged {
        return DiscardVerdict::Refuse(
            "it has staged changes; unstage them first (`nit reset <path>`) so nothing staged is lost"
                .into(),
        );
    }
    match kind {
        drift_triage::DriftKind::Stale { commit, date } => DiscardVerdict::Restore(format!(
            "its content matches {} ({}), so nothing is lost",
            commit, date
        )),
        drift_triage::DriftKind::Deleted => {
            DiscardVerdict::Restore("it was deleted; HEAD's version comes back".into())
        }
        drift_triage::DriftKind::Unique => DiscardVerdict::Refuse(
            "its content was never committed; commit it, or ask whoever made it".into(),
        ),
        drift_triage::DriftKind::Unknown => DiscardVerdict::Refuse(
            "could not prove its content is in git history, so it counts as new".into(),
        ),
    }
}

fn cmd_discard(paths: &[String], config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    if paths.is_empty() {
        return Err("name the files to discard".into());
    }
    let strategy = config.git_strategy();
    let home = dirs::home_dir().expect("cannot determine home directory");
    let rels: Vec<String> = paths
        .iter()
        .map(|p| normalize_commit_path(p, &home))
        .collect();
    let mut args: Vec<String> = vec!["status".into(), "--porcelain".into(), "--".into()];
    args.extend(rels.iter().map(|r| drift_triage::root_pathspec(r)));
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let porcelain = git::git_output_with(strategy, &argv)?;
    let drift = sync_status::detect_pre_pull_drift(&porcelain);
    let triaged = drift_triage::triage(strategy, &drift);
    let mut refused = 0usize;
    for rel in &rels {
        let Some(t) = triaged.iter().find(|t| &t.path == rel) else {
            eprintln!("nit: {} has no local changes; nothing to discard", rel);
            continue;
        };
        match discard_verdict(&t.status, &t.kind) {
            DiscardVerdict::Restore(why) => {
                git::run_git_with(
                    strategy,
                    &["checkout", "HEAD", "--", &drift_triage::root_pathspec(rel)],
                )?;
                eprintln!("nit: restored {} to HEAD: {}", rel, why);
            }
            DiscardVerdict::Refuse(why) => {
                refused += 1;
                eprintln!("nit: NOT discarding {}: {}", rel, why);
            }
        }
    }
    drift_queue::refresh(config);
    if refused > 0 {
        return Err(format!("{} file(s) not discarded", refused).into());
    }
    Ok(())
}

/// Copies of the edited files a pull went around, taken before the pull.
///
/// The pull itself leaves them alone (git guarantees that, see `pull_gate`),
/// but the steps after it (templates, secrets, triggers) are nit's own and
/// are only drift-checked one by one. This guard makes the promise hold for
/// the whole run: the bytes are copied aside first, and any file whose bytes
/// differ afterwards keeps its copy and is reported.
struct PulledAroundGuard {
    dir: PathBuf,
    kept: Vec<(String, Option<Vec<u8>>)>,
}

impl PulledAroundGuard {
    fn dir() -> PathBuf {
        dirs::home_dir()
            .expect("cannot determine home directory")
            .join(".local/share/nit/pulled-around")
    }

    fn take(paths: &[String]) -> Self {
        Self::take_in(&Self::dir(), &dirs::home_dir().expect("home"), paths)
    }

    fn take_in(dir: &Path, home: &Path, paths: &[String]) -> Self {
        let kept: Vec<(String, Option<Vec<u8>>)> = paths
            .iter()
            .map(|p| {
                let bytes = std::fs::read(home.join(p)).ok();
                if let Some(b) = &bytes {
                    let dest = dir.join(p);
                    if let Some(parent) = dest.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&dest, b);
                }
                (p.clone(), bytes)
            })
            .collect();
        PulledAroundGuard {
            dir: dir.to_path_buf(),
            kept,
        }
    }

    fn verify(&self) -> Vec<(String, PathBuf)> {
        self.verify_in(&dirs::home_dir().expect("home"))
    }

    /// The paths whose bytes changed, each with where its edited copy is.
    /// Unchanged files have their copy removed.
    fn verify_in(&self, home: &Path) -> Vec<(String, PathBuf)> {
        let mut changed = Vec::new();
        for (p, before) in &self.kept {
            let now = std::fs::read(home.join(p)).ok();
            let copy = self.dir.join(p);
            if &now == before {
                let _ = std::fs::remove_file(&copy);
            } else if before.is_some() {
                changed.push((p.clone(), copy));
            }
        }
        changed
    }
}

// ---------------------------------------------------------------------------
// T-12: cmd_update — Pull + render + deploy + triggers (fleet sync)
// ---------------------------------------------------------------------------

/// The always_commit paths that `git status --porcelain` shows edited in the
/// work tree and not staged. A staged one belongs to a session that is in the
/// middle of committing it; a deleted one is a decision, not a settings change.
fn always_commit_due(porcelain: &str, list: &[String]) -> Vec<String> {
    porcelain
        .lines()
        .filter_map(|l| {
            let mut c = l.chars();
            let (x, y) = (c.next()?, c.next()?);
            if x != ' ' || y != 'M' {
                return None;
            }
            let path = l.get(3..)?.trim().trim_matches('"');
            list.iter().any(|p| p == path).then(|| path.to_string())
        })
        .collect()
}

fn cmd_update(safe: bool, config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let strategy = config.git_strategy();
    let machine_name = config.machine_name.clone();

    // Carry forward last_success_at from prior status so it survives failures.
    let prior = sync_status::load_status();
    let prior_success = prior
        .as_ref()
        .and_then(|s| s.last_success_at.clone())
        .or_else(|| {
            prior.as_ref().and_then(|s| {
                matches!(s.result, sync_status::SyncResult::Ok).then(|| s.completed_at.clone())
            })
        });

    let mut status = sync_status::SyncStatus::new(machine_name.clone());
    status.last_success_at = prior_success;
    status.last_notified_at = prior.as_ref().and_then(|s| s.last_notified_at.clone());

    // 0a. ALWAYS-COMMIT. Fredrik's standing ruling (2026-10-18): a file on
    // fleet.toml's `[sync] always_commit` list is committed when it has changed,
    // so it is never a question for him. Only the listed paths go in (`--only`),
    // a staged one is left to the session staging it, and a refusal (a hook, a
    // secret) leaves it as an ordinary edit for the pull gate. Not pushed: the
    // next push carries it.
    let always: Vec<String> = config
        .fleet
        .sync
        .as_ref()
        .map(|s| s.always_commit.clone())
        .unwrap_or_default();
    // Held through the pull below, so a session's commit never lands in the
    // middle of it; released right after the pull, before templates and triggers.
    let commit_lock = session::commit_lock().ok();
    if !always.is_empty() {
        let before = git::git_output_with(strategy, &["status", "--porcelain"]).unwrap_or_default();
        let due = always_commit_due(&before, &always);
        if !due.is_empty() {
            let msg = format!(
                "nightly: {} changed on {} (fleet.toml always_commit)",
                due.join(", "),
                machine_name
            );
            let mut args = vec!["commit", "--only", "-q", "-m", msg.as_str(), "--"];
            args.extend(due.iter().map(String::as_str));
            match git::run_git_with(strategy, &args) {
                Ok(()) => eprintln!(
                    "nit update: committed {} always-commit path(s): {}",
                    due.len(),
                    due.join(", ")
                ),
                Err(e) => eprintln!(
                    "nit update: could not commit {} ({e}); left as an edit",
                    due.join(", ")
                ),
            }
        }
    }

    // 0. PRE-PULL DRIFT CHECK — sacred: never clobber local state.
    // If any tracked file is modified/deleted, ABORT and write status. Untracked
    // files are fine (gitignored or genuinely new — won't be touched by pull).
    let porcelain = git::git_output_with(strategy, &["status", "--porcelain"]).unwrap_or_default();
    // `detect_pre_pull_drift` keeps detecting ALL drift (it prevented the
    // 2026-05-04 clobber). Until 2026-10-18 forward-only files were filtered out
    // here; that list is gone, and the pull gate below decides per file.
    let all_drift = sync_status::detect_pre_pull_drift(&porcelain);
    // Since 2026-10-15 edited files block the pull only when the pull would
    // touch them (or a staged entry would stop a true merge). Anything else is
    // pulled around and left exactly as it is. See `pull_gate`.
    let gate = if all_drift.is_empty() {
        pull_gate::PullGate::Proceed {
            pulled_around: Vec::new(),
        }
    } else {
        pull_gate::decide(&all_drift, pull_gate::incoming(strategy).as_ref())
    };
    let (drift, block_reason, pulled_around) = match gate {
        pull_gate::PullGate::Block { reason, files } => (files, Some(reason), Vec::new()),
        pull_gate::PullGate::Proceed { pulled_around } => (Vec::new(), None, pulled_around),
    };
    if !drift.is_empty() {
        status.result = sync_status::SyncResult::AbortedDrift;
        status.drift_files = drift.clone();
        // Say WHY each file is dirty, not merely that it is. A file whose exact
        // bytes are already committed is a stale worktree, not lost work, and it
        // wants the opposite remedy. See `drift_triage`'s module doc for the
        // MERIAN deadlock that made this necessary.
        let triaged = drift_triage::triage(strategy, &drift);
        let now = chrono::Utc::now();
        let stale_days = drift_triage::days_since_rfc3339(status.last_success_at.as_deref(), now);
        let deadlocked = drift_triage::is_deadlock(status.last_success_at.as_deref(), now);

        eprintln!(
            "nit update: ABORTED — pre-pull drift detected ({} file(s)):",
            drift.len()
        );
        if let Some(reason) = &block_reason {
            eprintln!("  why: {}", reason.explain());
            status.errors.push(reason.explain().to_string());
        }
        for f in &triaged {
            eprintln!("  {} {}  [{}]", f.status, f.path, f.kind.label());
        }
        eprintln!();

        if deadlocked {
            let days = stale_days.unwrap_or(0);
            let all_stale =
                !triaged.is_empty() && triaged.iter().all(|f| f.kind.is_safe_to_discard());

            eprintln!(
                "\u{26a0} DEADLOCKED: no successful sync for {} days. This will NOT clear itself.",
                days
            );
            eprintln!("  The abort refuses to pull while tracked files are dirty, and these files");
            eprintln!("  can only become clean BY pulling. Every further night repeats this.");
            if all_stale {
                eprintln!(
                    "  Every file above is a stale copy already in git — discarding loses nothing."
                );
            }
            eprintln!();

            let notify_url = drift_triage::should_notify(status.last_notified_at.as_deref(), now)
                .then(|| {
                    config
                        .fleet
                        .sync
                        .as_ref()
                        .and_then(|s| s.ntfy_url.as_deref())
                })
                .flatten();
            if let Some(url) = notify_url {
                let body = drift_triage::deadlock_message(&machine_name, days, &triaged);
                // Only stamp on a DELIVERED alert: a failed push (a travel
                // machine off the VPN, a token without rights to the topic)
                // must not buy a week of silence.
                if drift_triage::notify_ntfy(url, "nit sync deadlocked", &body) {
                    status.last_notified_at = Some(now.to_rfc3339());
                } else {
                    // Say so. An alert that fails quietly is the same bug as an
                    // abort that fails quietly, one layer up — and this exact
                    // push returned 403 the first time it was wired, because
                    // the token had no rights to a brand-new topic.
                    eprintln!(
                        "  (could not deliver the alert to {} — unreachable, or the ntfy token \
                         lacks publish rights on that topic)",
                        url
                    );
                }
            }
        }

        status.completed_at = chrono::Utc::now().to_rfc3339();
        sync_status::save_status(&status);

        eprintln!("Local edits would be at risk if pull merged. Resolve manually:");
        eprintln!(
            "  - Discard:  git --git-dir={} --work-tree=$HOME checkout -- <file>",
            git::bare_git_dir().display()
        );
        eprintln!("  - Stage:    nit add <file> && nit commit -m \"...\"");
        eprintln!("  - Inspect:  nit diff <file>");
        eprintln!();
        eprintln!("Status written: ~/.local/share/nit/last-sync.json");
        return Err("aborted: pre-pull drift detected".into());
    }

    // 0b. Edited files the pull leaves alone: keep a copy of each first, so
    //     that if any later step (a template, a secret, a trigger) changes one
    //     after all, the edit is still on disk and the change is reported.
    let pulled_around_paths: Vec<String> = pulled_around
        .iter()
        .filter_map(|l| drift_triage::parse_porcelain_entry(l).map(|(_, p)| p))
        .collect();
    let guard = PulledAroundGuard::take(&pulled_around_paths);
    if !pulled_around_paths.is_empty() {
        eprintln!(
            "nit: pulling around {} edited file(s) the pull does not touch:",
            pulled_around_paths.len()
        );
        for p in &pulled_around_paths {
            eprintln!("nit:     {}", p);
        }
    }
    status.pulled_around = pulled_around.clone();

    // 1. git pull
    eprintln!("nit: pulling latest...");
    let pull_status = git::exec_git_with(strategy, &["pull", "--no-rebase", "--no-edit"])?;
    drop(commit_lock);
    if !pull_status.success() {
        status.result = sync_status::SyncResult::PullFailed;
        status.errors.push("git pull failed".to_string());
        status.completed_at = chrono::Utc::now().to_rfc3339();
        sync_status::save_status(&status);
        return Err("git pull failed".into());
    }

    // 2. For each template: deploy if clean, skip if drifted
    let mappings = template::discover_templates(config);
    let mut drifted_rels: Vec<String> = Vec::new();
    let mut deployed_count: usize = 0;
    let mut skipped_count: usize = 0;

    for mapping in &mappings {
        let rel = target_rel_path(&mapping.target);

        let rendered = match template::render_template(mapping, config) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "nit: ERROR rendering {}: {}",
                    mapping.rel_source.display(),
                    e
                );
                continue;
            }
        };

        let rendered_with_comment = prepend_warning(&rendered, &mapping.target);

        let base_content = syncbase::read_sync_base(&rel);
        let target_content = std::fs::read_to_string(&mapping.target).ok();

        let has_drift = is_real_drift(
            base_content.as_deref(),
            target_content.as_deref(),
            &rendered_with_comment,
        );

        if has_drift {
            // nit update special behavior: SKIP drifted files (preserve local fixes)
            let drift_diff = syncbase::detect_drift(&rel, target_content.as_deref().unwrap_or(""));
            if let Some(diff) = &drift_diff {
                syncbase::save_drift(&rel, diff);
            }
            drifted_rels.push(rel.clone());
            skipped_count += 1;
            eprintln!("nit: \u{26a0} Skipped {} — target has local drift", rel);
        } else {
            // No drift: deploy rendered, update sync-base
            write_target(&mapping.target, &rendered_with_comment)?;
            syncbase::write_sync_base(&rel, &rendered_with_comment);
            // Clear any stale .diff (drift resolved out-of-band, e.g. via
            // template-source edit) so it can't phantom-report forever.
            syncbase::clear_drift(&rel);
            deployed_count += 1;
        }
    }

    // 3. Decrypt secrets (drift-check enforced — `nit update` runs nightly via cron;
    // unflushed target edits surface as a hard failure rather than silently disappearing)
    let mut secrets_drift_count = 0usize;
    match encrypt::deploy_secrets(config, false) {
        Ok(results) => {
            for r in &results {
                match &r.status {
                    encrypt::DeployStatus::Deployed => {
                        eprintln!("nit: secret {} → {}", r.tier, r.target);
                    }
                    encrypt::DeployStatus::Skipped(reason) => {
                        eprintln!("nit: secret {} skipped: {}", r.tier, reason);
                    }
                    encrypt::DeployStatus::Error(e) => {
                        eprintln!("nit: secret {} ERROR: {}", r.tier, e);
                    }
                    encrypt::DeployStatus::DriftDetected {
                        target_bytes,
                        source_bytes,
                        classification,
                    } => {
                        secrets_drift_count += 1;
                        eprintln!(
                            "nit: secret {} DRIFT: {} (target {}B, source-decrypt {}B) — {}",
                            r.tier,
                            r.target,
                            target_bytes,
                            source_bytes,
                            encrypt::drift_guidance(classification, &r.target)
                        );
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("nit: warning: secret deployment failed: {}", e);
        }
    }
    if secrets_drift_count > 0 {
        // WRITE THE RECORD. Without this the abort was invisible: `last-sync.json`
        // kept its previous contents, so a machine failing here every night still
        // reported its last SUCCESS — Shannon showed `ok — 2026-06-27` for ten
        // weeks. A health check that reads a stale success is worse than one that
        // reads nothing, because nothing looks wrong.
        status.result = sync_status::SyncResult::SecretsDrift;
        status.errors.push(format!(
            "{} secret tier(s) have unflushed target edits",
            secrets_drift_count
        ));
        status.completed_at = chrono::Utc::now().to_rfc3339();
        sync_status::save_status(&status);
        return Err(format!(
            "{} secret tier(s) have unflushed target edits — update aborted",
            secrets_drift_count
        )
        .into());
    }

    // 4. Run triggers (skip drifted files; --safe skips all)
    let log_dir = default_log_dir();
    let mut trigger_state = trigger::load_trigger_state();
    // A trigger watching a file that is being edited would run on half-done
    // input; skip it like a drifted template, and it runs once the edit lands.
    let mut skip_for_triggers = drifted_rels.clone();
    skip_for_triggers.extend(pulled_around_paths.iter().cloned());
    let trigger_results = trigger::run_applicable_triggers(
        config,
        &mut trigger_state,
        &skip_for_triggers,
        safe,
        &log_dir,
    );
    trigger::save_trigger_state(&trigger_state);

    for tr in &trigger_results {
        match &tr.status {
            trigger::RunStatus::Success => {
                eprintln!("nit: trigger '{}' succeeded", tr.name);
            }
            trigger::RunStatus::Failed(code) => {
                eprintln!(
                    "nit: trigger '{}' failed (exit {}), log: {}",
                    tr.name,
                    code,
                    tr.log_path.display()
                );
            }
            trigger::RunStatus::Skipped(reason) => {
                eprintln!("nit: trigger '{}' skipped: {}", tr.name, reason);
            }
        }
    }

    // 4b. Did anything touch an edited file after all? Then its edit is kept
    //     aside and the run is reported as failed, never as a quiet success.
    for changed in guard.verify() {
        let msg = format!(
            "{} changed during the update; your edited version is kept at {}",
            changed.0,
            changed.1.display()
        );
        eprintln!("nit: \u{26a0} {}", msg);
        status.errors.push(msg);
    }

    // 5. Write final sync status (no git commit — we're pulling others' changes).
    status.templates_deployed = deployed_count;
    status.templates_skipped_drift = skipped_count;
    for tr in &trigger_results {
        match &tr.status {
            trigger::RunStatus::Success => status.triggers_succeeded += 1,
            trigger::RunStatus::Failed(_) => status.triggers_failed += 1,
            trigger::RunStatus::Skipped(_) => {}
        }
    }
    status.result = if status.triggers_failed > 0 {
        sync_status::SyncResult::TriggersFailed
    } else if status
        .errors
        .iter()
        .any(|e| e.contains("changed during the update"))
    {
        sync_status::SyncResult::EditTouched
    } else {
        sync_status::SyncResult::Ok
    };
    status.completed_at = chrono::Utc::now().to_rfc3339();
    if matches!(status.result, sync_status::SyncResult::Ok) {
        status.last_success_at = Some(status.completed_at.clone());
    }
    sync_status::save_status(&status);

    eprintln!(
        "nit update: {} deployed, {} skipped (drift)",
        deployed_count, skipped_count
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_status — One-line summary with drift count
// ---------------------------------------------------------------------------

fn cmd_status(
    config: &NitConfig,
    show_untracked: bool,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let strategy = config.git_strategy();

    // Template drift count
    let mappings = template::discover_templates(config);
    let template_count = mappings.len();
    let drift_count = syncbase::list_drifted_files().len();

    // Git status summary.
    //
    // Bare repo has `status.showuntrackedfiles=no` set by bootstrap (intentional —
    // `$HOME`-as-work-tree would otherwise spew thousands of "untracked" paths from
    // unrelated home subdirectories). When --show-untracked is passed we override
    // for THIS invocation only via `-c status.showUntrackedFiles=normal` + the
    // `--untracked-files=normal` flag on the porcelain call, which together force
    // git to enumerate untracked entries respecting `.gitignore` / `info/exclude`.
    let mut args: Vec<&str> = Vec::new();
    if show_untracked {
        args.push("-c");
        args.push("status.showUntrackedFiles=normal");
    }
    args.push("status");
    args.push("--porcelain");
    if show_untracked {
        args.push("--untracked-files=normal");
    }
    let git_status = git::git_output_with(strategy, &args).unwrap_or_default();
    let is_modified = |l: &str| l.starts_with(" M") || l.starts_with("M ");
    let modified = git_status.lines().filter(|l| is_modified(l)).count();
    let staged = git_status
        .lines()
        .filter(|l| {
            let first = l.chars().next().unwrap_or(' ');
            first != ' ' && first != '?'
        })
        .count();
    let untracked = git_status.lines().filter(|l| l.starts_with("??")).count();

    // Trigger count
    let trigger_count = config.applicable_triggers().len();

    let mine = session::Session::current().staged(strategy).len();
    println!(
        "nit: {} templates ({} drifted), {} triggers | git: {} staged by this session, {} in \
         the shared index, {} modified, {} untracked",
        template_count, drift_count, trigger_count, mine, staged, modified, untracked
    );
    // Last-sync health summary (from ~/.local/share/nit/last-sync.json).
    if let Some(last) = sync_status::load_status() {
        println!("{}", sync_status::one_line_summary(&last));

        // A machine can abort every night for months and still report a calm
        // one-liner, which is exactly how MERIAN went 67 nights and 536 commits
        // unnoticed. Once the abort is old enough to be a deadlock, say so here
        // too — this is the surface a person actually looks at.
        // Any non-Ok result, not just pre-pull drift: a machine stuck on secret
        // drift is exactly as stuck, and that was the one nobody could see.
        if !matches!(last.result, sync_status::SyncResult::Ok) {
            let now = chrono::Utc::now();
            if drift_triage::is_deadlock(last.last_success_at.as_deref(), now) {
                let days = drift_triage::days_since_rfc3339(last.last_success_at.as_deref(), now)
                    .unwrap_or(0);
                println!(
                    "\u{26a0} nit: DEADLOCKED — no successful sync for {} days; drift blocks the \
                     pull and the pull is what would clear it. Run `nit update` to see the \
                     per-file triage.",
                    days
                );
            }
        }
    }

    // Verbose: list staged + modified paths after the summary. Untracked is
    // gated separately by --show-untracked because it requires the heavy
    // scan override; -v alone keeps the call fast.
    if verbose {
        // Anything not a space and not a `?` in the first column is staged
        // (porcelain v1: M /A /D /R /C / first chars). Print the porcelain
        // line so the prefix is visible — matches what users expect from
        // having reached for `git status --short` in the past.
        let staged_lines: Vec<&str> = git_status
            .lines()
            .filter(|l| {
                let first = l.chars().next().unwrap_or(' ');
                first != ' ' && first != '?'
            })
            .collect();
        if !staged_lines.is_empty() {
            println!();
            println!("Staged:");
            for line in &staged_lines {
                println!("  {}", line);
            }
        }

        // Unstaged-modified: second column is M / D / etc., first column is space.
        let modified_lines: Vec<&str> = git_status
            .lines()
            .filter(|l| l.starts_with(" "))
            .filter(|l| !l.starts_with("  ") && !l.starts_with("??"))
            .collect();
        if !modified_lines.is_empty() {
            println!();
            println!("Modified (run `nit add <path>` to stage):");
            for line in &modified_lines {
                println!("  {}", line);
            }
        }

        // What `nit commit` would take: this session's own staging (its
        // private index since 2026-10-18), and what the shared index holds
        // that is not this session's.
        let session = session::Session::current();
        let mine = session.staged(strategy);
        let shared = syncbase::staged_paths(strategy).unwrap_or_default();
        let others: Vec<&String> = shared.iter().filter(|p| !mine.contains(p)).collect();
        if !mine.is_empty() || !others.is_empty() {
            println!();
            println!("Commit scope (this session): {} path(s)", mine.len());
            for p in &mine {
                println!("  + {}", p);
            }
            if !others.is_empty() {
                println!(
                    "  ({} path(s) staged in the shared index are not this session's: raw \
                     git, or the nit before 2026-10-18. `nit add <paths>` claims yours)",
                    others.len()
                );
            }
        }
    }

    // Heavy-scan detail: print the untracked paths so the user knows what's
    // stage-able. Hint to `nit add <path>` so the next step is obvious.
    if show_untracked && untracked > 0 {
        println!();
        println!("Untracked (run `nit add <path>` to track):");
        for line in git_status.lines().filter(|l| l.starts_with("??")) {
            // Porcelain `?? <path>` — slice off the "?? " prefix.
            if let Some(rest) = line.strip_prefix("?? ") {
                println!("  {}", rest);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_passthrough — Generic pass-through for git subcommands without nit equivalents
// ---------------------------------------------------------------------------
//
// Why this exists: nit wraps a bare git repo at ~/.local/share/nit/repo.git
// with $HOME as work-tree. Any raw `git <cmd>` against this repo requires
// `--git-dir=$HOME/.local/share/nit/repo.git --work-tree=$HOME` — a long
// flag pair that the global "Aesthetic-as-decision" directive flags as a
// classic escape-hatch signature. Every absent subcommand becomes friction
// that pushes users (and AI agents) toward the raw-git escape hatch.
//
// This helper takes a git subcommand name + trailing args and runs it via
// the strategy-aware exec wrapper, with stdout/stderr streaming to the
// terminal so pagers (for log/diff) and remote prompts (for push) work
// naturally.

fn cmd_passthrough(
    subcmd: &str,
    args: &[String],
    config: &NitConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let strategy = config.git_strategy();
    let mut all_args: Vec<&str> = vec![subcmd];
    all_args.extend(args.iter().map(|s| s.as_str()));
    git::run_git_with(strategy, &all_args)
}

// ---------------------------------------------------------------------------
// cmd_run — Manual trigger execution
// ---------------------------------------------------------------------------

fn cmd_run(name: &str, config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let log_dir = default_log_dir();
    let result = trigger::run_manual(name, config, &log_dir)?;

    match &result.status {
        trigger::RunStatus::Success => {
            eprintln!("nit: trigger '{}' succeeded", result.name);
            eprintln!("nit: log at {}", result.log_path.display());
        }
        trigger::RunStatus::Failed(code) => {
            eprintln!("nit: trigger '{}' failed (exit {})", result.name, code);
            eprintln!("nit: log at {}", result.log_path.display());
            return Err(format!("trigger '{}' failed", name).into());
        }
        trigger::RunStatus::Skipped(reason) => {
            eprintln!("nit: trigger '{}' skipped: {}", result.name, reason);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_encrypt / cmd_decrypt / cmd_rekey — Age encryption wiring
// ---------------------------------------------------------------------------

fn cmd_encrypt(file: &str, config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let plaintext_path = resolve_path(file);

    // Find which tier this should belong to based on filename
    let filename = plaintext_path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");

    // Look for a matching tier
    let matching_tier = config
        .fleet
        .secrets
        .tiers
        .iter()
        .find(|(name, _)| filename.contains(name.as_str()));

    let (tier_name, tier_config) = matching_tier.ok_or_else(|| {
        format!(
            "cannot determine tier for '{}' — filename should contain a tier name ({})",
            file,
            config
                .fleet
                .secrets
                .tiers
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let output_path = config.secrets_dir.join(format!("{}.env.age", tier_name));

    encrypt::encrypt_file(&plaintext_path, &tier_config.recipients, &output_path)?;

    eprintln!(
        "nit: encrypted {} → {} ({} recipients)",
        file,
        output_path.display(),
        tier_config.recipients.len()
    );

    Ok(())
}

fn cmd_decrypt(file: &str, config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let encrypted_path = resolve_path(file);
    let identity_path = config::expand_tilde(&config.local.identity);

    let plaintext = encrypt::decrypt_file(&encrypted_path, &identity_path)?;
    // Output to stdout (not stderr) for piping
    print!("{}", plaintext);

    Ok(())
}

fn cmd_rekey(config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let identity_path = config::expand_tilde(&config.local.identity);
    let secrets_dir = &config.secrets_dir;

    if !secrets_dir.exists() {
        return Err(format!("secrets directory not found: {}", secrets_dir.display()).into());
    }

    let mut rekeyed = 0;

    for (tier_name, tier_config) in &config.fleet.secrets.tiers {
        let encrypted_path = secrets_dir.join(format!("{}.env.age", tier_name));
        if !encrypted_path.exists() {
            eprintln!("nit: skipping {} — encrypted file not found", tier_name);
            continue;
        }

        encrypt::rekey_file(&encrypted_path, &identity_path, &tier_config.recipients)?;
        eprintln!(
            "nit: rekeyed {} ({} recipients)",
            tier_name,
            tier_config.recipients.len()
        );
        rekeyed += 1;
    }

    eprintln!("nit: rekeyed {} tiers", rekeyed);
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_bootstrap — stub (T-13)
// ---------------------------------------------------------------------------

fn cmd_bootstrap(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    bootstrap::run_bootstrap(url)
}

// ---------------------------------------------------------------------------
// cmd_fleet_add_recipient — append age pubkey to a tier's recipients list
// ---------------------------------------------------------------------------
//
// Hemma bootstrap Step 5 automation. When onboarding a new fleet machine,
// its age pubkey must be added to the appropriate `[secrets.tiers.*]`
// recipient lists before `nit rekey` can re-encrypt secrets for that machine.
// Uses toml_edit so comments + formatting are preserved (fleet.toml has
// per-recipient comments documenting which machine each key belongs to).
//
// Usage: nit fleet-add-recipient <tier> <pubkey> [--comment <machine-name>]

fn cmd_fleet_add_recipient(
    tier: &str,
    pubkey: &str,
    comment: Option<&str>,
    _config: &NitConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate pubkey format — age1 prefix is mandatory for the X25519 recipients
    // we use. (Catches "public key: age1..." vs raw "age1..." copy-paste mistakes.)
    if !pubkey.starts_with("age1") {
        return Err(format!(
            "invalid age pubkey: must start with 'age1', got '{}'",
            pubkey
        )
        .into());
    }

    // Locate fleet.toml (same path hemma uses)
    let fleet_path = config::expand_tilde("~/dotfiles/fleet.toml");
    if !fleet_path.exists() {
        return Err(format!("fleet.toml not found at {}", fleet_path.display()).into());
    }

    let content = std::fs::read_to_string(&fleet_path)?;
    let updated = add_recipient_to_toml(&content, tier, pubkey, comment)?;

    // Only write if content actually changed (no-op case prints its own message)
    if updated != content {
        std::fs::write(&fleet_path, &updated)?;
        eprintln!(
            "nit: added {} to tier '{}' in {}",
            pubkey,
            tier,
            fleet_path.display()
        );
        eprintln!("nit: run `nit rekey` to re-encrypt .age files for the new recipient set");
    }

    Ok(())
}

/// Pure function: given the TOML source string, a tier name, and a pubkey,
/// return the updated TOML with the pubkey appended to that tier's recipients.
/// Preserves comments + formatting via toml_edit. Idempotent: if pubkey is
/// already in the list, returns the input unchanged.
///
/// Extracted as pure fn so it's testable without filesystem I/O.
fn add_recipient_to_toml(
    content: &str,
    tier: &str,
    pubkey: &str,
    comment: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut doc: toml_edit::DocumentMut = content.parse()?;

    // Navigate [secrets.tiers.<tier>].recipients — all nested tables.
    let recipients = doc
        .get_mut("secrets")
        .and_then(|s| s.as_table_mut())
        .and_then(|s| s.get_mut("tiers"))
        .and_then(|t| t.as_table_mut())
        .and_then(|t| t.get_mut(tier))
        .and_then(|t| t.as_table_mut())
        .and_then(|t| t.get_mut("recipients"))
        .and_then(|r| r.as_array_mut())
        .ok_or_else(|| {
            format!(
                "tier '{}' not found in fleet.toml (or has no recipients list). Known tier structure is [secrets.tiers.<name>].recipients = [...]",
                tier
            )
        })?;

    // Idempotency: if pubkey already present, no-op.
    for val in recipients.iter() {
        if let Some(s) = val.as_str()
            && s == pubkey
        {
            eprintln!(
                "nit: pubkey {} already in tier '{}' — no change",
                pubkey, tier
            );
            return Ok(content.to_string());
        }
    }

    // Append with trailing comma + optional comment, matching existing style
    // (each recipient on its own line, comment as inline suffix).
    let mut new_val = toml_edit::Value::from(pubkey);
    if let Some(c) = comment {
        new_val.decor_mut().set_suffix(format!("  # {}", c));
    }
    recipients.push_formatted(new_val);

    Ok(doc.to_string())
}

fn cmd_fleet() -> Result<(), Box<dyn std::error::Error>> {
    let fleet = config::load_fleet_only()?;

    // Output format: name:ssh_host:role:critical (space-separated)
    // Consumed by hemma Justfile via: fleet := `nit fleet`
    let mut entries = Vec::new();
    let mut names: Vec<&String> = fleet.machines.keys().collect();
    names.sort();
    for name in names {
        let m = &fleet.machines[name];
        let role = m.role.join(",");
        entries.push(format!("{}:{}:{}:{}", name, m.ssh_host, role, m.critical));
    }
    println!("{}", entries.join(" "));

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_list — Inventory
// ---------------------------------------------------------------------------

fn cmd_list(config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mappings = template::discover_templates(config);
    let triggers = config.applicable_triggers();
    let drifted = syncbase::list_drifted_files();

    println!("Templates ({}):", mappings.len());
    for m in &mappings {
        let rel = target_rel_path(&m.target);
        let drift_marker = if drifted.contains(&rel) {
            " [DRIFT]"
        } else {
            ""
        };
        let exists = if m.target.exists() {
            "\u{2713}"
        } else {
            "\u{2717}"
        };
        println!(
            "  {} {} → {}{}",
            exists,
            m.rel_source.display(),
            m.target.display(),
            drift_marker
        );
    }

    println!("\nTriggers ({}):", triggers.len());
    for t in &triggers {
        let filter = match (&t.os, &t.role) {
            (Some(os), Some(role)) => format!(" [os={}, role={}]", os, role),
            (Some(os), None) => format!(" [os={}]", os),
            (None, Some(role)) => format!(" [role={}]", role),
            (None, None) => String::new(),
        };
        println!("  {} → {}{}", t.name, t.script, filter);
    }

    println!("\nSecrets ({} tiers):", config.fleet.secrets.tiers.len());
    // Determine this machine's age public key from the local identity file.
    // Used for the actual recipient-membership check below (replaces the old
    // heuristic that just substring-matched tier name against machine role).
    let identity_path = config::expand_tilde(&config.local.identity);
    let my_pubkey = read_identity_pubkey(&identity_path);
    for (name, tier) in &config.fleet.secrets.tiers {
        let can_decrypt = match &my_pubkey {
            Some(pk) if tier.recipients.iter().any(|r| r == pk) => "\u{2713}",
            Some(_) => "\u{2717}",
            None => "?",
        };
        println!(
            "  {} {} → {} ({} recipients)",
            can_decrypt,
            name,
            tier.target,
            tier.recipients.len()
        );
    }
    if my_pubkey.is_none() {
        eprintln!(
            "nit: warning: could not read identity at {} — secret status shows '?'",
            identity_path.display()
        );
    }

    Ok(())
}

/// Extract the age public key from an identity (key.txt) file.
/// Looks for the `# public key: ageXXX...` comment line that age writes when
/// generating identities. Returns None if the file is missing or malformed.
fn read_identity_pubkey(identity_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(identity_path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# public key:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── discard_verdict: discard only what is provably in git ─────────
    #[test]
    fn a_stale_copy_may_be_discarded() {
        let k = drift_triage::DriftKind::Stale {
            commit: "abc123".into(),
            date: "2026-06-28".into(),
        };
        assert!(matches!(
            discard_verdict(" M", &k),
            DiscardVerdict::Restore(_)
        ));
    }

    #[test]
    fn new_or_unprovable_content_is_never_discarded() {
        for k in [
            drift_triage::DriftKind::Unique,
            drift_triage::DriftKind::Unknown,
        ] {
            assert!(matches!(
                discard_verdict(" M", &k),
                DiscardVerdict::Refuse(_)
            ));
        }
    }

    #[test]
    fn anything_staged_is_never_discarded_even_when_stale() {
        let k = drift_triage::DriftKind::Stale {
            commit: "abc123".into(),
            date: "2026-06-28".into(),
        };
        assert!(matches!(
            discard_verdict("MM", &k),
            DiscardVerdict::Refuse(_)
        ));
    }

    // ─── PulledAroundGuard: an edit the pull went around stays safe ────
    #[test]
    fn guard_drops_the_copy_of_an_untouched_edit() {
        let home = tempfile::TempDir::new().unwrap();
        let keep = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(home.path().join("docs")).unwrap();
        std::fs::write(home.path().join("docs/a.md"), b"my edit").unwrap();
        let g = PulledAroundGuard::take_in(keep.path(), home.path(), &["docs/a.md".into()]);
        assert!(
            keep.path().join("docs/a.md").exists(),
            "copy taken before the pull"
        );
        assert!(g.verify_in(home.path()).is_empty());
        assert!(
            !keep.path().join("docs/a.md").exists(),
            "copy removed when unchanged"
        );
    }

    #[test]
    fn guard_keeps_and_reports_an_edit_something_changed() {
        let home = tempfile::TempDir::new().unwrap();
        let keep = tempfile::TempDir::new().unwrap();
        std::fs::write(home.path().join("a.lua"), b"my edit").unwrap();
        let g = PulledAroundGuard::take_in(keep.path(), home.path(), &["a.lua".into()]);
        std::fs::write(home.path().join("a.lua"), b"a trigger wrote this").unwrap();
        let changed = g.verify_in(home.path());
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, "a.lua");
        assert_eq!(std::fs::read(&changed[0].1).unwrap(), b"my edit");
    }

    #[test]
    fn guard_reports_an_edited_file_that_vanished() {
        let home = tempfile::TempDir::new().unwrap();
        let keep = tempfile::TempDir::new().unwrap();
        std::fs::write(home.path().join("a.md"), b"my edit").unwrap();
        let g = PulledAroundGuard::take_in(keep.path(), home.path(), &["a.md".into()]);
        std::fs::remove_file(home.path().join("a.md")).unwrap();
        assert_eq!(g.verify_in(home.path()).len(), 1);
    }

    #[test]
    fn guard_ignores_a_locally_deleted_file_that_stays_deleted() {
        let home = tempfile::TempDir::new().unwrap();
        let keep = tempfile::TempDir::new().unwrap();
        let g = PulledAroundGuard::take_in(keep.path(), home.path(), &["gone.md".into()]);
        assert!(g.verify_in(home.path()).is_empty());
    }

    // ─── is_real_drift: self-healing drift detection ──────────────────
    #[test]
    fn test_is_real_drift_no_base_is_not_drift() {
        // never deployed (no sync-base) → fresh deploy, not drift
        assert!(!is_real_drift(None, Some("x"), "x"));
    }

    #[test]
    fn test_is_real_drift_base_equals_target_is_not_drift() {
        // target matches last deploy → normal update of new render, not drift
        assert!(!is_real_drift(Some("x"), Some("x"), "new-render"));
    }

    #[test]
    fn test_is_real_drift_render_caught_up_to_target_is_not_drift() {
        // sync-base stale, BUT render now equals the (locally-edited) target →
        // source caught up; writing render is a no-op → self-heal, not drift.
        // This is the phantom-drift case (mannaminne-ingest plist, 2026-06-29).
        assert!(!is_real_drift(Some("old-render"), Some("new"), "new"));
    }

    #[test]
    fn test_is_real_drift_genuine_local_edit_is_drift() {
        // target differs from BOTH base and render → real local edit to preserve
        assert!(is_real_drift(Some("old"), Some("local-edit"), "new-render"));
    }

    #[test]
    fn test_is_real_drift_missing_target_is_not_drift() {
        assert!(!is_real_drift(Some("x"), None, "x"));
    }

    // ─── add_recipient_to_toml ────────────────────────────────────────
    //
    // Hemma bootstrap Step 5: add a new machine's age pubkey to the
    // appropriate tier's recipient list, preserving comments/format.

    const SAMPLE_FLEET: &str = r#"# Fleet inventory

[machines.macmini]
ssh_host = "macmini"
os = "darwin"
role = ["dev"]

[secrets]
source_dir = "~/dotfiles/secrets"

[secrets.tiers.tier-all]
recipients = [
  "age1aaa",  # Mac Mini
  "age1bbb",  # Darwin
]
target = "~/.secrets/tier-all.env"

[secrets.tiers.tier-mac]
recipients = [
  "age1aaa",  # Mac Mini
]
target = "~/.secrets/tier-mac.env"

[templates]
source_dir = "~/dotfiles/templates"
"#;

    #[test]
    fn test_add_recipient_appends_to_tier() {
        let updated = add_recipient_to_toml(SAMPLE_FLEET, "tier-all", "age1ccc", Some("merian"))
            .expect("should succeed");

        assert!(
            updated.contains("age1ccc"),
            "new pubkey should appear in output"
        );
        assert!(
            updated.contains("# merian"),
            "comment should appear as inline suffix"
        );
        assert!(
            updated.contains("age1aaa") && updated.contains("age1bbb"),
            "existing recipients must be preserved"
        );
    }

    #[test]
    fn test_add_recipient_preserves_comments() {
        let updated = add_recipient_to_toml(SAMPLE_FLEET, "tier-all", "age1ccc", Some("merian"))
            .expect("should succeed");

        // Existing comments must survive
        assert!(
            updated.contains("# Mac Mini"),
            "existing 'Mac Mini' comment must survive"
        );
        assert!(
            updated.contains("# Darwin"),
            "existing 'Darwin' comment must survive"
        );
        assert!(
            updated.contains("# Fleet inventory"),
            "top-level comment must survive"
        );
    }

    #[test]
    fn test_add_recipient_idempotent_duplicate() {
        // Adding a pubkey that's already there is a no-op (returns input unchanged)
        let updated = add_recipient_to_toml(SAMPLE_FLEET, "tier-all", "age1aaa", Some("macmini"))
            .expect("should succeed");

        assert_eq!(
            updated.trim(),
            SAMPLE_FLEET.trim(),
            "duplicate pubkey → no changes"
        );
    }

    #[test]
    fn test_add_recipient_rejects_invalid_pubkey() {
        // We do this check in cmd_fleet_add_recipient (not the pure fn), so the
        // pure fn accepts anything — test via the outer validation separately.
        // Here: confirm the pure fn doesn't crash on non-age1 input.
        let result = add_recipient_to_toml(SAMPLE_FLEET, "tier-all", "not-a-key", None);
        assert!(
            result.is_ok(),
            "pure fn shouldn't validate format (that's the wrapper's job)"
        );
    }

    #[test]
    fn test_add_recipient_missing_tier_errors() {
        let err =
            add_recipient_to_toml(SAMPLE_FLEET, "tier-nonexistent", "age1xyz", None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not found") || msg.contains("tier-nonexistent"),
            "error should mention missing tier, got: {}",
            msg
        );
    }

    #[test]
    fn test_add_recipient_to_different_tiers_independent() {
        // Adding to tier-mac shouldn't affect tier-all.
        let updated = add_recipient_to_toml(SAMPLE_FLEET, "tier-mac", "age1ddd", Some("merian"))
            .expect("should succeed");

        // Count occurrences — tier-mac should have one new, tier-all should be unchanged
        let ddd_count = updated.matches("age1ddd").count();
        assert_eq!(ddd_count, 1, "new pubkey added exactly once");

        // tier-all still has only 2 recipients
        // Crude check: "age1bbb" (last tier-all entry) should be followed by ]
        // of the tier-all recipients array, not by another age1 line.
        assert!(
            updated.contains("age1aaa") && updated.contains("age1bbb"),
            "tier-all existing recipients preserved"
        );
    }

    #[test]
    fn test_add_recipient_no_comment() {
        // Optional comment: caller can omit it
        let updated = add_recipient_to_toml(SAMPLE_FLEET, "tier-all", "age1noc", None)
            .expect("should succeed");
        assert!(updated.contains("age1noc"));
    }

    #[test]
    fn test_add_recipient_output_is_valid_toml() {
        // Round-trip: output must parse as valid TOML
        let updated = add_recipient_to_toml(SAMPLE_FLEET, "tier-all", "age1parsecheck", Some("m3"))
            .expect("should succeed");

        let reparsed: toml::Value = updated.parse().expect("output must be valid TOML");
        // Sanity: navigate to the recipients list and confirm entry present
        let recipients = reparsed
            .get("secrets")
            .and_then(|s| s.get("tiers"))
            .and_then(|t| t.get("tier-all"))
            .and_then(|t| t.get("recipients"))
            .and_then(|r| r.as_array())
            .expect("recipients array should exist");
        let has_new = recipients
            .iter()
            .any(|v| v.as_str() == Some("age1parsecheck"));
        assert!(has_new, "re-parsed TOML must contain new recipient");
    }

    // ─── resolve_path_with ──────────────────────────────────────────────
    //
    // nit's work-tree is $HOME. Relative paths that the user types from a
    // subdirectory (e.g., `nit add .claude/CLAUDE.md` from ~/dotfiles) should
    // fall back to $HOME-relative resolution if the cwd-relative form doesn't
    // exist. Tests use tempfile::tempdir() to isolate cwd + home so they
    // don't depend on real filesystem state.

    #[test]
    fn test_resolve_path_absolute_passthrough() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("cwd");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let result = resolve_path_with("/etc/passwd", &cwd, &home);
        assert_eq!(result, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn test_resolve_path_cwd_relative_existing_prefers_cwd() {
        // When the cwd-relative form exists, use it (don't surprise-jump to home).
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("Projects/foo");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        // Create file in BOTH locations to make sure cwd is preferred.
        std::fs::write(cwd.join("bar.txt"), b"cwd").unwrap();
        std::fs::write(home.join("bar.txt"), b"home").unwrap();

        let result = resolve_path_with("bar.txt", &cwd, &home);
        assert_eq!(
            result,
            cwd.join("bar.txt"),
            "cwd-relative existing must take precedence over home-relative"
        );
    }

    #[test]
    fn test_resolve_path_falls_back_to_home_when_cwd_missing() {
        // The bug-fix case: from ~/dotfiles, `nit add .claude/CLAUDE.md`
        // should resolve to ~/.claude/CLAUDE.md (not ~/dotfiles/.claude/CLAUDE.md).
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("dotfiles");
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/CLAUDE.md"), b"").unwrap();

        let result = resolve_path_with(".claude/CLAUDE.md", &cwd, &home);
        assert_eq!(
            result,
            home.join(".claude/CLAUDE.md"),
            "must fall back to home-relative when cwd-relative is missing"
        );
    }

    #[test]
    fn test_resolve_path_returns_cwd_form_when_neither_exists() {
        // When the user typos a path, the error should be contextual to the
        // invocation (cwd-relative form), not jumped to a home path the user
        // never thought of.
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("cwd");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let result = resolve_path_with("nonexistent.txt", &cwd, &home);
        assert_eq!(
            result,
            cwd.join("nonexistent.txt"),
            "missing-everywhere must return cwd-relative for contextual error"
        );
    }

    #[test]
    fn test_resolve_path_dotted_subdir_from_home_works() {
        // Sanity: when running from $HOME itself, cwd-relative === home-relative,
        // both work, cwd-form wins.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/CLAUDE.md"), b"").unwrap();

        let result = resolve_path_with(".claude/CLAUDE.md", &home, &home);
        assert_eq!(result, home.join(".claude/CLAUDE.md"));
    }

    // The 2026-05-17 scope planner and its tests were replaced 2026-10-18 by
    // a private index per session (src/session.rs, tests/commit_flow.rs).
    fn sv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // ── The final `git commit` argument list ──────────────────────────────

    #[test]
    fn a_scoped_commit_names_its_paths_after_the_separator() {
        assert_eq!(
            commit_git_args(Some("msg"), false, false, &sv(&[":/a.txt", ":/b c.txt"])),
            sv(&["commit", "-m", "msg", "--", ":/a.txt", ":/b c.txt"])
        );
    }

    #[test]
    fn concluding_a_merge_passes_no_pathspec() {
        // git: "cannot do a partial commit during a merge"
        assert_eq!(
            commit_git_args(Some("merge"), false, true, &sv(&[":/a.txt"])),
            sv(&["commit", "-m", "merge"])
        );
        assert_eq!(
            commit_git_args(None, false, true, &[]),
            sv(&["commit", "--no-edit"]),
            "no message given → git's prepared merge message"
        );
    }

    #[test]
    fn an_amend_with_no_paths_rewords_and_takes_nothing_staged() {
        assert_eq!(
            commit_git_args(Some("better"), true, false, &[]),
            sv(&["commit", "--amend", "-m", "better", "--only"])
        );
    }

    #[test]
    fn an_amend_with_paths_keeps_the_message_unless_one_is_given() {
        assert_eq!(
            commit_git_args(None, true, false, &sv(&[":/a.txt"])),
            sv(&["commit", "--amend", "--no-edit", "--", ":/a.txt"])
        );
    }

    #[test]
    fn an_empty_scope_never_becomes_a_commit_of_the_whole_index() {
        // `--only` with no pathspec is refused by git unless `--amend` is
        // there, so this call fails and commits nothing.
        let args = commit_git_args(Some("msg"), false, false, &[]);
        assert_eq!(args, sv(&["commit", "-m", "msg", "--only"]));
    }

    // ── nit-transparency: passthrough `--` preservation (sharp-edge #5) ────

    fn av(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn passthrough_routes_log_preserving_double_dash() {
        let argv = av(&["nit", "log", "master..HEAD", "--", "dotfiles/CLAUDE.md"]);
        let got = passthrough_subcommand(&argv);
        assert_eq!(
            got,
            Some((
                "log",
                av(&["master..HEAD", "--", "dotfiles/CLAUDE.md"]).as_slice()
            )),
            "the `--` pathspec separator MUST survive verbatim"
        );
    }

    #[test]
    fn passthrough_routes_bare_diff() {
        let argv = av(&["nit", "diff"]);
        assert_eq!(passthrough_subcommand(&argv), Some(("diff", [].as_slice())));
    }

    #[test]
    fn passthrough_routes_diff_cached() {
        let argv = av(&["nit", "diff", "--cached"]);
        assert_eq!(
            passthrough_subcommand(&argv),
            Some(("diff", av(&["--cached"]).as_slice()))
        );
    }

    #[test]
    fn passthrough_ignores_nit_subcommands_and_empty() {
        assert!(passthrough_subcommand(&av(&["nit", "commit", "-m", "x"])).is_none());
        assert!(passthrough_subcommand(&av(&["nit", "status", "-v"])).is_none());
        assert!(passthrough_subcommand(&av(&["nit", "add", "f"])).is_none());
        assert!(passthrough_subcommand(&av(&["nit", "sync"])).is_none());
        assert!(passthrough_subcommand(&av(&["nit"])).is_none());
    }

    // ── nit-transparency: -m/-F commit-message resolution ─────────────────

    #[test]
    fn commit_msg_m_and_f_mutually_exclusive() {
        let e = resolve_commit_message(&av(&["m"]), Some("f"), Some("x")).unwrap_err();
        assert!(e.contains("cannot combine"), "got: {e}");
    }

    #[test]
    fn commit_msg_multiple_m_with_f_still_errors() {
        let e = resolve_commit_message(&av(&["a", "b"]), Some("f"), None).unwrap_err();
        assert!(e.contains("cannot combine"), "got: {e}");
    }

    #[test]
    fn commit_msg_dash_m_passthrough() {
        assert_eq!(
            resolve_commit_message(&av(&["hello"]), None, None).unwrap(),
            "hello"
        );
    }

    #[test]
    fn commit_msg_multiple_m_joined_as_paragraphs() {
        // git semantics: each -m is a paragraph, joined by a blank line.
        assert_eq!(
            resolve_commit_message(&av(&["title line", "body paragraph"]), None, None).unwrap(),
            "title line\n\nbody paragraph"
        );
    }

    #[test]
    fn commit_msg_dash_f_trims_trailing_newline() {
        assert_eq!(
            resolve_commit_message(&[], Some("msg.txt"), Some("subject\n\nbody\n")).unwrap(),
            "subject\n\nbody"
        );
    }

    #[test]
    fn commit_msg_dash_f_empty_is_error() {
        let e = resolve_commit_message(&[], Some("-"), Some("  \n")).unwrap_err();
        assert!(e.contains("empty"), "got: {e}");
    }

    #[test]
    fn commit_msg_dash_f_unreadable_is_error() {
        let e = resolve_commit_message(&[], Some("nope.txt"), None).unwrap_err();
        assert!(e.contains("could not read"), "got: {e}");
    }

    #[test]
    fn commit_msg_default_when_neither() {
        assert_eq!(
            resolve_commit_message(&[], None, None).unwrap(),
            "nit commit"
        );
    }

    // ── always_commit selection (2026-10-18) ───────────────────────────────

    #[test]
    fn always_commit_due_takes_only_listed_unstaged_edits() {
        let list = av(&[".codex/config.toml", "Documents/a b.json"]);
        let porcelain = " M .codex/config.toml\n M notes.md\nM  Documents/a b.json\n D x\n";
        assert_eq!(
            always_commit_due(porcelain, &list),
            av(&[".codex/config.toml"])
        );
    }

    #[test]
    fn always_commit_due_reads_a_quoted_path() {
        let list = av(&["Documents/a b.json"]);
        assert_eq!(
            always_commit_due(" M \"Documents/a b.json\"\n", &list),
            av(&["Documents/a b.json"])
        );
    }

    #[test]
    fn always_commit_due_ignores_deletions_and_an_empty_list() {
        assert!(
            always_commit_due(" D .codex/config.toml\n", &av(&[".codex/config.toml"])).is_empty()
        );
        assert!(always_commit_due(" M .codex/config.toml\n", &[]).is_empty());
    }
}
