//! Is there a newer horo? The release workflow publishes `latest.json`
//! with each release, and only marks a release latest once every
//! platform's files are attached, so this never offers a half-published
//! version. (The Tauri build reads the same manifest.)

use std::time::Duration;

use crate::error::{Error, Result};

pub const MANIFEST: &str = "https://github.com/gitautas/horo/releases/latest/download/latest.json";

/// The minisign public key update archives are signed with (the private
/// half is the `TAURI_SIGNING_PRIVATE_KEY` release secret). The same key
/// the Tauri app checks, so its installs take the gpui app as an update.
pub const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDIwNkE2NTFBNDk4NDZDMEIKUldRTGJJUkpHbVZxSUxScldxR0hSb1FZeUVGUVVtNHdLWnkrejNnWndYYlR3a2VLd3BabmxQS00K";

/// The newest published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    /// The release page (which download is right is the browser's call).
    pub url: String,
}

pub async fn latest_release() -> Result<Release> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        version: String,
        #[serde(default)]
        notes: String,
    }
    let manifest: Manifest = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(Error::Http)?
        .get(MANIFEST)
        .send()
        .await
        .map_err(Error::Http)?
        .error_for_status()
        .map_err(Error::Http)?
        .json()
        .await
        .map_err(Error::Http)?;
    // Tags carry a `v`, the manifest shouldn't: tolerate either.
    let version = manifest.version.trim_start_matches('v').to_owned();
    if version.is_empty() {
        return Err(Error::Other("the release manifest has no version".into()));
    }
    Ok(Release { url: format!("https://github.com/gitautas/horo/releases/tag/v{version}"), notes: manifest.notes, version })
}

/// `a > b`, comparing dotted numbers part by part (`0.10.0 > 0.9.9`). A
/// part that isn't a number counts as 0, so a malformed manifest fails
/// to offer an update rather than nagging every launch.
pub fn is_newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| {
        let mut out = [0u64; 3];
        for (slot, piece) in out.iter_mut().zip(v.trim_start_matches('v').split('.')) {
            *slot = piece.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("").parse().unwrap_or(0);
        }
        out
    };
    parts(a) > parts(b)
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.4.2", "0.4.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("0.4.1", "0.4.1"));
        assert!(!is_newer("garbage", "0.0.1"));
    }
}
