//! The profile's root identity: whose authority every denizen grant descends
//! from.
//!
//! The capability-model round ruled (OQ2, 2026-07-24) that the user is a **root
//! subject**, not an implicit infinite authority, and that the root is a
//! personae identity rather than a placeholder constant. Install is an
//! attenuating delegation signed by this key; uninstall revokes it.
//!
//! **The key must persist.** Every install certificate names this identity's
//! master public key as its root; a key that changed across restarts would
//! fail every chain as `WrongRoot`. (When the root DOES legitimately change —
//! the vault migration below — the adopt path re-issues from the reviewed
//! grant projections: `denizen::rebuild`'s re-root heal.)
//!
//! ## Where the key lives
//!
//! The **personae vault**, opened with the same ceremony the personae bins
//! use (`bootstrap::open_storage` + `load_or_create_profile`): OS-sealed
//! records under DPAPI on Windows, or the portable passphrase vault when
//! `PERSONAE_PASSPHRASE` is set. By default turnstone opens the SHARED vault at
//! [`default_vault_dir`] with the `default` profile — the same profile the
//! personae SSH agent serves — so turnstone's root identity IS the user's
//! personae identity, not a browser-local key.
//!
//! Where no sealed backend exists (non-Windows without a passphrase), the
//! previous unsealed-seed path remains as a LOUD fallback: a browser that
//! refuses to start over a key store is worse than one that says plainly what
//! protects its key. Once the vault opens on a machine, the legacy unsealed
//! seed file is retired (deleted) so no plaintext key lingers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use identity::bootstrap::{self, Unlock};
use identity::vault::{IdentityStorage, IdentityVault, ProfileId};
use identity::{
    DerivedKeyAttestation, Ed25519Keypair, Ed25519PublicKey, IdentityError, IdentityProvider,
    InMemoryProvider,
};

/// The shared personae vault directory (`%LOCALAPPDATA%\personae\vault`).
pub fn default_vault_dir() -> PathBuf {
    bootstrap::default_vault_dir()
}

/// Where the LEGACY unsealed master seed lived (pre-vault, C4's stopgap).
pub fn master_key_path(data_root: &Path) -> PathBuf {
    data_root.join("identity").join("master.key")
}

/// The profile's root identity: vault-sealed when a backend exists, the
/// legacy unsealed seed otherwise. Both faces implement [`IdentityProvider`]
/// with identical derivation (`master.derive_child`), so certificates issued
/// under either verify the same way.
pub enum RootIdentity {
    /// The personae vault (sealed at rest; the production home).
    Vault {
        vault: IdentityVault<Box<dyn IdentityStorage>>,
        /// The backend's honest one-line account of what protects the key,
        /// straight from personae — shown, never guessed.
        description: String,
    },
    /// The pre-vault unsealed seed (or a process-local key when even that
    /// could not persist). Loud, never silent.
    Unsealed(InMemoryProvider),
}

impl RootIdentity {
    /// What protects the key, for the boot log and any future settings row.
    pub fn description(&self) -> String {
        match self {
            RootIdentity::Vault { description, .. } => description.clone(),
            RootIdentity::Unsealed(_) => "UNSEALED profile seed (no vault backend)".to_string(),
        }
    }
}

impl IdentityProvider for RootIdentity {
    fn master_public_key(&self) -> Ed25519PublicKey {
        match self {
            RootIdentity::Vault { vault, .. } => vault.master_public_key(),
            RootIdentity::Unsealed(provider) => provider.master_public_key(),
        }
    }

    fn derive_keypair(&self, salt: &[u8]) -> Result<Ed25519Keypair, IdentityError> {
        match self {
            RootIdentity::Vault { vault, .. } => vault.derive_keypair(salt),
            RootIdentity::Unsealed(provider) => provider.derive_keypair(salt),
        }
    }

    fn attest_derived_key(&self, salt: &[u8]) -> Result<DerivedKeyAttestation, IdentityError> {
        match self {
            RootIdentity::Vault { vault, .. } => vault.attest_derived_key(salt),
            RootIdentity::Unsealed(provider) => provider.attest_derived_key(salt),
        }
    }
}

/// Load the profile's root identity: the personae vault at `vault_dir` first
/// (profile `default` — shared with the personae bins, so turnstone's root is
/// the user's actual identity), the legacy unsealed seed as a loud fallback.
///
/// Never fails the caller: a browser that refuses to start over a key store
/// is worse than one whose denizens need re-rooting. A changed root is not
/// silent breakage either — `denizen::rebuild` re-issues from the reviewed
/// projections under the current root (the re-root heal).
pub fn load_or_create_root(data_root: &Path, vault_dir: &Path) -> Arc<RootIdentity> {
    match open_vault(vault_dir) {
        Ok(root) => {
            tracing::info!(protection = %root.description(), "profile identity");
            retire_unsealed_seed(data_root);
            Arc::new(root)
        }
        Err(err) => {
            tracing::warn!(
                %err,
                "personae vault unavailable; falling back to the unsealed profile seed"
            );
            Arc::new(RootIdentity::Unsealed(unsealed_fallback(data_root)))
        }
    }
}

