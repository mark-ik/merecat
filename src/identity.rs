//! The profile's root identity: whose authority every denizen grant descends
//! from.
//!
//! The capability-model round ruled (OQ2, 2026-07-24) that the user is a **root
//! subject**, not an implicit infinite authority, and that the root is a
//! personae identity rather than a placeholder constant. Install is then an
//! attenuating delegation signed by this key, and uninstall revokes it.
//!
//! **The key must persist.** Every install certificate names this identity's
//! master public key as its root; if the key changed across restarts, every
//! certificate would fail to verify as `WrongRoot` and every installed denizen
//! would silently lose its authority. So the master seed is written once into
//! the profile and read back thereafter.
//!
//! ## What this is, and what it is not yet
//!
//! This is a real Ed25519 master identity, and the certificates it signs are
//! real — but the seed sits **unsealed** in the profile directory. personae's
//! [`IdentityVault`](identity::vault::IdentityVault) is the production home
//! (sealed at rest, passphrase- or OS-unlocked; it is where the SSH key
//! already lives), and it implements the same
//! [`IdentityProvider`](identity::IdentityProvider) trait this returns. The
//! swap is therefore a constructor change here, not a change anywhere that
//! consumes it — merecat needs an unlock path in the shell before it can
//! demand a passphrase at boot, and inventing one silently would be worse than
//! naming the gap.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use identity::{IdentityProvider, InMemoryProvider};

/// Where the profile's master seed lives.
pub fn master_key_path(data_root: &Path) -> PathBuf {
    data_root.join("identity").join("master.key")
}

/// Load the profile's root identity, minting it on first run.
///
/// Never fails the caller: an unreadable or unwritable profile falls back to a
/// process-local random identity with a loud warning, because a browser that
/// refuses to start over a key file is worse than one whose denizens need
/// re-installing. A fallback identity means existing install certificates stop
/// verifying, which surfaces as denizens losing authority — visible, not
/// silent.
pub fn load_or_create_root(data_root: &Path) -> Arc<InMemoryProvider> {
    let path = master_key_path(data_root);
    match std::fs::read(&path) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            return Arc::new(InMemoryProvider::from_seed(seed));
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
    Arc::new(provider)
}

/// The root subject: the master public key every denizen grant descends from.
pub fn root_subject(provider: &impl IdentityProvider) -> servitor::Subject {
    servitor::Subject::new(provider.master_public_key().to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_identity_survives_a_restart() {
        // Load-bearing: install certificates name this key as their root, so a
        // key that changed across restarts would revoke every denizen.
        let dir = std::env::temp_dir().join(format!("merecat-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let first = load_or_create_root(&dir);
        let second = load_or_create_root(&dir);
        assert_eq!(
            root_subject(first.as_ref()),
            root_subject(second.as_ref()),
            "the same profile yields the same root identity"
        );
        assert!(master_key_path(&dir).is_file(), "the seed persisted");

        // A different profile is a different root.
        let other_dir = dir.join("other");
        let other = load_or_create_root(&other_dir);
        assert_ne!(root_subject(first.as_ref()), root_subject(other.as_ref()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
