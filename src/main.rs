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
        NitCommand::Pick { file, dismiss } => cmd_pick(file.as_deref(), dismiss, config),
        NitCommand::Commit { message } => cmd_commit(message.as_deref(), config),
        NitCommand::Update { safe } => cmd_update(safe, config),
        NitCommand::Status => cmd_status(config),
        NitCommand::Encrypt { file } => cmd_encrypt(&file, config),
        NitCommand::Decrypt { file } => cmd_decrypt(&file, config),
        NitCommand::Rekey => cmd_rekey(config),
        NitCommand::List => cmd_list(config),
        NitCommand::Run { name } => cmd_run(&name, config),
        // Bootstrap and Git handled in main()
        NitCommand::Bootstrap { .. } | NitCommand::Git(_) => unreachable!(),
    }
}

// --- T-3: Smart add with template target detection ---

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

            // Scan all templates for drift awareness
            if !mappings.is_empty() {
                eprintln!("nit: scanning {} templates for drift...", mappings.len());
                for mapping in &mappings {
                    report_template_drift(mapping);
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

        // Show drift between current target and what template would render
        // (Full drift display + ack writing comes in T-5/T-8)
        if target.exists() {
            eprintln!(
                "nit: drift check for {} (full review with `nit pick`)",
                target.display()
            );
        }
    }

    Ok(())
}

/// Resolve a path string to an absolute PathBuf
fn resolve_path(path_str: &str) -> PathBuf {
    let path = Path::new(path_str);

    // Handle tilde
    if path_str.starts_with("~/") || path_str == "~" {
        return config::expand_tilde(path_str);
    }

    // Handle relative paths
    if path.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join(path);
        }
    }

    path.to_path_buf()
}

/// Report template drift (placeholder — full implementation in T-5)
fn report_template_drift(mapping: &template::TemplateMapping) {
    if mapping.target.exists() {
        // In T-5 this will compare sync-base vs target to detect drift
        // For now just note the template exists
        eprintln!(
            "  {} → {}",
            mapping.rel_source.display(),
            mapping.target.display()
        );
    }
}

// --- Command stubs (implemented progressively) ---

fn cmd_apply(_file: Option<&str>, _config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    // T-4/T-5: Render templates + deploy (no commit)
    eprintln!("nit apply: not yet implemented");
    Ok(())
}

fn cmd_pick(
    _file: Option<&str>,
    _dismiss: bool,
    _config: &NitConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // T-9: Proactive drift review
    eprintln!("nit pick: not yet implemented");
    Ok(())
}

fn cmd_commit(
    _message: Option<&str>,
    _config: &NitConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // T-10: Render + deploy + ack gate + git commit + triggers
    eprintln!("nit commit: not yet implemented");
    Ok(())
}

fn cmd_update(_safe: bool, _config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    // T-12: git pull + render + deploy + triggers (fleet sync)
    eprintln!("nit update: not yet implemented");
    Ok(())
}

fn cmd_status(config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let strategy = config.git_strategy();

    // Template drift count
    let mappings = template::discover_templates(config);
    let template_count = mappings.len();

    // Git status summary
    let git_status = git::git_output_with(strategy, &["status", "--porcelain"])
        .unwrap_or_default();
    let modified = git_status.lines().filter(|l| l.starts_with(" M") || l.starts_with("M ")).count();
    let staged = git_status.lines().filter(|l| {
        let first = l.chars().next().unwrap_or(' ');
        first != ' ' && first != '?'
    }).count();
    let untracked = git_status.lines().filter(|l| l.starts_with("??")).count();

    // Trigger count
    let trigger_count = config.applicable_triggers().len();

    println!(
        "nit: {} templates, {} triggers | git: {} staged, {} modified, {} untracked",
        template_count, trigger_count, staged, modified, untracked
    );

    Ok(())
}

fn cmd_bootstrap(_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    // T-13: Clone bare repo + configure + initial deploy
    eprintln!("nit bootstrap: not yet implemented");
    Ok(())
}

fn cmd_encrypt(_file: &str, _config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    // T-6: Age encryption
    eprintln!("nit encrypt: not yet implemented");
    Ok(())
}

fn cmd_decrypt(_file: &str, _config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    // T-6: Age decryption
    eprintln!("nit decrypt: not yet implemented");
    Ok(())
}

fn cmd_rekey(_config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    // T-6: Re-encrypt all secrets
    eprintln!("nit rekey: not yet implemented");
    Ok(())
}

fn cmd_list(config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mappings = template::discover_templates(config);
    let triggers = config.applicable_triggers();

    println!("Templates ({}):", mappings.len());
    for m in &mappings {
        let exists = if m.target.exists() { "✓" } else { "✗" };
        println!("  {} {} → {}", exists, m.rel_source.display(), m.target.display());
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
        }) || name.contains("all") {
            "✓"
        } else {
            "✗"
        };
        println!("  {} {} → {} ({} recipients)", can_decrypt, name, tier.target, tier.recipients.len());
    }

    Ok(())
}

fn cmd_run(_name: &str, _config: &NitConfig) -> Result<(), Box<dyn std::error::Error>> {
    // T-7: Manual trigger execution
    eprintln!("nit run: not yet implemented");
    Ok(())
}