fn open_vault(vault_dir: &Path) -> Result<RootIdentity, IdentityError> {
    let opened = bootstrap::open_storage(vault_dir, Unlock::from_env())?;
    let (profile, created) =
        bootstrap::load_or_create_profile(&*opened.storage, &ProfileId("default".into()))?;
    if created {
        tracing::info!("minted the profile's personae identity (vault profile `default`)");
    }
    Ok(RootIdentity::Vault {
        vault: IdentityVault::with_profile(opened.storage, profile),
        description: opened.description,
    })
}

/// Delete the legacy unsealed seed once the vault holds the identity: leaving
/// a plaintext key on disk after the sealed one exists is the exact exposure
/// the swap removes. Any denizen certificates rooted on the old key re-root
/// through the adopt heal, so nothing depends on the file.
fn retire_unsealed_seed(data_root: &Path) {
    let path = master_key_path(data_root);
    if !path.is_file() {
        return;
    }
    match std::fs::remove_file(&path) {
        Ok(()) => tracing::info!(
            path = ?path,
            "retired the legacy unsealed master seed (the vault holds the identity now)"
        ),
        Err(err) => tracing::warn!(%err, path = ?path, "could not retire the unsealed master seed"),
    }
}

/// The pre-vault path, unchanged: a 32-byte seed in the profile directory,
/// minted on first run. Kept as the fallback for platforms with no sealed
/// backend and no passphrase.
fn unsealed_fallback(data_root: &Path) -> InMemoryProvider {
    let path = master_key_path(data_root);
    match std::fs::read(&path) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            return InMemoryProvider::from_seed(seed);
        }
        Ok(_) => {
            tracing::warn!(path = ?path, "profile master key is not 32 bytes; minting a fresh one");
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            tracing::warn!(%err, path = ?path, "could not read the profile master key");
        }
    }
    let provider = InMemoryProvider::random();
    let seed = provider.master_keypair().to_seed();
    let written = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, seed)
    })();
    if let Err(err) = written {
        tracing::warn!(
            %err,
            path = ?path,
            "could not persist the profile master key; denizen grants will not survive a restart"
        );
    }
    provider
}

/// The root subject: the master public key every denizen grant descends from.
pub fn root_subject(provider: &impl IdentityProvider) -> servitor::Subject {
    servitor::Subject::new(provider.master_public_key().to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("turnstone-identity-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A vault dir that cannot exist (a path under a FILE), forcing the
    /// unsealed fallback deterministically on every platform.
    fn broken_vault_dir(base: &Path) -> PathBuf {
        std::fs::create_dir_all(base).unwrap();
        let file = base.join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        file.join("vault")
    }

    #[test]
    fn the_root_identity_survives_a_restart() {
        // Load-bearing: install certificates name this key as their root, so
        // the same profile must yield the same identity across opens —
        // through the vault when one opens, through the fallback otherwise.
        let dir = scratch("restart");
        let vault = dir.join("personae-vault");

        let first = load_or_create_root(&dir, &vault);
        let second = load_or_create_root(&dir, &vault);
        assert_eq!(
            root_subject(first.as_ref()),
            root_subject(second.as_ref()),
            "the same profile yields the same root identity ({})",
            first.description()
        );

        // A different profile is a different root.
        let other = dir.join("other");
        let other_root = load_or_create_root(&other, &other.join("personae-vault"));
        assert_ne!(root_subject(first.as_ref()), root_subject(other_root.as_ref()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_unsealed_fallback_persists_and_says_so() {
        let dir = scratch("fallback");
        let broken = broken_vault_dir(&dir);

        let first = load_or_create_root(&dir, &broken);
        assert!(matches!(first.as_ref(), RootIdentity::Unsealed(_)));
        assert!(first.description().contains("UNSEALED"), "honest about the protection");
        assert!(master_key_path(&dir).is_file(), "the fallback seed persisted");

        let second = load_or_create_root(&dir, &broken);
        assert_eq!(
            root_subject(first.as_ref()),
            root_subject(second.as_ref()),
            "the fallback is stable too"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn opening_the_vault_retires_the_unsealed_seed() {
        // The migration: a profile that ran on the unsealed fallback, once the
        // vault opens (DPAPI on Windows), stops keeping a plaintext key. The
        // root CHANGES here — the vault identity supersedes the stopgap — and
        // the denizen adopt path re-roots certificates from the reviewed
        // projections, which is tested in denizen/app.
        let dir = scratch("retire");
        let seeded = load_or_create_root(&dir, &broken_vault_dir(&dir));
        assert!(master_key_path(&dir).is_file(), "starts unsealed");

        let vaulted = load_or_create_root(&dir, &dir.join("personae-vault"));
        assert!(matches!(vaulted.as_ref(), RootIdentity::Vault { .. }));
        assert!(
            !master_key_path(&dir).is_file(),
            "the plaintext seed is retired once the vault holds the identity"
        );
        assert_ne!(
            root_subject(seeded.as_ref()),
            root_subject(vaulted.as_ref()),
            "the vault identity supersedes the stopgap key"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
