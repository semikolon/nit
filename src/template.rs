//! Template rendering (tera) — renders ~/dotfiles/templates/*.tmpl to target paths
//!
//! Path mapping: directory structure mirrors target path
//!   templates/.zshenv.tmpl → ~/.zshenv
//!   templates/Library/LaunchAgents/foo.plist.tmpl → ~/Library/LaunchAgents/foo.plist

use crate::config::{NitConfig, expand_tilde};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tera::{Context, Tera};
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

/// Result of rendering a single template
#[derive(Debug)]
#[allow(dead_code)]
pub struct RenderResult {
    /// The template mapping that was rendered
    pub mapping: TemplateMapping,
    /// Ok(rendered_string) or Err(error_message)
    pub result: Result<String, String>,
}

/// Render a single template with the given config context
pub fn render_template(
    mapping: &TemplateMapping,
    config: &NitConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    let template_content = std::fs::read_to_string(&mapping.source)
        .map_err(|e| format!("cannot read template {}: {}", mapping.source.display(), e))?;

    let mut context = Context::new();

    // Machine identity
    // Use crate::config::current_os() to normalize "macos" → "darwin"
    // (matches chezmoi/Unix uname convention; trigger filters use the same).
    context.insert("hostname", &config.machine_name);
    context.insert("os", crate::config::current_os());
    context.insert("arch", std::env::consts::ARCH);

    // Roles as array
    context.insert("role", &config.machine.role);

    // is_<role> booleans for each role this machine has
    for role in &config.machine.role {
        context.insert(format!("is_{}", role), &true);
    }

    // Home directory
    let home = dirs::home_dir().expect("cannot determine home directory");
    context.insert("home_dir", &home.to_string_lossy().to_string());

    // Render using one_off (no template directory needed)
    let rendered = Tera::one_off(&template_content, &context, false)
        .map_err(|e| format!("template error in {}: {}", mapping.rel_source.display(), e))?;

    Ok(rendered)
}

/// Render all discovered templates, returning results for each (partial success supported)
#[allow(dead_code)]
pub fn render_all(config: &NitConfig) -> Vec<RenderResult> {
    let mappings = discover_templates(config);

    mappings
        .into_iter()
        .map(|mapping| {
            let result = render_template(&mapping, config).map_err(|e| e.to_string());
            RenderResult { mapping, result }
        })
        .collect()
}

