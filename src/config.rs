//! Configuration loading: fleet.toml (shared) + local.toml (per-machine) + triggers.toml

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Shared fleet configuration (tracked in repo at ~/dotfiles/fleet.toml)
#[derive(Debug, Deserialize)]
pub struct FleetConfig {
    #[serde(default)]
    pub machines: HashMap<String, MachineConfig>,
    #[serde(default)]
    pub templates: TemplatesConfig,
    #[serde(default)]
    pub secrets: SecretsConfig,
    #[serde(default)]
    pub permissions: PermissionsConfig,
    #[serde(default)]
    pub exclude: HashMap<String, ExcludeRule>,
    #[serde(default)]
    pub sync: Option<SyncConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MachineConfig {
    pub ssh_host: String,
    #[serde(default)]
    pub role: Vec<String>,
    #[serde(default)]
    pub critical: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct TemplatesConfig {
    #[serde(default = "default_templates_dir")]
    pub source_dir: String,
}

fn default_templates_dir() -> String {
    "~/dotfiles/templates".to_string()
}

#[derive(Debug, Default, Deserialize)]
pub struct SecretsConfig {
    #[serde(default = "default_secrets_dir")]
    pub source_dir: String,
    #[serde(default)]
    pub tiers: HashMap<String, TierConfig>,
}

fn default_secrets_dir() -> String {
    "~/dotfiles/secrets".to_string()
}

#[derive(Debug, Deserialize)]
pub struct TierConfig {
    pub recipients: Vec<String>,
    pub target: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub private: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExcludeRule {
    pub unless_role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SyncConfig {
    #[serde(default = "default_sync_command")]
    pub command: String,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub idle_gated: bool,
    #[serde(default)]
    pub overrides: HashMap<String, SyncOverride>,
}

fn default_sync_command() -> String {
    "nit update".to_string()
}

#[derive(Debug, Deserialize)]
pub struct SyncOverride {
    pub strategy: Option<String>,
}

/// Per-machine identity (NOT tracked, at ~/.config/nit/local.toml)
#[derive(Debug, Deserialize)]
pub struct LocalConfig {
    pub machine: String,
    #[serde(default = "default_identity")]
    pub identity: String,
    #[serde(default)]
    pub git: GitStrategyConfig,
}

fn default_identity() -> String {
    "~/.config/nit/age-key.txt".to_string()
}

/// Git strategy: bare repo (default) or home dir
#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct GitStrategyConfig {
    #[serde(default = "default_strategy")]
    pub strategy: GitStrategy,
}

impl Default for GitStrategyConfig {
    fn default() -> Self {
        GitStrategyConfig {
            strategy: GitStrategy::Bare,
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GitStrategy {
    /// Strategy B (default): bare repo at ~/.local/share/nit/repo.git, work tree = $HOME
    Bare,
    /// Strategy A: regular ~/.git repo with GIT_CEILING_DIRECTORIES
    Home,
}

fn default_strategy() -> GitStrategy {
    GitStrategy::Bare
}

/// Trigger definition (from triggers.toml)
#[derive(Debug, Deserialize, Clone)]
pub struct TriggerDef {
    pub name: String,
    pub script: String,
    #[serde(default)]
    pub watch: Vec<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

/// Triggers file wrapper
#[derive(Debug, Deserialize)]
pub struct TriggersFile {
    #[serde(default)]
    pub trigger: Vec<TriggerDef>,
}

/// Resolved configuration for the current machine
#[derive(Debug)]
pub struct NitConfig {
    pub fleet: FleetConfig,
    pub local: LocalConfig,
    pub machine_name: String,
    pub machine: MachineConfig,
    pub triggers: Vec<TriggerDef>,
    /// Resolved absolute path to templates source directory
    pub templates_dir: PathBuf,
    /// Resolved absolute path to secrets source directory
    pub secrets_dir: PathBuf,
    /// Resolved absolute path to the dotfiles project hub
    pub project_dir: PathBuf,
}

impl NitConfig {
    /// Check if this machine has a given role
    pub fn has_role(&self, role: &str) -> bool {
        self.machine.role.iter().any(|r| r == role)
    }

    /// Get the git strategy for this machine
    pub fn git_strategy(&self) -> &GitStrategy {
        &self.local.git.strategy
    }

    /// Get triggers applicable to this machine (filtered by os/role)
    pub fn applicable_triggers(&self) -> Vec<&TriggerDef> {
        let os = std::env::consts::OS;
        self.triggers
            .iter()
            .filter(|t| {
                // OS filter
                if let Some(ref trigger_os) = t.os {
                    if trigger_os != os {
                        return false;
                    }
                }
                // Role filter
                if let Some(ref trigger_role) = t.role {
                    if !self.has_role(trigger_role) {
                        return false;
                    }
                }
                true
            })
            .collect()
    }
}

/// Expand ~ to $HOME
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .expect("cannot determine home directory")
            .join(rest)
    } else if path == "~" {
        dirs::home_dir().expect("cannot determine home directory")
    } else {
        PathBuf::from(path)
    }
}

/// Load and resolve configuration from default paths
pub fn load_config() -> Result<NitConfig, Box<dyn std::error::Error>> {
    let fleet_path = expand_tilde("~/dotfiles/fleet.toml");
    let local_path = expand_tilde("~/.config/nit/local.toml");
    let triggers_path = expand_tilde("~/dotfiles/triggers.toml");

    load_config_from(&fleet_path, &local_path, &triggers_path)
}

/// Load and resolve configuration from explicit paths (testable)
pub fn load_config_from(
    fleet_path: &Path,
    local_path: &Path,
    triggers_path: &Path,
) -> Result<NitConfig, Box<dyn std::error::Error>> {
    let fleet = load_fleet(fleet_path)?;
    let local = load_local(local_path)?;
    let triggers = load_triggers(triggers_path)?;

    let machine_name = local.machine.clone();
    let machine = fleet
        .machines
        .get(&machine_name)
        .ok_or_else(|| {
            format!(
                "machine '{}' not found in fleet.toml (available: {})",
                machine_name,
                fleet.machines.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?
        .clone();

    let templates_dir = expand_tilde(&fleet.templates.source_dir);
    let secrets_dir = expand_tilde(&fleet.secrets.source_dir);
    let project_dir = expand_tilde("~/dotfiles");

    Ok(NitConfig {
        fleet,
        local,
        machine_name,
        machine,
        triggers,
        templates_dir,
        secrets_dir,
        project_dir,
    })
}

fn load_fleet(path: &Path) -> Result<FleetConfig, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "nit: cannot read fleet.toml at {}: {}\n  Run `nit bootstrap` to set up.",
            path.display(),
            e
        )
    })?;
    let config: FleetConfig = toml::from_str(&content).map_err(|e| {
        format!("nit: parse error in fleet.toml: {}", e)
    })?;
    Ok(config)
}

fn load_local(path: &Path) -> Result<LocalConfig, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "nit: cannot read local.toml at {}: {}\n  Run `nit bootstrap` to set up this machine.",
            path.display(),
            e
        )
    })?;
    let config: LocalConfig = toml::from_str(&content).map_err(|e| {
        format!("nit: parse error in local.toml: {}", e)
    })?;
    Ok(config)
}

