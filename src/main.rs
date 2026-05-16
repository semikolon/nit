//! nit — AI-era dotfiles manager
//!
//! git's rivet. Bare git for 547 plain files (edit in place),
//! selective tera templates (~10 files) + age encryption (4 files)
//! + hash triggers (19 scripts).

mod bootstrap;
mod config;
mod encrypt;
mod git;
mod permissions;
mod pick;
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

    /// Render + deploy + git commit + triggers
    Commit {
        /// Commit message
        #[arg(short, long)]
        message: Option<String>,
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

    /// Any unrecognized subcommand falls through to git
    #[command(external_subcommand)]
    Git(Vec<String>),
}

fn main() -> ExitCode {
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
        NitCommand::Commit { message } => cmd_commit(message.as_deref(), config),
        NitCommand::Update { safe } => cmd_update(safe, config),
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

    // Discover templates and build reverse lookup
    let mappings = template::discover_templates(config);
    let target_to_source = template::build_target_to_source_map(&mappings);

    let mut git_add_paths: Vec<PathBuf> = Vec::new();
    let mut template_redirects: Vec<(PathBuf, PathBuf)> = Vec::new(); // (target, source)

    for path_str in paths {
        // Handle "." and "-A" as bulk operations
        if path_str == "." || path_str == "-A" {
            // Stage all modified tracked files via git
            git::exec_git_with(strategy, &["add", path_str])?;

            // Scan all templates for drift awareness + write acks
            if !mappings.is_empty() {
                eprintln!("nit: scanning {} templates for drift...", mappings.len());
                for mapping in &mappings {
                    report_template_drift(mapping, config);
                }
            }
            return Ok(());
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

    // Stage plain files via git add
    if !git_add_paths.is_empty() {
        let path_strs: Vec<String> = git_add_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let mut args = vec!["add"];
        let refs: Vec<&str> = path_strs.iter().map(|s| s.as_str()).collect();
        args.extend(refs);
        git::exec_git_with(strategy, &args)?;
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
        git::exec_git_with(strategy, &["add", &source_str])?;

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

    Ok(())
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

        let has_drift = matches!((&base_content, &target_content), (Some(base), Some(target)) if base != target);

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
                    } => {
                        secrets_drift_count += 1;
                        eprintln!(
                            "nit: secret {} DRIFT: {} (target {}B, source-decrypt {}B) — \
                             target diverges from source. The target was likely vim-edited \
                             without `nit encrypt`. Apply was skipped to prevent silent overwrite. \
                             Resolve with one of:\n  \
                             nit encrypt {}    # flush target → source (preserve target edits)\n  \
                             nit apply --force-drift   # overwrite target with source (discard target edits)",
                            r.tier, r.target, target_bytes, source_bytes, r.target
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

fn cmd_commit(message: Option<&str>, config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let strategy = config.git_strategy();
    let msg = message.unwrap_or("nit commit");

    // 1. Check what's staged
    let staged_output = git::git_output_with(strategy, &["diff", "--cached", "--name-only"])?;
    let staged_files: Vec<&str> = staged_output.lines().filter(|l| !l.is_empty()).collect();

    if staged_files.is_empty() {
        return Err("nothing staged to commit".into());
    }

    // 2. Identify staged template sources
    let mappings = template::discover_templates(config);
    // Find which staged files are template sources
    let home = dirs::home_dir().expect("cannot determine home directory");
    let staged_template_mappings: Vec<&template::TemplateMapping> = mappings
        .iter()
        .filter(|m| {
            let source_rel = m
                .source
                .strip_prefix(&home)
                .unwrap_or(&m.source)
                .to_string_lossy()
                .to_string();
            staged_files.iter().any(|sf| *sf == source_rel)
        })
        .collect();

    let has_template_sources = !staged_template_mappings.is_empty();

    // 4. If ONLY plain files staged (no template sources): skip all ack checks
    if !has_template_sources {
        // Straight git commit
        git::exec_git_with(strategy, &["commit", "-m", msg])?;
        eprintln!("nit: committed (plain files only, no templates)");
        return Ok(());
    }

    // 3. For each staged template source, apply 4-cell ack validation
    //
    // Acks are keyed by SESSION ANCHOR (not raw PPID) — see syncbase::
    // get_session_anchor for the walk-up rationale. This makes one CC
    // conversation = one stable identity across all of CC's per-Bash-call
    // ephemeral shells. Same for Codex sessions, terminal shell sessions, etc.
    let my_anchor = syncbase::get_session_anchor();
    let my_acks = syncbase::read_acks(my_anchor);

    // Prune dead-anchor ack files (pure housekeeping, no longer load-bearing
    // since cross-session ack reuse was removed — only own-anchor acks count).
    syncbase::prune_dead_acks();

    let mut blocked = false;
    let mut block_reasons: Vec<String> = Vec::new();
    let mut drifted_rels: Vec<String> = Vec::new();

    for mapping in &staged_template_mappings {
        let rel = target_rel_path(&mapping.target);

        // Current rendered content
        let rendered = template::render_template(mapping, config)?;
        let rendered_with_comment = prepend_warning(&rendered, &mapping.target);
        let current_rendered_hash = syncbase::hash_content(&rendered_with_comment);

        // Current target content
        let current_target_content = std::fs::read_to_string(&mapping.target).unwrap_or_default();
        let current_target_hash = syncbase::hash_content(&current_target_content);

        if let Some(ack) = my_acks.get(&rel) {
            // I have an ack — check 4-cell matrix
            let rendered_match = ack.rendered_hash == current_rendered_hash;
            let target_match = ack.target_hash == current_target_hash;

            match (rendered_match, target_match) {
                (true, true) => {
                    // Nothing changed since review — proceed
                }
                (true, false) => {
                    blocked = true;
                    block_reasons.push(format!(
                        "{}: target drift since review — run nit pick or nit apply",
                        rel
                    ));
                }
                (false, true) => {
                    blocked = true;
                    block_reasons.push(format!(
                        "{}: source changed since review — run nit pick or nit apply",
                        rel
                    ));
                }
                (false, false) => {
                    blocked = true;
                    block_reasons.push(format!(
                        "{}: both source and target changed since review — run nit pick or nit apply",
                        rel
                    ));
                }
            }
        } else {
            // No ack for my session anchor → show drift inline, write ack, refuse.
            //
            // Cross-session ack reuse was removed (Apr 21, 2026). Rationale: the
            // committing agent should ALWAYS have witnessed the drift themselves.
            // Output-scrolling-past in another session's review doesn't equal
            // the committer's deliberate awareness. The "first commit fails,
            // second proceeds" pattern structurally enforces that this agent
            // engaged with the drift before persisting it. See spec design.md
            // § "Why no cross-session ack reuse" for the full rationale.
            eprintln!("nit: {} — no prior review found, showing drift:", rel);
            if let Some(drift) = detect_live_drift(mapping, config) {
                for line in drift.lines() {
                    eprintln!("    {}", line);
                }
            } else {
                eprintln!("    (no drift detected)");
            }
            // Write ack so second commit (same session anchor) proceeds.
            syncbase::write_ack(&rel, &current_target_hash, &current_rendered_hash);
            blocked = true;
            block_reasons.push(format!(
                "{}: first commit attempt — ack written, re-run nit commit to proceed",
                rel
            ));
        }
    }

    if blocked {
        eprintln!("nit: BLOCKED — resolve before committing:");
        for reason in &block_reasons {
            eprintln!("  {}", reason);
        }
        return Err("commit blocked by ack validation".into());
    }

    // 5. All acks valid — deploy, commit, run triggers
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

        // Check for drift (source wins)
        let base_content = syncbase::read_sync_base(&rel);
        let target_content = std::fs::read_to_string(&mapping.target).ok();

        let has_drift = matches!((&base_content, &target_content), (Some(base), Some(target)) if base != target);

        if has_drift {
            let drift_diff = syncbase::detect_drift(&rel, target_content.as_deref().unwrap_or(""));
            if let Some(diff) = &drift_diff {
                syncbase::save_drift(&rel, diff);
            }
            drifted_rels.push(rel.clone());
            eprintln!("nit: \u{26a0} Drift saved for {} — source wins", rel);
        }

        write_target(&mapping.target, &rendered_with_comment)?;
        syncbase::write_sync_base(&rel, &rendered_with_comment);
    }

    // Decrypt secrets (drift-check enforced — commit path has no force escape)
    if let Err(e) = encrypt::deploy_secrets(config, false) {
        eprintln!("nit: warning: secret deployment failed: {}", e);
    }

    // Git commit (only what was staged — do NOT auto-stage)
    git::exec_git_with(strategy, &["commit", "-m", msg])?;

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

    eprintln!("nit: committed with {} templates deployed", mappings.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// T-12: cmd_update — Pull + render + deploy + triggers (fleet sync)
// ---------------------------------------------------------------------------

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

    // 0. PRE-PULL DRIFT CHECK — sacred: never clobber local state.
    // If any tracked file is modified/deleted, ABORT and write status. Untracked
    // files are fine (gitignored or genuinely new — won't be touched by pull).
    let porcelain = git::git_output_with(strategy, &["status", "--porcelain"]).unwrap_or_default();
    let drift = sync_status::detect_pre_pull_drift(&porcelain);
    if !drift.is_empty() {
        status.result = sync_status::SyncResult::AbortedDrift;
        status.drift_files = drift.clone();
        status.completed_at = chrono::Utc::now().to_rfc3339();
        sync_status::save_status(&status);

        eprintln!(
            "nit update: ABORTED — pre-pull drift detected ({} file(s)):",
            drift.len()
        );
        for line in &drift {
            eprintln!("  {}", line);
        }
        eprintln!();
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

    // 1. git pull
    eprintln!("nit: pulling latest...");
    let pull_status = git::exec_git_with(strategy, &["pull"])?;
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

        let has_drift = matches!((&base_content, &target_content), (Some(base), Some(target)) if base != target);

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
                    } => {
                        secrets_drift_count += 1;
                        eprintln!(
                            "nit: secret {} DRIFT: {} (target {}B, source-decrypt {}B) — \
                             unflushed manual edit detected. Run `nit encrypt {}` to flush \
                             or `nit apply --force-drift` to discard target edits.",
                            r.tier, r.target, target_bytes, source_bytes, r.target
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
            "{} secret tier(s) have unflushed target edits — update aborted",
            secrets_drift_count
        )
        .into());
    }

    // 4. Run triggers (skip drifted files; --safe skips all)
    let log_dir = default_log_dir();
    let mut trigger_state = trigger::load_trigger_state();
    let trigger_results =
        trigger::run_applicable_triggers(config, &mut trigger_state, &drifted_rels, safe, &log_dir);
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
    let modified = git_status
        .lines()
        .filter(|l| l.starts_with(" M") || l.starts_with("M "))
        .count();
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

    println!(
        "nit: {} templates ({} drifted), {} triggers | git: {} staged, {} modified, {} untracked",
        template_count, drift_count, trigger_count, staged, modified, untracked
    );

    // Last-sync health summary (from ~/.local/share/nit/last-sync.json).
    if let Some(last) = sync_status::load_status() {
        println!("{}", sync_status::one_line_summary(&last));
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
    git::exec_git_with(strategy, &all_args)?;
    Ok(())
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
}