/// Return the appropriate "managed by nit" comment for a file type
pub fn warning_comment(file_path: &Path) -> Option<String> {
    let source_hint = file_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    // Check extension first
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    // Also check the full filename for dotfiles without extensions
    let filename = file_path.file_name().and_then(|f| f.to_str()).unwrap_or("");

    match ext {
        // Hash-style comments
        "sh" | "zsh" | "bash" | "py" | "rb" | "toml" | "yaml" | "yml" | "conf" | "env" => Some(
            format!("# Managed by nit — edit templates/{} instead", source_hint),
        ),
        // XML-style comments
        "plist" | "xml" | "html" | "svg" => Some(format!(
            "<!-- Managed by nit — edit templates/{} instead -->",
            source_hint
        )),
        // JSON has no comments
        "json" => None,
        // Check dotfile names that have no extension
        _ => match filename {
            ".zshenv" | ".zshrc" | ".zprofile" | ".bashrc" | ".bash_profile" | ".gitconfig" => {
                Some(format!(
                    "# Managed by nit — edit templates/{} instead",
                    source_hint
                ))
            }
            _ => {
                // Default: hash comment
                Some(format!(
                    "# Managed by nit — edit templates/{} instead",
                    source_hint
                ))
            }
        },
    }
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
        assert_eq!(resolve_template_target(&target, &map), Some(source.clone()));

        // Tilde path match
        assert_eq!(
            resolve_template_target(Path::new("~/.zshenv"), &map),
            Some(source.clone())
        );

        // Non-template path
        assert_eq!(resolve_template_target(Path::new("~/.bashrc"), &map), None);
    }

    /// Helper to build a test NitConfig with a given templates_dir
    fn test_config(templates_dir: PathBuf, project_dir: PathBuf) -> NitConfig {
        NitConfig {
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
            templates_dir,
            secrets_dir: project_dir.join("secrets"),
            project_dir,
        }
    }

    #[test]
    fn test_empty_templates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(&templates_dir).unwrap();

        let config = test_config(templates_dir, dir.path().to_path_buf());
        let mappings = discover_templates(&config);
        assert!(mappings.is_empty());
    }

    #[test]
    fn test_nonexistent_templates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates"); // not created

        let config = test_config(templates_dir, dir.path().to_path_buf());
        let mappings = discover_templates(&config);
        assert!(mappings.is_empty());
    }

    #[test]
    fn test_deeply_nested_template_path() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(templates_dir.join(".config/deeply/nested/path")).unwrap();
        std::fs::write(
            templates_dir.join(".config/deeply/nested/path/config.toml.tmpl"),
            "# deep",
        )
        .unwrap();

        let home = dirs::home_dir().unwrap();
        let config = test_config(templates_dir, dir.path().to_path_buf());
        let mappings = discover_templates(&config);
        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].target,
            home.join(".config/deeply/nested/path/config.toml")
        );
    }

    #[test]
    fn test_file_with_multiple_dots_in_name() {
        // e.g., "com.example.my-daemon.plist.tmpl"
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(templates_dir.join("Library/LaunchAgents")).unwrap();
        std::fs::write(
            templates_dir.join("Library/LaunchAgents/com.example.daemon.plist.tmpl"),
            "<!-- plist -->",
        )
        .unwrap();

        let home = dirs::home_dir().unwrap();
        let config = test_config(templates_dir, dir.path().to_path_buf());
        let mappings = discover_templates(&config);
        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].target,
            home.join("Library/LaunchAgents/com.example.daemon.plist")
        );
    }

    #[test]
    fn test_non_tmpl_files_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(&templates_dir).unwrap();

        // Various non-.tmpl files that should all be ignored
        std::fs::write(templates_dir.join("README.md"), "docs").unwrap();
        std::fs::write(templates_dir.join(".gitkeep"), "").unwrap();
        std::fs::write(templates_dir.join("config.toml"), "not a template").unwrap();
        std::fs::write(templates_dir.join("script.sh"), "#!/bin/bash").unwrap();
        // Only this one should be discovered
        std::fs::write(templates_dir.join(".zshrc.tmpl"), "# shell").unwrap();

        let config = test_config(templates_dir, dir.path().to_path_buf());
        let mappings = discover_templates(&config);
        assert_eq!(mappings.len(), 1);
        assert!(mappings[0].source.to_string_lossy().contains(".zshrc.tmpl"));
    }

    #[test]
    fn test_build_target_to_source_map() {
        let home = dirs::home_dir().unwrap();
        let mappings = vec![
            TemplateMapping {
                source: PathBuf::from("/dotfiles/templates/.zshenv.tmpl"),
                target: home.join(".zshenv"),
                rel_source: PathBuf::from(".zshenv.tmpl"),
            },
            TemplateMapping {
                source: PathBuf::from("/dotfiles/templates/.zprofile.tmpl"),
                target: home.join(".zprofile"),
                rel_source: PathBuf::from(".zprofile.tmpl"),
            },
        ];

        let map = build_target_to_source_map(&mappings);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map[&home.join(".zshenv")],
            PathBuf::from("/dotfiles/templates/.zshenv.tmpl")
        );
    }

    #[test]
    fn test_resolve_template_target_absolute_path() {
        let home = dirs::home_dir().unwrap();
        let source = PathBuf::from("/tmp/templates/.zshenv.tmpl");
        let target = home.join(".zshenv");

        let mut map = HashMap::new();
        map.insert(target.clone(), source.clone());

        // Absolute path should match
        let abs_target = home.join(".zshenv");
        assert_eq!(
            resolve_template_target(&abs_target, &map),
            Some(source.clone())
        );
    }

    #[test]
    fn test_resolve_template_target_no_match() {
        let map = HashMap::new(); // empty map
        assert_eq!(resolve_template_target(Path::new("/any/path"), &map), None);
        assert_eq!(resolve_template_target(Path::new("~/.zshrc"), &map), None);
    }

    #[test]
    fn test_multiple_templates_unique_targets() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(&templates_dir).unwrap();

        // Create several templates
        for name in &[".zshenv", ".zprofile", ".gitconfig"] {
            std::fs::write(templates_dir.join(format!("{}.tmpl", name)), "# content").unwrap();
        }

        let config = test_config(templates_dir, dir.path().to_path_buf());
        let mappings = discover_templates(&config);
        assert_eq!(mappings.len(), 3);

        // All targets should be unique
        let targets: std::collections::HashSet<PathBuf> =
            mappings.iter().map(|m| m.target.clone()).collect();
        assert_eq!(targets.len(), 3);

        // All sources should be unique
        let sources: std::collections::HashSet<PathBuf> =
            mappings.iter().map(|m| m.source.clone()).collect();
        assert_eq!(sources.len(), 3);
    }

    #[test]
    fn test_rel_source_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(templates_dir.join("Library/LaunchAgents")).unwrap();

        std::fs::write(
            templates_dir.join("Library/LaunchAgents/foo.plist.tmpl"),
            "<!-- test -->",
        )
        .unwrap();

        let config = test_config(templates_dir, dir.path().to_path_buf());
        let mappings = discover_templates(&config);
        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].rel_source,
            PathBuf::from("Library/LaunchAgents/foo.plist.tmpl")
        );
    }

    // --- Template rendering tests ---

    /// Helper: create a single-template config with roles and a template file
    fn render_test_setup(
        template_content: &str,
        filename: &str,
        roles: Vec<&str>,
    ) -> (tempfile::TempDir, NitConfig, TemplateMapping) {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(&templates_dir).unwrap();

        let tmpl_file = templates_dir.join(filename);
        std::fs::write(&tmpl_file, template_content).unwrap();

        let home = dirs::home_dir().unwrap();
        let target_name = filename.strip_suffix(".tmpl").unwrap();

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
                machine: "test-host".to_string(),
                identity: "~/.config/nit/age-key.txt".to_string(),
                git: Default::default(),
            },
            machine_name: "test-host".to_string(),
            machine: crate::config::MachineConfig {
                ssh_host: "localhost".to_string(),
                role: roles.into_iter().map(|s| s.to_string()).collect(),
                critical: false,
            },
            triggers: vec![],
            templates_dir: templates_dir.clone(),
            secrets_dir: dir.path().join("secrets"),
            project_dir: dir.path().to_path_buf(),
        };

        let mapping = TemplateMapping {
            source: tmpl_file,
            target: home.join(target_name),
            rel_source: PathBuf::from(filename),
        };

        (dir, config, mapping)
    }

    #[test]
    fn test_render_hostname_os_arch() {
        let (_dir, config, mapping) = render_test_setup(
            "host={{ hostname }} os={{ os }} arch={{ arch }}",
            ".test.conf.tmpl",
            vec![],
        );

        let rendered = render_template(&mapping, &config).unwrap();
        assert!(rendered.contains("host=test-host"));
        // Templates see the normalized OS name (macos → darwin)
        assert!(rendered.contains(&format!("os={}", crate::config::current_os())));
        assert!(rendered.contains(&format!("arch={}", std::env::consts::ARCH)));
    }

    #[test]
    fn test_render_role_array_and_is_booleans() {
        let (_dir, config, mapping) = render_test_setup(
            "{% for r in role %}{{ r }}\n{% endfor %}dev={{ is_dev }} server={{ is_server }}",
            ".roles.conf.tmpl",
            vec!["dev", "server"],
        );

        let rendered = render_template(&mapping, &config).unwrap();
        assert!(rendered.contains("dev"));
        assert!(rendered.contains("server"));
        assert!(rendered.contains("dev=true"));
        assert!(rendered.contains("server=true"));
    }

    #[test]
    fn test_render_conditional_blocks() {
        // Use the normalized OS constant (matches what templates see)
        let current_os = crate::config::current_os();
        let template = format!(
            r#"{{% if os == "{}" %}}matched{{% else %}}other{{% endif %}}"#,
            current_os
        );
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(&templates_dir).unwrap();
        std::fs::write(templates_dir.join(".os.conf.tmpl"), &template).unwrap();

        let home = dirs::home_dir().unwrap();
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
                machine: "test-host".to_string(),
                identity: "~/.config/nit/age-key.txt".to_string(),
                git: Default::default(),
            },
            machine_name: "test-host".to_string(),
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

        let mapping = TemplateMapping {
            source: templates_dir.join(".os.conf.tmpl"),
            target: home.join(".os.conf"),
            rel_source: PathBuf::from(".os.conf.tmpl"),
        };

        let rendered = render_template(&mapping, &config).unwrap();
        assert_eq!(rendered, "matched");
    }

    #[test]
    fn test_render_conditional_role_check() {
        let template = "{% if is_dev is defined and is_dev %}DEV MODE{% else %}PROD{% endif %}";
        let (_dir, config, mapping) = render_test_setup(template, ".mode.conf.tmpl", vec!["dev"]);

        let rendered = render_template(&mapping, &config).unwrap();
        assert_eq!(rendered, "DEV MODE");
    }

    #[test]
    fn test_render_conditional_role_absent() {
        let template = "{% if is_dev is defined and is_dev %}DEV MODE{% else %}PROD{% endif %}";
        let (_dir, config, mapping) =
            render_test_setup(template, ".mode.conf.tmpl", vec!["server"]);

        let rendered = render_template(&mapping, &config).unwrap();
        assert_eq!(rendered, "PROD");
    }

    #[test]
    fn test_render_home_dir() {
        let (_dir, config, mapping) =
            render_test_setup("home={{ home_dir }}", ".home.conf.tmpl", vec![]);

        let rendered = render_template(&mapping, &config).unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(rendered, format!("home={}", home.display()));
    }

    #[test]
    fn test_render_template_syntax_error() {
        let (_dir, config, mapping) =
            render_test_setup("{% if unclosed", ".broken.conf.tmpl", vec![]);

        let err = render_template(&mapping, &config).unwrap_err();
        let err_str = err.to_string();
        // Error should mention the file path
        assert!(
            err_str.contains(".broken.conf.tmpl"),
            "error should contain file path, got: {}",
            err_str
        );
    }

    #[test]
    fn test_render_all_partial_success() {
        let dir = tempfile::tempdir().unwrap();
        let templates_dir = dir.path().join("templates");
        std::fs::create_dir_all(&templates_dir).unwrap();

        // Two good templates
        std::fs::write(
            templates_dir.join(".good1.conf.tmpl"),
            "host={{ hostname }}",
        )
        .unwrap();
        std::fs::write(templates_dir.join(".good2.conf.tmpl"), "arch={{ arch }}").unwrap();
        // One broken template
        std::fs::write(templates_dir.join(".broken.conf.tmpl"), "{% if unclosed").unwrap();

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
                machine: "test-host".to_string(),
                identity: "~/.config/nit/age-key.txt".to_string(),
                git: Default::default(),
            },
            machine_name: "test-host".to_string(),
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

        let results = render_all(&config);
        assert_eq!(results.len(), 3);

        let ok_count = results.iter().filter(|r| r.result.is_ok()).count();
        let err_count = results.iter().filter(|r| r.result.is_err()).count();
        assert_eq!(ok_count, 2);
        assert_eq!(err_count, 1);

        // The broken one should have an error mentioning the file
        let broken = results.iter().find(|r| r.result.is_err()).unwrap();
        assert!(
            broken
                .result
                .as_ref()
                .unwrap_err()
                .contains(".broken.conf.tmpl"),
        );
    }

    #[test]
    fn test_render_empty_template() {
        let (_dir, config, mapping) = render_test_setup("", ".empty.conf.tmpl", vec![]);

        let rendered = render_template(&mapping, &config).unwrap();
        assert_eq!(rendered, "");
    }

    // --- Warning comment tests ---

    #[test]
    fn test_warning_comment_hash_by_extension() {
        for ext in &[
            "sh", "zsh", "bash", "py", "rb", "toml", "yaml", "yml", "conf", "env",
        ] {
            let path = PathBuf::from(format!("test.{}", ext));
            let comment = warning_comment(&path);
            assert!(comment.is_some(), "expected comment for .{} extension", ext);
            assert!(
                comment.as_ref().unwrap().starts_with("# Managed by nit"),
                "expected hash comment for .{}, got: {:?}",
                ext,
                comment
            );
        }
    }

    #[test]
    fn test_warning_comment_xml_style() {
        for ext in &["plist", "xml", "html", "svg"] {
            let path = PathBuf::from(format!("test.{}", ext));
            let comment = warning_comment(&path);
            assert!(comment.is_some(), "expected comment for .{} extension", ext);
            assert!(
                comment.as_ref().unwrap().starts_with("<!-- Managed by nit"),
                "expected XML comment for .{}, got: {:?}",
                ext,
                comment
            );
        }
    }

    #[test]
    fn test_warning_comment_json_none() {
        let path = PathBuf::from("config.json");
        assert!(warning_comment(&path).is_none());
    }

    #[test]
    fn test_warning_comment_dotfiles_no_extension() {
        for name in &[
            ".zshenv",
            ".zshrc",
            ".zprofile",
            ".bashrc",
            ".bash_profile",
            ".gitconfig",
        ] {
            let path = PathBuf::from(name);
            let comment = warning_comment(&path);
            assert!(comment.is_some(), "expected comment for {}", name);
            assert!(
                comment.as_ref().unwrap().starts_with("# Managed by nit"),
                "expected hash comment for {}, got: {:?}",
                name,
                comment
            );
        }
    }

    #[test]
    fn test_warning_comment_unknown_defaults_to_hash() {
        let path = PathBuf::from("somefile.xyz");
        let comment = warning_comment(&path);
        assert!(comment.is_some());
        assert!(comment.unwrap().starts_with("# Managed by nit"));
    }

    #[test]
    fn test_warning_comment_includes_source_filename() {
        let path = PathBuf::from("config.toml");
        let comment = warning_comment(&path).unwrap();
        assert!(comment.contains("config.toml"));
    }
}
