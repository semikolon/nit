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

/// Direction of a secret-target drift (which side has content the other lacks),
/// so the abort message can guide the operator instead of always blaming an
/// "unflushed manual edit". The pre-2026-05-18 heuristic sent the operator to
/// investigate a non-edit whenever the source merely advanced upstream and the
/// deployed target was stale. See nit `TODO.md` § "Finding — nit secret-drift
/// heuristic is false-positive-prone".
#[derive(Debug, PartialEq, Eq)]
pub enum DriftClass {
    /// Source has key(s) the target lacks, with NO target-only keys and NO
    /// changed values → the source legitimately advanced; the deployed target
    /// is stale. `nit apply --force-drift` (take source) is the safe fix;
    /// `nit encrypt` here would WRONGLY push the stale target back.
    StaleTarget { missing: usize },
    /// Target has extra key(s) and/or changed value(s) not in the source → a
    /// genuine local edit `nit encrypt` would flush. Discarding via
    /// `--force-drift` would lose it. (Wins over StaleTarget when both hold:
    /// any target-unique content is the dangerous-to-discard signal.)
    LikelyUnflushedEdit { extra: usize, changed: usize },
    /// Non-env content, or a byte-diff with no key-level delta (whitespace /
    /// comments / ordering) — can't disambiguate; show both options.
    Ambiguous,
}

/// Status of a secret deployment
#[derive(Debug)]
pub enum DeployStatus {
    Deployed,
    Skipped(String),
    Error(String),
    /// Target file diverges from source-decrypt. `classification` captures the
    /// *direction* of the divergence so the caller emits direction-correct
    /// guidance instead of always blaming an unflushed manual edit (2026-05-18
    /// heuristic refinement; see nit `TODO.md`).
    /// See also: `~/.claude/CLAUDE.md` § "Secrets editing — `nit encrypt` is
    /// part of the same edit, not a follow-up step" (May 4, 2026 directive).
    DriftDetected {
        target_bytes: usize,
        source_bytes: usize,
        classification: DriftClass,
    },
}

/// Check whether deploying the source-decrypt would clobber a target with
/// unflushed manual edits. Returns Some if the target file exists AND its
/// content differs from `plaintext`. Returns None if no drift (target is
/// missing OR contents already match — the deploy is safe / is a no-op).
///
/// Note: we don't use mtime as a signal — both vim-edits-without-encrypt
/// AND legitimate forward deploys (post-pull, post-rekey) can produce
/// "source-decrypt != target". The user must resolve the ambiguity.
fn check_target_drift(plaintext: &[u8], target_path: &Path) -> Option<(usize, usize, DriftClass)> {
    if !target_path.exists() {
        return None;
    }
    let target_content = match fs::read(target_path) {
        Ok(c) => c,
        Err(_) => return None,
    };
    if target_content == plaintext {
        return None;
    }
    let classification = classify_env_drift(plaintext, &target_content);
    Some((target_content.len(), plaintext.len(), classification))
}

/// Parse an env-style buffer into KEY→VALUE, ignoring blank / `#`-comment /
/// no-`=` lines and an optional leading `export ` on the key. Mirrors the
/// operator's mental model of a `tier-*.env` file (order-insensitive).
fn parse_env(buf: &[u8]) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    let text = String::from_utf8_lossy(buf);
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((raw_key, value)) = line.split_once('=') else {
            continue;
        };
        let mut key = raw_key.trim();
        if let Some(rest) = key.strip_prefix("export ") {
            key = rest.trim();
        }
        if key.is_empty() {
            continue;
        }
        map.insert(key.to_string(), value.to_string());
    }
    map
}