fn load_triggers(path: &Path) -> Result<Vec<TriggerDef>, Box<dyn std::error::Error>> {
    // triggers.toml is optional — return empty if not found
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(format!("nit: cannot read triggers.toml: {}", e).into());
        }
    };
    let file: TriggersFile = toml::from_str(&content).map_err(|e| {
        format!("nit: parse error in triggers.toml: {}", e)
    })?;
    Ok(file.trigger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/foo/bar");
        assert!(expanded.to_str().unwrap().contains("foo/bar"));
        assert!(!expanded.to_str().unwrap().starts_with("~"));
    }

    #[test]
    fn test_expand_tilde_bare() {
        let expanded = expand_tilde("~");
        assert_eq!(expanded, dirs::home_dir().unwrap());
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let expanded = expand_tilde("/absolute/path");
        assert_eq!(expanded, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_parse_fleet_toml() {
        let toml_str = r#"
[machines.mac-mini]
ssh_host = "localhost"
role = ["dev", "primary"]

[machines.darwin]
ssh_host = "darwin"
role = ["dev", "server"]
critical = true

[templates]
source_dir = "~/dotfiles/templates"

[secrets]
source_dir = "~/dotfiles/secrets"

[secrets.tiers.tier-all]
recipients = ["age1abc..."]
target = "~/.secrets/tier-all.env"
"#;
        let config: FleetConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.machines.len(), 2);
        assert_eq!(config.machines["mac-mini"].role, vec!["dev", "primary"]);
        assert!(config.machines["darwin"].critical);
        assert_eq!(config.secrets.tiers.len(), 1);
    }

    #[test]
    fn test_parse_local_toml_default_strategy() {
        let toml_str = r#"
machine = "mac-mini"
"#;
        let config: LocalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.machine, "mac-mini");
        assert_eq!(config.git.strategy, GitStrategy::Bare);
        assert_eq!(config.identity, "~/.config/nit/age-key.txt");
    }

    #[test]
    fn test_parse_local_toml_home_strategy() {
        let toml_str = r#"
machine = "mac-mini"

[git]
strategy = "home"
"#;
        let config: LocalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.git.strategy, GitStrategy::Home);
    }

    #[test]
    fn test_parse_triggers_toml() {
        let toml_str = r#"
[[trigger]]
name = "install-packages-darwin"
script = "scripts/darwin/install-packages.sh"
watch = [".Brewfile"]
os = "darwin"

[[trigger]]
name = "build-rust-hooks"
script = "scripts/build-rust-hooks.sh"
watch = [".claude/hooks/*/Cargo.toml"]
role = "dev"
"#;
        let file: TriggersFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.trigger.len(), 2);
        assert_eq!(file.trigger[0].name, "install-packages-darwin");
        assert_eq!(file.trigger[0].os, Some("darwin".to_string()));
        assert_eq!(file.trigger[1].role, Some("dev".to_string()));
        assert_eq!(file.trigger[1].watch, vec![".claude/hooks/*/Cargo.toml"]);
    }

    #[test]
    fn test_load_config_missing_machine() {
        let dir = tempfile::tempdir().unwrap();

        let fleet_path = dir.path().join("fleet.toml");
        let local_path = dir.path().join("local.toml");
        let triggers_path = dir.path().join("triggers.toml");

        std::fs::write(
            &fleet_path,
            r#"
[machines.darwin]
ssh_host = "darwin"
role = ["server"]
"#,
        )
        .unwrap();
        std::fs::write(&local_path, "machine = \"nonexistent\"\n").unwrap();

        let result = load_config_from(&fleet_path, &local_path, &triggers_path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("darwin"));
    }

    #[test]
    fn test_load_config_full() {
        let dir = tempfile::tempdir().unwrap();

        let fleet_path = dir.path().join("fleet.toml");
        let local_path = dir.path().join("local.toml");
        let triggers_path = dir.path().join("triggers.toml");

        std::fs::write(
            &fleet_path,
            r#"
[machines.mac-mini]
ssh_host = "localhost"
role = ["dev", "primary"]

[templates]
source_dir = "~/dotfiles/templates"

[secrets]
source_dir = "~/dotfiles/secrets"
"#,
        )
        .unwrap();
        std::fs::write(&local_path, "machine = \"mac-mini\"\n").unwrap();
        std::fs::write(
            &triggers_path,
            r#"
[[trigger]]
name = "test-trigger"
script = "scripts/test.sh"
watch = ["test.txt"]
"#,
        )
        .unwrap();

        let config = load_config_from(&fleet_path, &local_path, &triggers_path).unwrap();
        assert_eq!(config.machine_name, "mac-mini");
        assert!(config.has_role("dev"));
        assert!(!config.has_role("server"));
        assert_eq!(config.triggers.len(), 1);
        assert_eq!(config.triggers[0].name, "test-trigger");
    }

    #[test]
    fn test_load_config_missing_triggers_ok() {
        let dir = tempfile::tempdir().unwrap();

        let fleet_path = dir.path().join("fleet.toml");
        let local_path = dir.path().join("local.toml");
        let triggers_path = dir.path().join("triggers.toml"); // doesn't exist

        std::fs::write(
            &fleet_path,
            r#"
[machines.mac-mini]
ssh_host = "localhost"
"#,
        )
        .unwrap();
        std::fs::write(&local_path, "machine = \"mac-mini\"\n").unwrap();

        let config = load_config_from(&fleet_path, &local_path, &triggers_path).unwrap();
        assert_eq!(config.triggers.len(), 0);
    }

    #[test]
    fn test_applicable_triggers_filter() {
        let dir = tempfile::tempdir().unwrap();

        let fleet_path = dir.path().join("fleet.toml");
        let local_path = dir.path().join("local.toml");
        let triggers_path = dir.path().join("triggers.toml");

        std::fs::write(
            &fleet_path,
            r#"
[machines.mac-mini]
ssh_host = "localhost"
role = ["dev"]
"#,
        )
        .unwrap();
        std::fs::write(&local_path, "machine = \"mac-mini\"\n").unwrap();
        std::fs::write(
            &triggers_path,
            r#"
[[trigger]]
name = "always"
script = "scripts/always.sh"

[[trigger]]
name = "linux-only"
script = "scripts/linux.sh"
os = "linux"

[[trigger]]
name = "dev-only"
script = "scripts/dev.sh"
role = "dev"

[[trigger]]
name = "server-only"
script = "scripts/server.sh"
role = "server"
"#,
        )
        .unwrap();

        let config = load_config_from(&fleet_path, &local_path, &triggers_path).unwrap();
        let applicable = config.applicable_triggers();
        let names: Vec<&str> = applicable.iter().map(|t| t.name.as_str()).collect();

        // "always" passes (no filter)
        assert!(names.contains(&"always"));
        // "dev-only" passes (mac-mini has dev role)
        assert!(names.contains(&"dev-only"));
        // "server-only" filtered out (mac-mini lacks server role)
        assert!(!names.contains(&"server-only"));
        // "linux-only" filtered on macOS (os = "linux" vs current os)
        if std::env::consts::OS == "macos" {
            assert!(!names.contains(&"linux-only"));
        }
    }

    #[test]
    fn test_fleet_toml_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let fleet_path = dir.path().join("fleet.toml");
        std::fs::write(&fleet_path, "this is not valid toml [[[").unwrap();

        let result = load_fleet(&fleet_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse error"));
    }

    #[test]
    fn test_missing_fleet_toml() {
        let result = load_fleet(Path::new("/nonexistent/fleet.toml"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cannot read fleet.toml"));
        assert!(err.contains("nit bootstrap"));
    }
}
