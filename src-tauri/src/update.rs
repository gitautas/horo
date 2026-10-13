//! Which update path this particular build can take, and — for the builds that
//! can't update themselves — what the newest release actually is.
//!
//! Tauri's updater replaces an install in place, so it only works where there
//! *is* an install to replace: a `.app`, an AppImage, or an NSIS/MSI install
//! root. Everything else the release workflow produces — the `.deb`, the
//! `.apk`, the `.ipa` — is owned by something that isn't us (apt, the phone),
//! and overwriting it from inside the app would either fail or corrupt it.
//!
//! Rather than let the frontend guess from a user-agent string, that decision
//! is made here, once, and handed over as a [`Mode`]. The two arms then read
//! the *same* manifest: `check()` on the updater plugin and [`latest_release`]
//! below both fetch `latest.json`, so a build can never be told about a version
//! the release workflow hasn't finished publishing.

use serde::Serialize;
use ts_rs::TS;

use crate::error::{Error, Result};

/// The manifest the release workflow publishes, and the updater reads.
///
/// Kept in one place so the manual path can never drift to a different source
/// of truth than the in-app one. `/releases/latest/download/` resolves against
/// whichever release is marked latest, which the workflow only does once every
/// platform's assets are attached — so this is never a half-published version.
const MANIFEST: &str = "https://github.com/gitautas/horo/releases/latest/download/latest.json";

#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export, rename = "UpdateMode")]
#[serde(rename_all = "kebab-case")]
// Each platform's `update_mode` can only ever construct one of these, so on
// any single target the other looks unreachable — but not across all of them.
#[allow(dead_code)]
pub enum Mode {
    /// The updater plugin is compiled in and this install is one it can
    /// replace. The frontend drives it through `@tauri-apps/plugin-updater`.
    InApp,
    /// A packaged build that something else installed. We can still say a new
    /// version exists and open the release page; we cannot apply it.
    Manual,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct LatestRelease {
    pub version: String,
    pub notes: String,
    /// The release page, not an asset: which asset is right depends on a
    /// platform question the browser is better placed to answer than we are.
    pub url: String,
}

/// Whether this build can install its own updates.
///
/// Compile-time everywhere except Linux, where the same binary ships inside an
/// AppImage (updatable) and inside a `.deb` (not). The AppImage runtime exports
/// `APPIMAGE` pointing at the image it launched from, and the updater needs
/// exactly that variable to know what to overwrite — so its presence is not a
/// heuristic for "is this an AppImage", it is the precise condition under which
/// the update would succeed.
#[tauri::command]
pub fn update_mode() -> Mode {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        Mode::Manual
    }

    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("APPIMAGE").is_some() {
            Mode::InApp
        } else {
            Mode::Manual
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        Mode::InApp
    }
}

/// The newest published release, for the builds that have to be updated by hand.
///
/// Only the version and notes are read: `latest.json`'s `platforms` map covers
/// the desktop bundles the updater serves, and says nothing about the `.apk` or
/// the `.deb`. Those are on the release page, which is where this points.
#[tauri::command]
pub async fn latest_release() -> Result<LatestRelease> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        version: String,
        #[serde(default)]
        notes: String,
    }

    let manifest: Manifest = reqwest::Client::new()
        .get(MANIFEST)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    // The manifest carries a bare version; tags carry a `v`. Tolerate either so
    // a hand-edited manifest can't produce a 404 of a link.
    let version = manifest.version.trim_start_matches('v').to_string();

    if version.is_empty() {
        return Err(Error::Other("release manifest has no version".into()));
    }

    Ok(LatestRelease {
        url: format!("https://github.com/gitautas/horo/releases/tag/v{version}"),
        notes: manifest.notes,
        version,
    })
}

/// `a > b`, comparing dotted numeric versions component by component.
///
/// Not `semver`: the release script only ever produces `major.minor.patch` from
/// arithmetic on the previous one, so a whole crate to parse pre-release tags
/// that cannot occur would be dead weight. A non-numeric component sorts as 0,
/// which makes a malformed manifest *fail to* offer an update rather than
/// nagging every launch.
pub fn is_newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| {
        let mut out = [0u64; 3];
        for (slot, piece) in out.iter_mut().zip(v.trim_start_matches('v').split('.')) {
            *slot = piece
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap_or("")
                .parse()
                .unwrap_or(0);
        }
        out
    };

    parts(a) > parts(b)
}

/// Is `latest` newer than the version this binary was built as?
#[tauri::command]
pub fn update_available(latest: String) -> bool {
    is_newer(&latest, env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_component_by_component() {
        assert!(is_newer("0.4.2", "0.4.1"));
        assert!(is_newer("0.5.0", "0.4.99"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.4.1", "0.4.1"));
        assert!(!is_newer("0.4.0", "0.4.1"));
        // 10 > 9 as a number, and would be `<` as a string.
        assert!(is_newer("0.10.0", "0.9.0"));
    }

    #[test]
    fn tolerates_a_leading_v_and_junk() {
        assert!(is_newer("v0.4.2", "0.4.1"));
        assert!(is_newer("0.4.2-rc1", "0.4.1"));
        // Unparseable reads as 0.0.0, so it never offers a bogus update.
        assert!(!is_newer("garbage", "0.4.1"));
    }
}