/// Classify the *direction* of a secret-target drift so the abort message can
/// guide the operator. The pre-2026-05-18 heuristic always said "unflushed
/// manual edit" — wrong, and a wasted investigation, when the source merely
/// advanced upstream and the target is stale. Conservative priority: ANY
/// target-unique content (extra key OR changed value) ⇒ `LikelyUnflushedEdit`
/// (the dangerous-to-discard case) even if keys are also missing; pure
/// missing-only ⇒ `StaleTarget` (safe to `--force-drift`).
fn classify_env_drift(source: &[u8], target: &[u8]) -> DriftClass {
    let src = parse_env(source);
    let tgt = parse_env(target);
    if src.is_empty() && tgt.is_empty() {
        return DriftClass::Ambiguous; // non-env content (e.g. a config blob)
    }
    let missing = src.keys().filter(|k| !tgt.contains_key(*k)).count();
    let extra = tgt.keys().filter(|k| !src.contains_key(*k)).count();
    let mut changed = 0usize;
    for (k, v) in &src {
        if let Some(tv) = tgt.get(k)
            && tv != v
        {
            changed += 1;
        }
    }
    if extra > 0 || changed > 0 {
        DriftClass::LikelyUnflushedEdit { extra, changed }
    } else if missing > 0 {
        DriftClass::StaleTarget { missing }
    } else {
        // byte-diff with no key-level delta (whitespace / comments / ordering)
        DriftClass::Ambiguous
    }
}

