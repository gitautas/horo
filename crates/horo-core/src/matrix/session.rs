//! Session persistence.
//!
//! Two things need to survive a restart: the Matrix access token and the
//! passphrase for the encrypted SQLite store that holds the E2EE keys. Both are
//! secrets, so they live in the OS keychain (Keychain / Credential Manager /
//! Secret Service) and never touch disk in plaintext.
//!
//! On disk we keep only `session.json`, which is non-sensitive: which user is
//! signed in, against which homeserver, and which store directory is theirs.
//! That file is what tells us there *is* a session to restore.
//!
//! If the platform has no usable keychain (a headless Linux box with no Secret
//! Service, say), we fall back to a `0600` file in the app data dir and report
//! it, rather than silently failing to sign in. The caller surfaces that to the
//! user so the weaker storage is never a surprise.

use std::path::{Path, PathBuf};

use matrix_sdk::{authentication::matrix::MatrixSession, ruma::OwnedUserId};
use rand::{Rng, distr::Alphanumeric};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Kept from before the rename to horo: changing it would orphan saved sessions.
const KEYRING_SERVICE: &str = "gg.uwu.uwum";
const SESSION_FILE: &str = "session.json";
const FALLBACK_FILE: &str = "session.secret.json";

/// The non-secret half of a persisted session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPointer {
    pub homeserver: String,
    pub user_id: String,
    pub device_id: String,
    /// Directory (relative to the app data dir) holding this user's SQLite store.
    pub store_dir: String,
    /// True when the secrets had to go to a file because no keychain was usable.
    #[serde(default)]
    pub insecure_fallback: bool,
}

/// The secret half: everything an attacker would need, kept in the keychain.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionSecrets {
    session: MatrixSession,
    /// Passphrase for the encrypted SQLite crypto store.
    store_passphrase: String,
}

/// A restored session, ready to hand to the client builder.
pub struct RestoredSession {
    pub pointer: SessionPointer,
    pub session: MatrixSession,
    pub store_passphrase: String,
    pub store_path: PathBuf,
}

fn pointer_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SESSION_FILE)
}

fn fallback_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FALLBACK_FILE)
}

/// A per-user store directory. User IDs contain `:` and `@`, which are awkward
/// in paths on Windows, so we sanitise rather than trust them.
fn store_dir_for(user_id: &OwnedUserId) -> String {
    let sanitised: String = user_id
        .as_str()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    format!("stores/{sanitised}")
}

pub fn generate_passphrase() -> String {
    rand::rng().sample_iter(&Alphanumeric).take(48).map(char::from).collect()
}

/// Write the file with owner-only permissions on Unix. Best effort elsewhere;
/// on Windows the app data dir is already per-user.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The keychain entry type, which is not the same crate layer on every
/// platform. `keyring`'s `v1` API picks a store for macOS, Windows and *nix,
/// but returns `Invalid("platform", ..)` on iOS — it deliberately covers no
/// mobile target. iOS therefore talks to `keyring_core` directly, against the
/// data-protection keychain registered in `install_ios_store`.
#[cfg(not(target_os = "ios"))]
type KeychainEntry = keyring::Entry;
#[cfg(target_os = "ios")]
type KeychainEntry = keyring_core::Entry;

/// Register the iOS data-protection keychain as the process-wide store, once.
///
/// `Store::new` uses the app's default access group, which the signing identity
/// supplies — no `keychain-access-groups` entitlement is needed as long as we
/// never share items with another app.
///
/// Note the accessibility this implies: items are readable only while the
/// device is unlocked. A sync resuming on a locked phone sees a keychain error
/// here, not a missing session, so it must never be treated as a sign-out.
#[cfg(target_os = "ios")]
fn install_ios_store() -> std::result::Result<(), String> {
    use std::sync::{Mutex, Once};

    static ONCE: Once = Once::new();
    static FAILURE: Mutex<Option<String>> = Mutex::new(None);

    ONCE.call_once(|| match apple_native_keyring_store::protected::Store::new() {
        Ok(store) => keyring_core::set_default_store(store),
        Err(e) => *FAILURE.lock().unwrap() = Some(e.to_string()),
    });

    match FAILURE.lock().unwrap().clone() {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

fn keyring_entry(user_id: &str) -> Option<KeychainEntry> {
    #[cfg(target_os = "ios")]
    if let Err(e) = install_ios_store() {
        tracing::warn!("no usable keychain ({e}); falling back to file storage");
        return None;
    }

    match KeychainEntry::new(KEYRING_SERVICE, user_id) {
        Ok(entry) => Some(entry),
        Err(e) => {
            tracing::warn!("no usable keychain ({e}); falling back to file storage");
            None
        }
    }
}

/// Persist a freshly logged-in session. Returns the pointer that was written,
/// whose `insecure_fallback` flag tells the caller whether the keychain worked.
pub fn save(
    data_dir: &Path,
    homeserver: &str,
    session: &MatrixSession,
    store_passphrase: &str,
) -> Result<SessionPointer> {
    let user_id = session.meta.user_id.clone();
    let secrets =
        SessionSecrets { session: session.clone(), store_passphrase: store_passphrase.to_owned() };
    let secrets_json = serde_json::to_string(&secrets)?;

    let stored_in_keychain = match keyring_entry(user_id.as_str()) {
        Some(entry) => match entry.set_password(&secrets_json) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("keychain write failed ({e}); falling back to file storage");
                false
            }
        },
        None => false,
    };

    if stored_in_keychain {
        // Clear any fallback left over from an earlier, keychain-less run so we
        // never leave a stale plaintext copy behind.
        let _ = std::fs::remove_file(fallback_path(data_dir));
    } else {
        write_private(&fallback_path(data_dir), &secrets_json)?;
    }

    let pointer = SessionPointer {
        homeserver: homeserver.to_owned(),
        user_id: user_id.to_string(),
        device_id: session.meta.device_id.to_string(),
        store_dir: store_dir_for(&user_id),
        insecure_fallback: !stored_in_keychain,
    };
    write_private(&pointer_path(data_dir), &serde_json::to_string_pretty(&pointer)?)?;
    Ok(pointer)
}

