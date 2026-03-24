//! Template rendering (tera) — renders ~/dotfiles/templates/*.tmpl to target paths
//!
//! Path mapping: directory structure mirrors target path
//!   templates/.zshenv.tmpl → ~/.zshenv
//!   templates/Library/LaunchAgents/foo.plist.tmpl → ~/Library/LaunchAgents/foo.plist

use crate::config::{expand_tilde, NitConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// A template mapping: source .tmpl file → target path in $HOME
#[derive(Debug, Clone)]
pub struct TemplateMapping {
    /// Absolute path to the .tmpl source file
    pub source: PathBuf,
    /// Absolute path to the rendered target file in $HOME
    pub target: PathBuf,
    /// Relative path from templates dir (e.g., ".zshenv.tmpl")
    pub rel_source: PathBuf,
}

/// Discover all templates and build source→target and target→source mappings
pub fn discover_templates(config: &NitConfig) -> Vec<TemplateMapping> {
    let templates_dir = &config.templates_dir;
    let home = dirs::home_dir().expect("cannot determine home directory");

    if !templates_dir.exists() {
        return Vec::new();
    }

    let mut mappings = Vec::new();

    for entry in WalkDir::new(templates_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let source = entry.path().to_path_buf();
        let ext = source.extension().and_then(|e| e.to_str());

        if ext != Some("tmpl") {
            continue;
        }

        // Strip templates_dir prefix and .tmpl suffix to get target relative path
        if let Ok(rel) = source.strip_prefix(templates_dir) {
            let rel_str = rel.to_string_lossy();
            // Remove .tmpl extension
            if let Some(target_rel) = rel_str.strip_suffix(".tmpl") {
                let target = home.join(target_rel);
                mappings.push(TemplateMapping {
                    source: source.clone(),
                    target,
                    rel_source: rel.to_path_buf(),
                });
            }
        }
    }

    mappings
}

/// Build a reverse lookup: target path → template source path
pub fn build_target_to_source_map(mappings: &[TemplateMapping]) -> HashMap<PathBuf, PathBuf> {
    mappings
        .iter()
        .map(|m| (m.target.clone(), m.source.clone()))
        .collect()
}

/// Check if a given path is a template target, return the source if so
pub fn resolve_template_target(
    path: &Path,
    target_to_source: &HashMap<PathBuf, PathBuf>,
) -> Option<PathBuf> {
    // Canonicalize for comparison (resolve symlinks, normalize)
    // Fall back to the raw path if canonicalize fails (file might not exist yet)
    let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Direct lookup
    if let Some(source) = target_to_source.get(&normalized) {
        return Some(source.clone());
    }

    // Try with tilde expansion
    let path_str = path.to_string_lossy();
    if path_str.starts_with("~/") || path_str == "~" {
        let expanded = expand_tilde(&path_str);
        if let Some(source) = target_to_source.get(&expanded) {
            return Some(source.clone());
        }
    }

    // Try matching against all targets (handles path normalization edge cases)
    for (target, source) in target_to_source {
        let target_canon = target.canonicalize().unwrap_or_else(|_| target.clone());
        if target_canon == normalized {
            return Some(source.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_mapping_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(templates_dir.join("Library/LaunchAgents")).unwrap();

        // Create test template files
        std::fs::write(templates_dir.join(".zshenv.tmpl"), "# test").unwrap();
        std::fs::write(
            templates_dir.join("Library/LaunchAgents/foo.plist.tmpl"),
            "<!-- test -->",
        )
        .unwrap();
        // Non-template file should be ignored
        std::fs::write(templates_dir.join("README.md"), "not a template").unwrap();

        let home = dirs::home_dir().unwrap();

        // Build a minimal config just for the templates_dir
        let config = NitConfig {
            fleet: crate::config::FleetConfig {
                machines: Default::default(),
                templates: crate::config::TemplatesConfig {
                    source_dir: templates_dir.to_string_lossy().to_string(),
                },
                secrets: Default::default(),
                permissions: Default::default(),
                exclude: Default::default(),
                sync: None,
            },
            local: crate::config::LocalConfig {
                machine: "test".to_string(),
                identity: "~/.config/nit/age-key.txt".to_string(),
                git: Default::default(),
            },
            machine_name: "test".to_string(),
            machine: crate::config::MachineConfig {
                ssh_host: "localhost".to_string(),
                role: vec![],
                critical: false,
            },
            triggers: vec![],
            templates_dir: templates_dir.clone(),
            secrets_dir: dir.path().join("secrets"),
            project_dir: dir.path().to_path_buf(),
        };

        let mappings = discover_templates(&config);
        assert_eq!(mappings.len(), 2);

        let targets: Vec<PathBuf> = mappings.iter().map(|m| m.target.clone()).collect();
        assert!(targets.contains(&home.join(".zshenv")));
        assert!(targets.contains(&home.join("Library/LaunchAgents/foo.plist")));
    }

    #[test]
    fn test_target_to_source_resolution() {
        let home = dirs::home_dir().unwrap();
        let source = PathBuf::from("/tmp/dotfiles/templates/.zshenv.tmpl");
        let target = home.join(".zshenv");

        let mut map = HashMap::new();
        map.insert(target.clone(), source.clone());

        // Direct path match
        assert_eq!(
            resolve_template_target(&target, &map),
            Some(source.clone())
        );

        // Tilde path match
        assert_eq!(
            resolve_template_target(Path::new("~/.zshenv"), &map),
            Some(source.clone())
        );

        // Non-template path
        assert_eq!(
            resolve_template_target(Path::new("~/.bashrc"), &map),
            None
        );
    }
}
