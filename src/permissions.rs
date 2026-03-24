//! File permission management — sets modes on private files, secrets, etc.
//!
//! Walks `[permissions]` config and sets file modes on ALL matched files
//! (templates + plain). Git doesn't track permissions beyond executable bit;
//! nit fills this gap.

use crate::config::{expand_tilde, NitConfig};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// Result of applying permissions to a file
#[derive(Debug)]
pub struct PermissionResult {
    pub path: PathBuf,
    pub mode: u32,
    pub status: PermissionStatus,
}

#[derive(Debug, PartialEq)]
pub enum PermissionStatus {
    Set,
    AlreadyCorrect,
    Skipped(String),
    Error(String),
}

/// Apply permissions from config to all matching files.
/// Called as part of every deploy cycle (idempotent).
pub fn apply_permissions(config: &NitConfig) -> Vec<PermissionResult> {
    let mut results = Vec::new();

    for pattern in &config.fleet.permissions.private {
        let expanded = expand_tilde(pattern);
        let pattern_str = expanded.to_string_lossy().to_string();

        match glob::glob(&pattern_str) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if entry.is_file() {
                        results.push(set_private(&entry));
                    }
                }
            }
            Err(e) => {
                results.push(PermissionResult {
                    path: expanded,
                    mode: 0o600,
                    status: PermissionStatus::Error(format!("invalid glob: {}", e)),
                });
            }
        }
    }

    results
}

/// Set a file to 0600 (owner read/write only)
fn set_private(path: &PathBuf) -> PermissionResult {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let current = meta.permissions().mode() & 0o777;
            if current == 0o600 {
                return PermissionResult {
                    path: path.clone(),
                    mode: 0o600,
                    status: PermissionStatus::AlreadyCorrect,
                };
            }
            match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
                Ok(()) => PermissionResult {
                    path: path.clone(),
                    mode: 0o600,
                    status: PermissionStatus::Set,
                },
                Err(e) => PermissionResult {
                    path: path.clone(),
                    mode: 0o600,
                    status: PermissionStatus::Error(format!("chmod failed: {}", e)),
                },
            }
        }
        Err(e) => PermissionResult {
            path: path.clone(),
            mode: 0o600,
            status: PermissionStatus::Error(format!("cannot stat: {}", e)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_set_private_from_world_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.env");
        fs::write(&path, "API_KEY=abc").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let result = set_private(&path);
        assert_eq!(result.status, PermissionStatus::Set);

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[test]
    fn test_set_private_already_correct() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("already-private.txt");
        fs::write(&path, "secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let result = set_private(&path);
        assert_eq!(result.status, PermissionStatus::AlreadyCorrect);
    }

    #[test]
    fn test_set_private_nonexistent() {
        let path = PathBuf::from("/nonexistent/file.txt");
        let result = set_private(&path);
        assert!(matches!(result.status, PermissionStatus::Error(_)));
    }

    #[test]
    fn test_apply_permissions_with_glob() {
        let dir = tempfile::tempdir().unwrap();
        let secrets_dir = dir.path().join(".secrets");
        fs::create_dir_all(&secrets_dir).unwrap();

        // Create files with wrong permissions
        for name in &["tier-all.env", "tier-servers.env"] {
            let path = secrets_dir.join(name);
            fs::write(&path, "secret").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }

        // Build minimal config
        let config = crate::config::NitConfig {
            fleet: crate::config::FleetConfig {
                machines: Default::default(),
                templates: Default::default(),
                secrets: Default::default(),
                permissions: crate::config::PermissionsConfig {
                    private: vec![format!("{}/*", secrets_dir.display())],
                },
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
            templates_dir: dir.path().join("templates"),
            secrets_dir: dir.path().join("secrets"),
            project_dir: dir.path().to_path_buf(),
        };

        let results = apply_permissions(&config);
        assert_eq!(results.len(), 2);
        for r in &results {
            assert_eq!(r.status, PermissionStatus::Set);
        }

        // Verify permissions actually set
        for name in &["tier-all.env", "tier-servers.env"] {
            let perms = fs::metadata(secrets_dir.join(name)).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn test_apply_permissions_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.txt");
        fs::write(&path, "secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let config = crate::config::NitConfig {
            fleet: crate::config::FleetConfig {
                machines: Default::default(),
                templates: Default::default(),
                secrets: Default::default(),
                permissions: crate::config::PermissionsConfig {
                    private: vec![path.display().to_string()],
                },
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
            templates_dir: dir.path().join("templates"),
            secrets_dir: dir.path().join("secrets"),
            project_dir: dir.path().to_path_buf(),
        };

        // Run twice — second run should all be AlreadyCorrect
        apply_permissions(&config);
        let results = apply_permissions(&config);
        for r in &results {
            assert_eq!(r.status, PermissionStatus::AlreadyCorrect);
        }
    }
}
