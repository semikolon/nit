//! Age encryption/decryption for tiered secrets

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::config::{NitConfig, expand_tilde};

/// Result of deploying a single secret file
#[derive(Debug)]
pub struct SecretResult {
    pub tier: String,
    pub target: String,
    pub status: DeployStatus,
}

/// Status of a secret deployment
#[derive(Debug)]
pub enum DeployStatus {
    Deployed,
    Skipped(String),
    Error(String),
}

/// Encrypt a plaintext file to one or more age recipients, writing to output_path.
pub fn encrypt_file(
    plaintext_path: &Path,
    recipients: &[String],
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if recipients.is_empty() {
        return Err("no recipients provided".into());
    }

    let plaintext = fs::read(plaintext_path).map_err(|e| {
        format!(
            "cannot read plaintext file {}: {}",
            plaintext_path.display(),
            e
        )
    })?;

    let parsed_recipients: Vec<age::x25519::Recipient> = recipients
        .iter()
        .map(|r| {
            r.parse::<age::x25519::Recipient>()
                .map_err(|e| format!("invalid recipient '{}': {}", r, e))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let recipients_refs: Vec<&dyn age::Recipient> = parsed_recipients
        .iter()
        .map(|r| r as &dyn age::Recipient)
        .collect();

    let encryptor = age::Encryptor::with_recipients(recipients_refs.into_iter())
        .expect("we provided recipients");

    let mut ciphertext = Vec::with_capacity(plaintext.len() + 1024);
    let mut writer = encryptor.wrap_output(&mut ciphertext)?;
    writer.write_all(&plaintext)?;
    writer.finish()?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, &ciphertext)?;

    Ok(())
}

/// Decrypt an age-encrypted file using an identity (private key) file.
/// Returns the plaintext as a String.
pub fn decrypt_file(
    encrypted_path: &Path,
    identity_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let identity_file = age::IdentityFile::from_file(
        identity_path
            .to_str()
            .ok_or("identity path is not valid UTF-8")?
            .to_string(),
    )
    .map_err(|e| {
        format!(
            "cannot read identity file {}: {}",
            identity_path.display(),
            e
        )
    })?;

    let identities = identity_file
        .into_identities()
        .map_err(|e| format!("cannot parse identities: {}", e))?;

    let ciphertext = fs::read(encrypted_path).map_err(|e| {
        format!(
            "cannot read encrypted file {}: {}",
            encrypted_path.display(),
            e
        )
    })?;

    // Auto-detect armor: chezmoi (and `age -a` / `age --armor`) writes
    // ASCII-armored age files starting with `-----BEGIN AGE ENCRYPTED FILE-----`.
    // age::Decryptor::new_buffered expects BINARY format. Wrap with
    // ArmoredReader which transparently handles both armored and binary input.
    let armored = age::armor::ArmoredReader::new(&ciphertext[..]);
    let decryptor = age::Decryptor::new_buffered(armored)
        .map_err(|e| format!("cannot create decryptor: {}", e))?;

    let identity_refs: Vec<&dyn age::Identity> = identities.iter().map(|i| i.as_ref()).collect();

    let mut reader = decryptor
        .decrypt(identity_refs.into_iter())
        .map_err(|e| format!("decryption failed: {}", e))?;

    let mut plaintext = Vec::new();
    reader.read_to_end(&mut plaintext)?;

    String::from_utf8(plaintext)
        .map_err(|e| format!("decrypted content is not valid UTF-8: {}", e).into())
}

/// Decrypt an age-encrypted file and write the plaintext to a target path
/// with 0600 permissions. Creates parent directories if needed.
pub fn decrypt_to_target(
    encrypted_path: &Path,
    target_path: &Path,
    identity_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let plaintext = decrypt_file(encrypted_path, identity_path)?;

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(target_path, &plaintext)?;
    fs::set_permissions(target_path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

/// Re-encrypt a file with new recipients. Decrypts with current identity,
/// then re-encrypts with the new recipient list. Uses atomic write (temp + rename).
pub fn rekey_file(
    encrypted_path: &Path,
    identity_path: &Path,
    new_recipients: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let plaintext = decrypt_file(encrypted_path, identity_path)?;

    // Write to a temp file in the same directory, then rename for atomicity
    let parent = encrypted_path.parent().unwrap_or(Path::new("."));
    let temp_path = parent.join(format!(".nit-rekey-{}.tmp", std::process::id()));

    // Encrypt plaintext to new recipients, writing to temp file
    let parsed_recipients: Vec<age::x25519::Recipient> = new_recipients
        .iter()
        .map(|r| {
            r.parse::<age::x25519::Recipient>()
                .map_err(|e| format!("invalid recipient '{}': {}", r, e))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let recipients_refs: Vec<&dyn age::Recipient> = parsed_recipients
        .iter()
        .map(|r| r as &dyn age::Recipient)
        .collect();

    let encryptor = age::Encryptor::with_recipients(recipients_refs.into_iter())
        .expect("we provided recipients");

    let mut ciphertext = Vec::with_capacity(plaintext.len() + 1024);
    let mut writer = encryptor.wrap_output(&mut ciphertext)?;
    writer.write_all(plaintext.as_bytes())?;
    writer.finish()?;

    fs::write(&temp_path, &ciphertext)?;
    fs::rename(&temp_path, encrypted_path)?;

    Ok(())
}

/// Deploy secrets from the configured secrets directory to their target paths.
/// For each tier, checks if this machine's public key is among the recipients.
/// Returns results for each tier.
pub fn deploy_secrets(config: &NitConfig) -> Result<Vec<SecretResult>, Box<dyn std::error::Error>> {
    let identity_path = expand_tilde(&config.local.identity);
    let secrets_dir = &config.secrets_dir;

    // Read this machine's public key from identity file
    let machine_pubkey = read_public_key_from_identity(&identity_path)?;

    let mut results = Vec::new();

    for (tier_name, tier_config) in &config.fleet.secrets.tiers {
        let encrypted_filename = format!("{}.env.age", tier_name);
        let encrypted_path = secrets_dir.join(&encrypted_filename);
        let target_path = expand_tilde(&tier_config.target);

        // Check if this machine is authorized (its public key is in recipients)
        if !tier_config.recipients.contains(&machine_pubkey) {
            results.push(SecretResult {
                tier: tier_name.clone(),
                target: target_path.display().to_string(),
                status: DeployStatus::Skipped(format!(
                    "machine key not in recipients for tier '{}'",
                    tier_name
                )),
            });
            continue;
        }

        // Check if the encrypted file exists
        if !encrypted_path.exists() {
            results.push(SecretResult {
                tier: tier_name.clone(),
                target: target_path.display().to_string(),
                status: DeployStatus::Error(format!(
                    "encrypted file not found: {}",
                    encrypted_path.display()
                )),
            });
            continue;
        }

        // Decrypt and deploy
        match decrypt_to_target(&encrypted_path, &target_path, &identity_path) {
            Ok(()) => {
                results.push(SecretResult {
                    tier: tier_name.clone(),
                    target: target_path.display().to_string(),
                    status: DeployStatus::Deployed,
                });
            }
            Err(e) => {
                results.push(SecretResult {
                    tier: tier_name.clone(),
                    target: target_path.display().to_string(),
                    status: DeployStatus::Error(e.to_string()),
                });
            }
        }
    }

    Ok(results)
}

/// Read an identity file and derive the public key string from it.
fn read_public_key_from_identity(
    identity_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(identity_path).map_err(|e| {
        format!(
            "cannot read identity file {}: {}",
            identity_path.display(),
            e
        )
    })?;

    for line in content.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Ok(identity) = line.parse::<age::x25519::Identity>() {
            return Ok(identity.to_public().to_string());
        }
    }

    Err(format!("no valid age identity found in {}", identity_path.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::ExposeSecret;
    use std::fs;

    /// Helper: generate a keypair, write identity file, return (pubkey_string, identity_path)
    fn setup_keypair(dir: &Path) -> (String, std::path::PathBuf) {
        let key = age::x25519::Identity::generate();
        let pubkey = key.to_public().to_string();
        let secret = key.to_string();
        let identity_path = dir.join("age-key.txt");
        fs::write(
            &identity_path,
            format!(
                "# created by nit test\n# public key: {}\n{}\n",
                pubkey,
                secret.expose_secret()
            ),
        )
        .unwrap();
        (pubkey, identity_path)
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());

        let plaintext_path = dir.path().join("secret.txt");
        let encrypted_path = dir.path().join("secret.txt.age");
        fs::write(&plaintext_path, "hello world").unwrap();

        encrypt_file(&plaintext_path, &[pubkey], &encrypted_path).unwrap();
        assert!(encrypted_path.exists());

        let decrypted = decrypt_file(&encrypted_path, &identity_path).unwrap();
        assert_eq!(decrypted, "hello world");
    }

    #[test]
    fn encrypt_to_multiple_recipients() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey_a, identity_a) = setup_keypair(dir.path());

        // Second keypair in a subdirectory to avoid filename collision
        let subdir = dir.path().join("b");
        fs::create_dir(&subdir).unwrap();
        let (pubkey_b, identity_b) = setup_keypair(&subdir);

        let plaintext_path = dir.path().join("secret.txt");
        let encrypted_path = dir.path().join("secret.txt.age");
        fs::write(&plaintext_path, "multi-recipient secret").unwrap();

        encrypt_file(&plaintext_path, &[pubkey_a, pubkey_b], &encrypted_path).unwrap();

        // Both keys should decrypt
        let decrypted_a = decrypt_file(&encrypted_path, &identity_a).unwrap();
        assert_eq!(decrypted_a, "multi-recipient secret");

        let decrypted_b = decrypt_file(&encrypted_path, &identity_b).unwrap();
        assert_eq!(decrypted_b, "multi-recipient secret");
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, _identity) = setup_keypair(dir.path());

        // Second (wrong) keypair
        let subdir = dir.path().join("wrong");
        fs::create_dir(&subdir).unwrap();
        let (_wrong_pubkey, wrong_identity) = setup_keypair(&subdir);

        let plaintext_path = dir.path().join("secret.txt");
        let encrypted_path = dir.path().join("secret.txt.age");
        fs::write(&plaintext_path, "cannot read this").unwrap();

        encrypt_file(&plaintext_path, &[pubkey], &encrypted_path).unwrap();

        let result = decrypt_file(&encrypted_path, &wrong_identity);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_to_target_sets_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());

        let plaintext_path = dir.path().join("secret.txt");
        let encrypted_path = dir.path().join("secret.txt.age");
        let target_path = dir.path().join("deployed/secrets/secret.txt");

        fs::write(&plaintext_path, "secret content").unwrap();
        encrypt_file(&plaintext_path, &[pubkey], &encrypted_path).unwrap();

        decrypt_to_target(&encrypted_path, &target_path, &identity_path).unwrap();

        assert!(target_path.exists());
        assert_eq!(fs::read_to_string(&target_path).unwrap(), "secret content");

        let perms = fs::metadata(&target_path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[test]
    fn decrypt_to_target_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());

        let plaintext_path = dir.path().join("secret.txt");
        let encrypted_path = dir.path().join("secret.txt.age");
        let target_path = dir.path().join("a/b/c/secret.txt");

        fs::write(&plaintext_path, "deep secret").unwrap();
        encrypt_file(&plaintext_path, &[pubkey], &encrypted_path).unwrap();

        decrypt_to_target(&encrypted_path, &target_path, &identity_path).unwrap();
        assert_eq!(fs::read_to_string(&target_path).unwrap(), "deep secret");
    }

    #[test]
    fn rekey_file_works() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey_a, identity_a) = setup_keypair(dir.path());

        let subdir = dir.path().join("b");
        fs::create_dir(&subdir).unwrap();
        let (pubkey_b, identity_b) = setup_keypair(&subdir);

        let plaintext_path = dir.path().join("secret.txt");
        let encrypted_path = dir.path().join("secret.txt.age");
        fs::write(&plaintext_path, "rekey me").unwrap();

        // Encrypt with key A
        encrypt_file(&plaintext_path, &[pubkey_a.clone()], &encrypted_path).unwrap();

        // Verify A can decrypt
        let decrypted = decrypt_file(&encrypted_path, &identity_a).unwrap();
        assert_eq!(decrypted, "rekey me");

        // Rekey to key B only
        rekey_file(&encrypted_path, &identity_a, &[pubkey_b]).unwrap();

        // Key B can now decrypt
        let decrypted_b = decrypt_file(&encrypted_path, &identity_b).unwrap();
        assert_eq!(decrypted_b, "rekey me");

        // Key A can no longer decrypt
        let result_a = decrypt_file(&encrypted_path, &identity_a);
        assert!(result_a.is_err());
    }

    #[test]
    fn deploy_secrets_authorized_tier() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());

        // Create a secrets source directory with an encrypted tier file
        let secrets_dir = dir.path().join("secrets");
        fs::create_dir(&secrets_dir).unwrap();

        let plaintext_path = dir.path().join("tier-all.env");
        let encrypted_path = secrets_dir.join("tier-all.env.age");
        fs::write(&plaintext_path, "API_KEY=secret123").unwrap();
        encrypt_file(&plaintext_path, &[pubkey.clone()], &encrypted_path).unwrap();

        let target_path = dir.path().join("deployed/tier-all.env");

        // Build a minimal NitConfig
        let config = build_test_config(
            dir.path(),
            &identity_path,
            &secrets_dir,
            &[(
                "tier-all",
                &[pubkey.as_str()],
                target_path.to_str().unwrap(),
            )],
        );

        let results = deploy_secrets(&config).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, DeployStatus::Deployed));
        assert_eq!(
            fs::read_to_string(&target_path).unwrap(),
            "API_KEY=secret123"
        );
    }

    #[test]
    fn deploy_secrets_unauthorized_tier_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (_pubkey, identity_path) = setup_keypair(dir.path());

        let secrets_dir = dir.path().join("secrets");
        fs::create_dir(&secrets_dir).unwrap();

        let target_path = dir.path().join("deployed/tier-servers.env");

        // Config with a tier that uses a DIFFERENT pubkey (not ours)
        let config = build_test_config(
            dir.path(),
            &identity_path,
            &secrets_dir,
            &[(
                "tier-servers",
                &["age1notourkey"],
                target_path.to_str().unwrap(),
            )],
        );

        let results = deploy_secrets(&config).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, DeployStatus::Skipped(_)));
        assert!(!target_path.exists());
    }

    #[test]
    fn deploy_secrets_missing_encrypted_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());

        let secrets_dir = dir.path().join("secrets");
        fs::create_dir(&secrets_dir).unwrap();
        // Do NOT create the .age file

        let target_path = dir.path().join("deployed/tier-all.env");

        let config = build_test_config(
            dir.path(),
            &identity_path,
            &secrets_dir,
            &[(
                "tier-all",
                &[pubkey.as_str()],
                target_path.to_str().unwrap(),
            )],
        );

        let results = deploy_secrets(&config).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, DeployStatus::Error(_)));
    }

    #[test]
    fn encrypt_no_recipients_fails() {
        let dir = tempfile::tempdir().unwrap();
        let plaintext_path = dir.path().join("secret.txt");
        let encrypted_path = dir.path().join("secret.txt.age");
        fs::write(&plaintext_path, "hello").unwrap();

        let result = encrypt_file(&plaintext_path, &[], &encrypted_path);
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_invalid_recipient_fails() {
        let dir = tempfile::tempdir().unwrap();
        let plaintext_path = dir.path().join("secret.txt");
        let encrypted_path = dir.path().join("secret.txt.age");
        fs::write(&plaintext_path, "hello").unwrap();

        let result = encrypt_file(
            &plaintext_path,
            &["not-a-valid-age-key".to_string()],
            &encrypted_path,
        );
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_missing_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (_pubkey, identity_path) = setup_keypair(dir.path());

        let result = decrypt_file(&dir.path().join("nonexistent.age"), &identity_path);
        assert!(result.is_err());
    }

    #[test]
    fn read_public_key_from_identity_works() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());

        let read_pubkey = read_public_key_from_identity(&identity_path).unwrap();
        assert_eq!(read_pubkey, pubkey);
    }

    /// Helper to build a NitConfig for testing deploy_secrets
    fn build_test_config(
        base_dir: &Path,
        identity_path: &Path,
        secrets_dir: &Path,
        tiers: &[(&str, &[&str], &str)],
    ) -> NitConfig {
        use crate::config::*;
        use std::collections::HashMap;

        let mut tier_configs = HashMap::new();
        for (name, recipients, target) in tiers {
            tier_configs.insert(
                name.to_string(),
                TierConfig {
                    recipients: recipients.iter().map(|s| s.to_string()).collect(),
                    target: target.to_string(),
                },
            );
        }

        NitConfig {
            fleet: FleetConfig {
                machines: {
                    let mut m = HashMap::new();
                    m.insert(
                        "test-machine".to_string(),
                        MachineConfig {
                            ssh_host: "localhost".to_string(),
                            role: vec!["dev".to_string()],
                            critical: false,
                        },
                    );
                    m
                },
                templates: TemplatesConfig {
                    source_dir: base_dir.join("templates").to_str().unwrap().to_string(),
                },
                secrets: SecretsConfig {
                    source_dir: secrets_dir.to_str().unwrap().to_string(),
                    tiers: tier_configs,
                },
                permissions: PermissionsConfig { private: vec![] },
                exclude: HashMap::new(),
                sync: None,
            },
            local: LocalConfig {
                machine: "test-machine".to_string(),
                identity: identity_path.to_str().unwrap().to_string(),
                git: GitStrategyConfig::default(),
            },
            machine_name: "test-machine".to_string(),
            machine: MachineConfig {
                ssh_host: "localhost".to_string(),
                role: vec!["dev".to_string()],
                critical: false,
            },
            triggers: vec![],
            templates_dir: base_dir.join("templates"),
            secrets_dir: secrets_dir.to_path_buf(),
            project_dir: base_dir.to_path_buf(),
        }
    }
}
