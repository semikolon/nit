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
    /// is stale. `nit apply --discard-target-edits` takes the source;
    /// `nit encrypt` here would WRONGLY push the stale target back.
    StaleTarget { missing: usize },
    /// Target has extra key(s) and/or changed value(s) not in the source → a
    /// genuine local edit `nit encrypt` would flush. Discarding via
    /// `--discard-target-edits` would lose it. (Wins over StaleTarget when both hold:
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
    /// The target differed from the source, but its bytes are exactly an EARLIER committed
    /// version of this same secret: a stale copy nobody has edited, so deploying the
    /// current source loses nothing. `versions_back` counts the source's commits back
    /// from the newest (1 = the newest commit's version).
    Refreshed {
        versions_back: usize,
    },
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
/// missing-only ⇒ `StaleTarget` (a stale source, or keys deleted by hand: only the earlier-version check can tell).
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
            "target lacks {missing} key(s) the source has and holds nothing the \
             source lacks. Either the source advanced upstream (the usual cause), or \
             someone deleted keys here by hand; nit could not match this file to an \
             earlier committed version of the source, so it did not decide. \
             `nit apply --discard-target-edits` takes the source (it loses nothing in \
             the first case, only the deletion in the second). Do NOT `nit encrypt`: \
             it would push the stale target back over the source."
        ),
        DriftClass::LikelyUnflushedEdit { extra, changed } => format!(
            "target has LOCAL edits not in the source ({extra} extra key(s), \
             {changed} changed) — run `nit encrypt {target}` to flush them into \
             the source, or `nit apply --discard-target-edits` to DISCARD them."
        ),
        DriftClass::Ambiguous => format!(
            "target diverges from source (non-key-level or mixed) — \
             `nit encrypt {target}` to flush target→source, or \
             `nit apply --discard-target-edits` to overwrite target with source."
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
    let ciphertext = fs::read(encrypted_path).map_err(|e| {
        format!(
            "cannot read encrypted file {}: {}",
            encrypted_path.display(),
            e
        )
    })?;
    decrypt_bytes(&ciphertext, identity_path)
}

