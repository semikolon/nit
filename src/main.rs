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
    version,
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
    Status,

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
            match cmd_status(&config) {
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
        NitCommand::Apply { file } => cmd_apply(file.as_deref(), config),
        NitCommand::Pick {
            file,
            dismiss,
            diff,
            edit,
        } => cmd_pick(file.as_deref(), dismiss, diff, edit, config),
        NitCommand::Commit { message } => cmd_commit(message.as_deref(), config),
        NitCommand::Update { safe } => cmd_update(safe, config),
        NitCommand::Status => cmd_status(config),
        NitCommand::Encrypt { file } => cmd_encrypt(&file, config),
        NitCommand::Decrypt { file } => cmd_decrypt(&file, config),
        NitCommand::Rekey => cmd_rekey(config),
        NitCommand::List => cmd_list(config),
        NitCommand::Run { name } => cmd_run(&name, config),
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
    let path = Path::new(path_str);

    // Handle tilde
    if path_str.starts_with("~/") || path_str == "~" {
        return config::expand_tilde(path_str);
    }

    // Handle relative paths
    if path.is_relative()
        && let Ok(cwd) = std::env::current_dir()
    {
        return cwd.join(path);
    }

    path.to_path_buf()
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
// ---------------------------------------------------------------------------

fn cmd_apply(file: Option<&str>, config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
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
        }

        deployed_count += 1;

        // 9. Write ack for this template
        let target_hash = syncbase::hash_content(&rendered_with_comment);
        let rendered_hash = syncbase::hash_content(&rendered_with_comment);
        syncbase::write_ack(&rel, &target_hash, &rendered_hash);
    }

    // 7. Decrypt secrets
    match encrypt::deploy_secrets(config) {
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
                }
            }
        }
        Err(e) => {
            eprintln!("nit: warning: secret deployment failed: {}", e);
        }
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
        eprintln!(
            "  Opening template source in $EDITOR. Incorporate desired changes by hand."
        );
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
            return Err(format!(
                "editor '{}' exited with status {}",
                editor, status
            )
            .into());
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

    // Decrypt secrets
    if let Err(e) = encrypt::deploy_secrets(config) {
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

    // 1. git pull
    eprintln!("nit: pulling latest...");
    let pull_status = git::exec_git_with(strategy, &["pull"])?;
    if !pull_status.success() {
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
            deployed_count += 1;
        }
    }

    // 3. Decrypt secrets
    match encrypt::deploy_secrets(config) {
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
                }
            }
        }
        Err(e) => {
            eprintln!("nit: warning: secret deployment failed: {}", e);
        }
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

    // 5. No commit
    eprintln!(
        "nit update: {} deployed, {} skipped (drift)",
        deployed_count, skipped_count
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_status — One-line summary with drift count
// ---------------------------------------------------------------------------

fn cmd_status(config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let strategy = config.git_strategy();

    // Template drift count
    let mappings = template::discover_templates(config);
    let template_count = mappings.len();
    let drift_count = syncbase::list_drifted_files().len();

    // Git status summary
    let git_status = git::git_output_with(strategy, &["status", "--porcelain"]).unwrap_or_default();
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
    for (name, tier) in &config.fleet.secrets.tiers {
        let can_decrypt = if config.machine.role.iter().any(|r| {
            // Simple heuristic: tier name contains role name
            name.contains(r)
        }) || name.contains("all")
        {
            "\u{2713}"
        } else {
            "\u{2717}"
        };
        println!(
            "  {} {} → {} ({} recipients)",
            can_decrypt,
            name,
            tier.target,
            tier.recipients.len()
        );
    }

    Ok(())
}