/// Direction-aware operator guidance for a detected drift. Single source of
/// truth shared by `cmd_apply`, `cmd_update`, and `bootstrap` so all three
/// emit identical, correct advice.
pub fn drift_guidance(class: &DriftClass, target: &str) -> String {
    match class {
        DriftClass::StaleTarget { missing } => format!(
            "target is STALE ({missing} key(s) the source added upstream are \
             missing; no local edits) — `nit apply --force-drift` is SAFE here \
             (take the authoritative source). Do NOT `nit encrypt`: it would \
             push the stale target back over the source."
        ),
        DriftClass::LikelyUnflushedEdit { extra, changed } => format!(
            "target has LOCAL edits not in the source ({extra} extra key(s), \
             {changed} changed) — run `nit encrypt {target}` to flush them into \
             the source, or `nit apply --force-drift` to DISCARD them."
        ),
        DriftClass::Ambiguous => format!(
            "target diverges from source (non-key-level or mixed) — \
             `nit encrypt {target}` to flush target→source, or \
             `nit apply --force-drift` to overwrite target with source."
        ),
    }
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
///
/// Currently called only by tests — `deploy_secrets` inlines the equivalent
/// logic so it can interpose the drift check between decrypt and write.
/// Kept as a `pub` API for callers that want plain decrypt-and-write without
/// drift-check semantics.
#[allow(dead_code)]
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
///
/// `force_drift_override = true` skips the drift-check that protects against
/// silent overwrites of unflushed manual edits. Use only when the user has
/// explicitly confirmed they want to discard target-side changes.
pub fn deploy_secrets(
    config: &NitConfig,
    force_drift_override: bool,
) -> Result<Vec<SecretResult>, Box<dyn std::error::Error>> {
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

        // Drift check: decrypt source, compare to existing target. Skip the
        // deploy if they diverge unless --force-drift was passed (the user is
        // explicitly confirming they want to clobber unflushed edits).
        let plaintext = match decrypt_file(&encrypted_path, &identity_path) {
            Ok(s) => s,
            Err(e) => {
                results.push(SecretResult {
                    tier: tier_name.clone(),
                    target: target_path.display().to_string(),
                    status: DeployStatus::Error(e.to_string()),
                });
                continue;
            }
        };

        if !force_drift_override
            && let Some((target_bytes, source_bytes, classification)) =
                check_target_drift(plaintext.as_bytes(), &target_path)
        {
            results.push(SecretResult {
                tier: tier_name.clone(),
                target: target_path.display().to_string(),
                status: DeployStatus::DriftDetected {
                    target_bytes,
                    source_bytes,
                    classification,
                },
            });
            continue;
        }

        // Deploy: write plaintext atomically to target with 0600 perms.
        if let Some(parent) = target_path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            results.push(SecretResult {
                tier: tier_name.clone(),
                target: target_path.display().to_string(),
                status: DeployStatus::Error(format!("mkdir parent: {}", e)),
            });
            continue;
        }
        match fs::write(&target_path, plaintext.as_bytes())
            .and_then(|()| fs::set_permissions(&target_path, fs::Permissions::from_mode(0o600)))
        {
            Ok(()) => results.push(SecretResult {
                tier: tier_name.clone(),
                target: target_path.display().to_string(),
                status: DeployStatus::Deployed,
            }),
            Err(e) => results.push(SecretResult {
                tier: tier_name.clone(),
                target: target_path.display().to_string(),
                status: DeployStatus::Error(e.to_string()),
            }),
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
        encrypt_file(
            &plaintext_path,
            std::slice::from_ref(&pubkey_a),
            &encrypted_path,
        )
        .unwrap();

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
        encrypt_file(
            &plaintext_path,
            std::slice::from_ref(&pubkey),
            &encrypted_path,
        )
        .unwrap();

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

        let results = deploy_secrets(&config, false).unwrap();
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

        let results = deploy_secrets(&config, false).unwrap();
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

        let results = deploy_secrets(&config, false).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, DeployStatus::Error(_)));
    }

    // ── 2026-05-18 drift-direction classifier (the fleet-rollout finding) ──

    /// The MERIAN `tier-mac` anchoring case: source legitimately gained a key
    /// upstream; the deployed target is just stale (missing it), no local
    /// edits. Must classify StaleTarget so the operator is told `--force-drift`
    /// is SAFE — NOT sent to investigate a non-edit.
    #[test]
    fn classify_stale_target_when_source_added_a_key() {
        let source = b"A=1\nB=2\nADDED_UPSTREAM=x\n";
        let target = b"A=1\nB=2\n";
        assert_eq!(
            classify_env_drift(source, target),
            DriftClass::StaleTarget { missing: 1 }
        );
    }

    #[test]
    fn classify_unflushed_edit_when_target_has_extra_key() {
        let source = b"A=1\nB=2\n";
        let target = b"A=1\nB=2\nC=3\n";
        assert_eq!(
            classify_env_drift(source, target),
            DriftClass::LikelyUnflushedEdit {
                extra: 1,
                changed: 0
            }
        );
    }

    #[test]
    fn classify_unflushed_edit_when_value_changed() {
        let source = b"A=1\nB=2\n";
        let target = b"A=1\nB=999\n";
        assert_eq!(
            classify_env_drift(source, target),
            DriftClass::LikelyUnflushedEdit {
                extra: 0,
                changed: 1
            }
        );
    }

    /// Mixed (target both missing a source key AND carrying an extra one):
    /// conservative priority must pick the dangerous-to-discard class so
    /// `--force-drift` is never wrongly advertised as safe.
    #[test]
    fn classify_mixed_prefers_unflushed_edit() {
        let source = b"A=1\nB=2\nC=3\n"; // C missing from target
        let target = b"A=1\nB=2\nD=4\n"; // D extra on target
        assert_eq!(
            classify_env_drift(source, target),
            DriftClass::LikelyUnflushedEdit {
                extra: 1,
                changed: 0
            }
        );
    }

    /// Parser must strip `export ` + ignore comments so a purely cosmetic
    /// byte-diff (same keys/values) is NOT mis-flagged as a genuine edit.
    #[test]
    fn classify_ambiguous_when_only_cosmetic_diff() {
        let source = b"export A=1\n# a comment\nB=2\n";
        let target = b"A=1\nB=2\n";
        assert_eq!(classify_env_drift(source, target), DriftClass::Ambiguous);
    }

    #[test]
    fn classify_ambiguous_for_non_env_content() {
        let source = b"some free-form\nblob with no equals\n";
        let target = b"a different blob entirely\n";
        assert_eq!(classify_env_drift(source, target), DriftClass::Ambiguous);
    }

    /// Guidance must be direction-correct: StaleTarget says force-drift is
    /// SAFE and warns off `nit encrypt`; UnflushedEdit names `nit encrypt
    /// <target>` and DISCARD; Ambiguous offers both.
    #[test]
    fn drift_guidance_is_direction_correct() {
        let stale = drift_guidance(&DriftClass::StaleTarget { missing: 2 }, "/tmp/t.env");
        assert!(stale.contains("STALE"));
        assert!(stale.contains("`nit apply --force-drift` is SAFE"));
        assert!(stale.contains("Do NOT `nit encrypt`"));

        let edit = drift_guidance(
            &DriftClass::LikelyUnflushedEdit {
                extra: 1,
                changed: 0,
            },
            "/tmp/t.env",
        );
        assert!(edit.contains("LOCAL edits"));
        assert!(edit.contains("nit encrypt /tmp/t.env"));
        assert!(edit.contains("DISCARD"));

        let amb = drift_guidance(&DriftClass::Ambiguous, "/tmp/t.env");
        assert!(amb.contains("non-key-level or mixed"));
        assert!(amb.contains("--force-drift"));
    }

    /// Anchoring scenario for the May 5, 2026 drift-detection feature: target
    /// has unflushed manual edits, source-decrypt diverges. `deploy_secrets`
    /// must NOT clobber it (must report DriftDetected). Then `--force-drift`
    /// (force_drift_override=true) bypasses the check and deploys.
    #[test]
    fn deploy_secrets_aborts_on_target_drift_then_force_proceeds() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());

        // Create a source: encrypt "ORIGINAL" → tier-all.env.age
        let secrets_dir = dir.path().join("secrets");
        fs::create_dir(&secrets_dir).unwrap();
        let plaintext_path = dir.path().join("plaintext.tmp");
        let encrypted_path = secrets_dir.join("tier-all.env.age");
        fs::write(&plaintext_path, "ORIGINAL=value\n").unwrap();
        encrypt_file(
            &plaintext_path,
            std::slice::from_ref(&pubkey),
            &encrypted_path,
        )
        .unwrap();

        // Simulate vim-edit-without-encrypt: target has different content
        let target_path = dir.path().join("deployed/tier-all.env");
        fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        fs::write(&target_path, "VIM_EDITED=newvalue\n").unwrap();

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

        // No-force: drift detected, target preserved
        let results = deploy_secrets(&config, false).unwrap();
        assert_eq!(results.len(), 1);
        match &results[0].status {
            DeployStatus::DriftDetected {
                target_bytes,
                source_bytes,
                classification,
            } => {
                assert_eq!(*target_bytes, "VIM_EDITED=newvalue\n".len());
                assert_eq!(*source_bytes, "ORIGINAL=value\n".len());
                // ORIGINAL only-in-source, VIM_EDITED only-in-target →
                // target-unique content present ⇒ genuine-edit class.
                assert_eq!(
                    *classification,
                    DriftClass::LikelyUnflushedEdit {
                        extra: 1,
                        changed: 0
                    }
                );
            }
            other => panic!("expected DriftDetected, got {:?}", other),
        }
        assert_eq!(
            fs::read_to_string(&target_path).unwrap(),
            "VIM_EDITED=newvalue\n",
            "target must NOT have been clobbered when drift detected without --force"
        );

        // With force_drift_override=true, deploy proceeds (clobbers target)
        let results = deploy_secrets(&config, true).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, DeployStatus::Deployed));
        assert_eq!(
            fs::read_to_string(&target_path).unwrap(),
            "ORIGINAL=value\n",
            "target SHOULD have been clobbered when force_drift_override=true"
        );
    }

    /// Drift check is content-based, not mtime-based: identical content → no drift,
    /// even if target was touched after source (e.g., previous successful deploy).
    #[test]
    fn deploy_secrets_no_drift_when_target_matches_source() {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());

        let secrets_dir = dir.path().join("secrets");
        fs::create_dir(&secrets_dir).unwrap();
        let plaintext_path = dir.path().join("plaintext.tmp");
        let encrypted_path = secrets_dir.join("tier-all.env.age");
        fs::write(&plaintext_path, "X=1\n").unwrap();
        encrypt_file(
            &plaintext_path,
            std::slice::from_ref(&pubkey),
            &encrypted_path,
        )
        .unwrap();

        // Pre-populate target with IDENTICAL content
        let target_path = dir.path().join("deployed/tier-all.env");
        fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        fs::write(&target_path, "X=1\n").unwrap();

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

        let results = deploy_secrets(&config, false).unwrap();
        assert_eq!(results.len(), 1);
        // Re-deploy of identical content is reported as Deployed (idempotent).
        assert!(matches!(results[0].status, DeployStatus::Deployed));
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