/// Decrypt age ciphertext held in memory (armored or binary) with an identity file.
pub fn decrypt_bytes(
    ciphertext: &[u8],
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

    // Auto-detect armor: chezmoi (and `age -a` / `age --armor`) writes
    // ASCII-armored age files starting with `-----BEGIN AGE ENCRYPTED FILE-----`.
    // age::Decryptor::new_buffered expects BINARY format. Wrap with
    // ArmoredReader which transparently handles both armored and binary input.
    let armored = age::armor::ArmoredReader::new(ciphertext);
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
/// `force_drift_override = true` (`nit apply --discard-target-edits`) skips the drift
/// check that protects against silent overwrites of unflushed manual edits. Use only when
/// the user has explicitly confirmed they want to discard target-side changes. Without it,
/// a drifted target is still refreshed automatically when it is provably a stale copy of
/// an earlier committed version (`stale_copy_depth`); anything else is reported, not
/// overwritten.
pub fn deploy_secrets(
    config: &NitConfig,
    force_drift_override: bool,
) -> Result<Vec<SecretResult>, Box<dyn std::error::Error>> {
    deploy_secrets_with(config, force_drift_override, &|age_path, identity| {
        earlier_versions_from_git(&config.local.git.strategy, age_path, identity)
    })
}

/// How many of a secret's earlier committed versions are compared with a drifted target.
/// Tiers change a few times a month, so this reaches back months; a target older than
/// that stays blocked and is resolved by hand, which is the safe failure.
const EARLIER_VERSIONS_CHECKED: usize = 40;

/// Position (1 = the newest commit's version) of the first earlier version whose bytes
/// equal `target`, or `None`. A match PROVES the target is a stale copy of this very secret
/// that nobody has edited: it is byte-for-byte something nit once deployed. That is the
/// same test `nit discard` applies to tracked files (restore only when nothing is lost),
/// and it is stricter than the key-level classifier, which cannot tell a source that
/// advanced from keys deleted by hand.
pub fn stale_copy_depth(target: &[u8], earlier_newest_first: &[Vec<u8>]) -> Option<usize> {
    earlier_newest_first
        .iter()
        .position(|v| v.as_slice() == target)
        .map(|i| i + 1)
}

/// Plaintext of the last `EARLIER_VERSIONS_CHECKED` committed versions of an encrypted
/// secret, newest first, decrypted in memory with this machine's identity (nothing is
/// written to disk). A version this machine cannot decrypt (an older recipient list, a
/// damaged blob) is skipped, so a failure here can only leave the target blocked.
pub fn earlier_versions_from_git(
    strategy: &crate::config::GitStrategy,
    encrypted_path: &Path,
    identity_path: &Path,
) -> Vec<Vec<u8>> {
    let Ok(rel) = encrypted_path.strip_prefix(crate::git::work_tree()) else {
        return Vec::new();
    };
    let Some(rel) = rel.to_str() else {
        return Vec::new();
    };
    let log = match crate::git::git_output_with(
        strategy,
        &[
            "log",
            "--format=%H",
            "-n",
            &EARLIER_VERSIONS_CHECKED.to_string(),
            "--",
            // `:/` roots the pathspec at the work tree. Without it git reads `rel`
            // relative to the CURRENT directory and finds nothing from anywhere but $HOME.
            &format!(":/{rel}"),
        ],
    ) {
        Ok(l) => l,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for sha in log.lines().filter(|l| !l.trim().is_empty()) {
        let blob = crate::git::git_command(strategy)
            .args(["show", &format!("{sha}:{rel}")])
            .output();
        let Ok(blob) = blob else { continue };
        if !blob.status.success() {
            continue;
        }
        if let Ok(plain) = decrypt_bytes(&blob.stdout, identity_path) {
            out.push(plain.into_bytes());
        }
    }
    out
}

/// `deploy_secrets` with the source of earlier versions injected (tests hand it a list;
/// production reads git).
pub fn deploy_secrets_with(
    config: &NitConfig,
    force_drift_override: bool,
    earlier: &dyn Fn(&Path, &Path) -> Vec<Vec<u8>>,
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
        // deploy if they diverge unless --discard-target-edits was passed (the user is
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

        let mut refreshed_from: Option<usize> = None;
        if !force_drift_override
            && let Some((target_bytes, source_bytes, classification)) =
                check_target_drift(plaintext.as_bytes(), &target_path)
        {
            // Before blocking, ask whether the target is merely a stale copy: byte-equal
            // to an earlier committed version of this secret. Then nobody edited it and
            // the source advanced (or removed a key) upstream, so deploying loses nothing.
            let on_disk = fs::read(&target_path).unwrap_or_default();
            match stale_copy_depth(&on_disk, &earlier(&encrypted_path, &identity_path)) {
                Some(n) => refreshed_from = Some(n),
                None => {
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
            }
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
                status: match refreshed_from {
                    Some(versions_back) => DeployStatus::Refreshed { versions_back },
                    None => DeployStatus::Deployed,
                },
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

    /// Guidance must be direction-correct: StaleTarget says what nit could not
    /// prove and warns off `nit encrypt`; UnflushedEdit names `nit encrypt
    /// <target>` and DISCARD; Ambiguous offers both.
    #[test]
    fn drift_guidance_is_direction_correct() {
        let stale = drift_guidance(&DriftClass::StaleTarget { missing: 2 }, "/tmp/t.env");
        assert!(stale.contains("lacks 2 key(s)"));
        assert!(stale.contains("nit apply --discard-target-edits"));
        assert!(
            !stale.contains("is SAFE"),
            "not proven stale, so it must not be called safe"
        );
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
        assert!(amb.contains("--discard-target-edits"));
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

    // ── 2026-11-22: a stale target is refreshed when it is provably a stale copy ──

    /// Build a one-tier config whose encrypted source decrypts to `source` and whose
    /// deployed target holds `target`.
    fn drift_fixture(
        source: &str,
        target: &str,
    ) -> (tempfile::TempDir, NitConfig, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (pubkey, identity_path) = setup_keypair(dir.path());
        let secrets_dir = dir.path().join("secrets");
        fs::create_dir(&secrets_dir).unwrap();
        let plaintext_path = dir.path().join("plaintext.tmp");
        fs::write(&plaintext_path, source).unwrap();
        encrypt_file(
            &plaintext_path,
            std::slice::from_ref(&pubkey),
            &secrets_dir.join("tier-all.env.age"),
        )
        .unwrap();
        let target_path = dir.path().join("deployed/tier-all.env");
        fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        fs::write(&target_path, target).unwrap();
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
        (dir, config, target_path)
    }

    fn versions(list: &[&str]) -> Vec<Vec<u8>> {
        list.iter().map(|v| v.as_bytes().to_vec()).collect()
    }

    #[test]
    fn stale_copy_depth_finds_the_position_newest_first() {
        let earlier = versions(&["A=3\n", "A=2\n", "A=1\n"]);
        assert_eq!(stale_copy_depth(b"A=3\n", &earlier), Some(1));
        assert_eq!(stale_copy_depth(b"A=1\n", &earlier), Some(3));
        assert_eq!(stale_copy_depth(b"A=9\n", &earlier), None);
        assert_eq!(stale_copy_depth(b"A=1\n", &[]), None);
    }

    /// The case that blocked the spela key on Darwin: the source gained a key upstream and
    /// the deployed target is the previous version. It is refreshed with no flag.
    #[test]
    fn a_target_that_is_an_earlier_version_is_refreshed_without_the_flag() {
        let v1 = "A=1\nB=2\n";
        let v2 = "A=1\nB=2\nNEW=3\n";
        let (_d, config, target) = drift_fixture(v2, v1);
        let results = deploy_secrets_with(&config, false, &|_, _| versions(&[v2, v1])).unwrap();
        assert!(
            matches!(
                results[0].status,
                DeployStatus::Refreshed { versions_back: 2 }
            ),
            "got {:?}",
            results[0].status
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), v2);
    }

    /// A key removed upstream: the target still has it and is byte-equal to an older version.
    /// The key-level classifier calls that an unflushed edit (extra key); the history proof
    /// shows nobody edited anything, so the removal propagates.
    #[test]
    fn a_key_removed_upstream_propagates_when_the_target_is_an_old_version() {
        let v1 = "A=1\nB=2\n";
        let v2 = "A=1\nB=2\nGONE=3\n";
        let (_d, config, target) = drift_fixture(v1, v2);
        let results = deploy_secrets_with(&config, false, &|_, _| versions(&[v1, v2])).unwrap();
        assert!(matches!(results[0].status, DeployStatus::Refreshed { .. }));
        assert_eq!(fs::read_to_string(&target).unwrap(), v1);
    }

    /// The May 2026 incident: a key added by hand on the machine and never encrypted. It is
    /// in no earlier version, so it is blocked and the file is left untouched.
    #[test]
    fn a_key_added_by_hand_is_still_blocked_and_kept() {
        let v1 = "A=1\nB=2\n";
        let edited = "A=1\nB=2\nSARPETORP_WIFI_PSK=hunter2\n";
        let (_d, config, target) = drift_fixture(v1, edited);
        let results = deploy_secrets_with(&config, false, &|_, _| versions(&[v1])).unwrap();
        assert!(matches!(
            results[0].status,
            DeployStatus::DriftDetected { .. }
        ));
        assert_eq!(fs::read_to_string(&target).unwrap(), edited);
    }

    /// A key deleted by hand looks like a stale target to the key-level classifier
    /// (missing-only). It matches no earlier version, so it is blocked, not refreshed.
    #[test]
    fn a_key_deleted_by_hand_is_blocked_not_mistaken_for_stale() {
        let v1 = "A=1\nB=2\nC=3\n";
        let (_d, config, target) = drift_fixture(v1, "A=1\nB=2\n");
        let results = deploy_secrets_with(&config, false, &|_, _| versions(&[v1])).unwrap();
        match &results[0].status {
            DeployStatus::DriftDetected { classification, .. } => {
                assert_eq!(*classification, DriftClass::StaleTarget { missing: 1 })
            }
            other => panic!("expected DriftDetected, got {:?}", other),
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "A=1\nB=2\n");
    }

    /// A changed value, not in any earlier version: blocked.
    #[test]
    fn a_hand_changed_value_is_blocked() {
        let v1 = "A=1\nB=2\n";
        let (_d, config, target) = drift_fixture(v1, "A=1\nB=999\n");
        let results = deploy_secrets_with(&config, false, &|_, _| versions(&[v1])).unwrap();
        assert!(matches!(
            results[0].status,
            DeployStatus::DriftDetected { .. }
        ));
        assert_eq!(fs::read_to_string(&target).unwrap(), "A=1\nB=999\n");
    }

    /// No history at all (a shallow clone, or every older version encrypted to a recipient
    /// list this machine is not on): nothing can be proven, so it stays blocked.
    #[test]
    fn with_no_history_the_old_behaviour_holds() {
        let (_d, config, target) = drift_fixture("A=1\nNEW=2\n", "A=1\n");
        let results = deploy_secrets_with(&config, false, &|_, _| Vec::new()).unwrap();
        assert!(matches!(
            results[0].status,
            DeployStatus::DriftDetected { .. }
        ));
        assert_eq!(fs::read_to_string(&target).unwrap(), "A=1\n");
    }

    /// History is only read when there is something to prove: no drift, no lookup.
    #[test]
    fn history_is_not_read_when_the_target_already_matches() {
        let (_d, config, _t) = drift_fixture("A=1\n", "A=1\n");
        let calls = std::cell::Cell::new(0);
        let results = deploy_secrets_with(&config, false, &|_, _| {
            calls.set(calls.get() + 1);
            Vec::new()
        })
        .unwrap();
        assert!(matches!(results[0].status, DeployStatus::Deployed));
        assert_eq!(calls.get(), 0);
    }

    /// The explicit flag still discards, and it never reads history.
    #[test]
    fn the_discard_flag_still_overwrites_without_proof() {
        let (_d, config, target) = drift_fixture("A=1\n", "A=1\nHAND=edit\n");
        let results = deploy_secrets_with(&config, true, &|_, _| panic!("no lookup")).unwrap();
        assert!(matches!(results[0].status, DeployStatus::Deployed));
        assert_eq!(fs::read_to_string(&target).unwrap(), "A=1\n");
    }

    /// Against this machine's real repository and key, not a fixture (run with
    /// `cargo test -- --ignored`): the git history of a real tier is readable and decrypts,
    /// newest first, and the commit that added `SPELA_ANTHROPIC_API_KEY` (2026-11-22) is
    /// the newest version while the one before it lacks the key. This is the case the
    /// feature exists for, and the one the synthetic tests above only imitate.
    #[test]
    #[ignore = "needs the Mac's real repo and age identity"]
    fn real_history_of_tier_servers_reads_back_in_order() {
        let home = dirs::home_dir().unwrap();
        let versions = earlier_versions_from_git(
            &crate::config::GitStrategy::Bare,
            &home.join("dotfiles/secrets/tier-servers.env.age"),
            &home.join(".config/nit/age-key.txt"),
        );
        assert!(versions.len() >= 3, "found {} versions", versions.len());
        let has = |v: &Vec<u8>| String::from_utf8_lossy(v).contains("SPELA_ANTHROPIC_API_KEY=");
        assert!(has(&versions[0]), "newest version holds the new key");
        assert!(!has(&versions[1]), "the version before it does not");
        // The target Darwin held before the update equals versions[1] byte for byte.
        assert_eq!(stale_copy_depth(&versions[1], &versions), Some(2));
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