/// Look for a session to restore. `Ok(None)` means "nobody is signed in", which
/// is a normal first-run state rather than an error.
pub fn load(data_dir: &Path) -> Result<Option<RestoredSession>> {
    let pointer_path = pointer_path(data_dir);
    if !pointer_path.exists() {
        return Ok(None);
    }

    let pointer: SessionPointer = serde_json::from_str(&std::fs::read_to_string(&pointer_path)?)?;

    let secrets_json = if pointer.insecure_fallback {
        std::fs::read_to_string(fallback_path(data_dir)).ok()
    } else {
        keyring_entry(&pointer.user_id).and_then(|entry| match entry.get_password() {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("keychain read failed: {e}");
                None
            }
        })
    };

    let Some(secrets_json) = secrets_json else {
        // The pointer says there's a session but the secrets are gone — the user
        // cleared their keychain, or moved the profile between machines. Treat
        // it as signed out and clean up so we don't loop on it.
        tracing::warn!("session pointer found but secrets are missing; clearing it");
        clear(data_dir, Some(&pointer.user_id))?;
        return Ok(None);
    };

    let secrets: SessionSecrets = serde_json::from_str(&secrets_json)?;
    let store_path = data_dir.join(&pointer.store_dir);

    Ok(Some(RestoredSession {
        session: secrets.session,
        store_passphrase: secrets.store_passphrase,
        store_path,
        pointer,
    }))
}

/// Forget the session. Leaves the SQLite store alone — `wipe_store` does that
/// separately, because signing out shouldn't necessarily destroy the key backup
/// cache on a machine the user still trusts.
pub fn clear(data_dir: &Path, user_id: Option<&str>) -> Result<()> {
    if let Some(user_id) = user_id
        && let Some(entry) = keyring_entry(user_id)
    {
        // A missing entry is the desired end state, so ignore NoEntry.
        if let Err(e) = entry.delete_credential() {
            tracing::debug!("keychain delete: {e}");
        }
    }
    let _ = std::fs::remove_file(fallback_path(data_dir));
    let _ = std::fs::remove_file(pointer_path(data_dir));
    Ok(())
}

/// Delete a user's encrypted store directory entirely.
pub fn wipe_store(data_dir: &Path, pointer: &SessionPointer) -> Result<()> {
    let path = data_dir.join(&pointer.store_dir);
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// Where a brand-new login for `user_id` should keep its store — always empty.
///
/// Anything already in that directory is unreadable to this login: signing out
/// deletes the store's passphrase, and a login that failed after the store was
/// created never saved one. Opening it under a freshly generated passphrase
/// fails with `aead::Error`, on every retry, so the user could never sign back
/// in. Nor could it be reused if the passphrase survived: it holds the crypto
/// identity of a device the server has already been told about, and a new
/// login is a new device. Room keys that matter are in the server-side backup.
pub fn new_store_path(data_dir: &Path, user_id: &OwnedUserId) -> Result<PathBuf> {
    let path = data_dir.join(store_dir_for(user_id));
    if path.exists() {
        tracing::info!("discarding the store left by an earlier sign-in to {user_id}");
        std::fs::remove_dir_all(&path).map_err(Error::Io)?;
    }
    std::fs::create_dir_all(&path).map_err(Error::Io)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_store_path_starts_empty() {
        let data_dir = std::env::temp_dir().join(format!("horo-store-test-{}", std::process::id()));
        let user_id = OwnedUserId::try_from("@someone:example.org").unwrap();

        let first = new_store_path(&data_dir, &user_id).unwrap();
        std::fs::write(first.join("matrix-sdk-crypto.sqlite3"), b"old device").unwrap();

        let second = new_store_path(&data_dir, &user_id).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read_dir(&second).unwrap().count(), 0);

        std::fs::remove_dir_all(&data_dir).unwrap();
    }
}
