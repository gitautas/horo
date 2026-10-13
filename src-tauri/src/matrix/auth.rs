//! Sign-in, sign-out, and bringing a session up to a syncing state.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use matrix_sdk::{
    Client,
    authentication::matrix::MatrixSession,
    encryption::{BackupDownloadStrategy, EncryptionSettings},
    ruma::{RoomId, api::client::session::get_login_types::v3::LoginType},
    store::RoomLoadSettings,
};
use matrix_sdk_ui::sync_service::SyncService;
use tokio::sync::Mutex;

use crate::{
    dto::{HomeserverInfo, SessionInfo, SsoProvider},
    error::{Error, Result},
    events,
    matrix::{
        core::{AppState, MatrixCore},
        presence, rooms, session,
    },
};

const DEVICE_NAME: &str = "horo";

/// Build a throwaway client just to inspect a server. No store, no session.
async fn probe_client(server: &str) -> Result<Client> {
    Client::builder()
        .server_name_or_homeserver_url(server)
        .user_agent(user_agent())
        .build()
        .await
        .map_err(Error::ClientBuild)
}

fn user_agent() -> String {
    format!("horo/{}", env!("CARGO_PKG_VERSION"))
}

/// How the SDK is allowed to move room keys around on our behalf.
///
/// The defaults are all "do nothing", which is wrong for a chat client: with
/// `BackupDownloadStrategy::Manual` a freshly verified device never fetches the
/// keys for messages sent before it existed, so the history stays a wall of
/// "can't decrypt" no matter how the verification went.
///
/// `AfterDecryptionFailure` is the strategy Element X uses — a message we can't
/// read triggers a download of exactly that Megolm session, so history decrypts
/// as it is read rather than in one enormous burst at startup.
///
/// `auto_enable_backups` is the other half of the same problem. Without a backup
/// on the server there is nothing for a new device to download in the first
/// place, and the first anyone hears of it is the day they sign in somewhere
/// else. Creating the backup is safe on its own: the keys are encrypted with a
/// recovery key that never leaves the client, and the settings pane is still
/// where that key is shown to the user.
///
/// Cross-signing is *not* auto-bootstrapped: it needs interactive auth, which
/// means a password prompt we'd have to raise out of a background task.
fn encryption_settings() -> EncryptionSettings {
    EncryptionSettings {
        auto_enable_cross_signing: false,
        backup_download_strategy: BackupDownloadStrategy::AfterDecryptionFailure,
        auto_enable_backups: true,
    }
}

/// Ask a homeserver what it is and how one signs in to it.
pub async fn discover(server: &str) -> Result<HomeserverInfo> {
    let server = server.trim();
    if server.is_empty() {
        return Err(Error::Auth("enter a homeserver".into()));
    }

    let client = probe_client(server).await?;
    let homeserver_url = client.homeserver().to_string();

    let login_types = client
        .matrix_auth()
        .get_login_types()
        .await
        .map_err(|e| Error::Auth(format!("couldn't reach {server}: {e}")))?;

    let mut supports_password = false;
    let mut supports_sso = false;
    let mut sso_providers = Vec::new();

    for flow in &login_types.flows {
        match flow {
            LoginType::Password(_) => supports_password = true,
            LoginType::Sso(sso) => {
                supports_sso = true;
                sso_providers.extend(sso.identity_providers.iter().map(|p| SsoProvider {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    icon: p.icon.as_ref().map(|i| i.to_string()),
                }));
            }
            _ => {}
        }
    }

    Ok(HomeserverInfo {
        server_name: server.to_owned(),
        livekit_service_url: discover_livekit_focus(&homeserver_url).await,
        homeserver_url,
        supports_password,
        supports_sso,
        sso_providers,
    })
}

/// MatrixRTC advertises its SFU in the client `.well-known` under
/// `org.matrix.msc4143.rtc_foci`. Ruma doesn't model the unstable field, so we
/// read the raw JSON. A server without one simply has no SFU, which is not an
/// error — calls fall back to whatever the user configures.
pub async fn discover_livekit_focus(homeserver_url: &str) -> Option<String> {
    let base = url::Url::parse(homeserver_url).ok()?;
    let well_known = base.join("/.well-known/matrix/client").ok()?;

    let body: serde_json::Value = reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?
        .get(well_known)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let foci = body
        .get("org.matrix.msc4143.rtc_foci")
        .or_else(|| body.get("m.rtc_foci"))?
        .as_array()?;

    foci.iter()
        .find(|f| f.get("type").and_then(|t| t.as_str()) == Some("livekit"))
        .and_then(|f| f.get("livekit_service_url"))
        .and_then(|u| u.as_str())
        .map(str::to_owned)
}

/// Password sign-in. On success the session is persisted and syncing starts.
pub async fn login_password(
    app: tauri::AppHandle,
    state: &AppState,
    data_dir: &Path,
    server: &str,
    username: &str,
    password: &str,
) -> Result<SessionInfo> {
    // Serialised against restore and against a second click; building two
    // sessions on one store is what `session_gate` exists to prevent.
    let _gate = state.session_gate.lock().await;

    // Log in against a store-less client first: we can't name the store
    // directory until we know the user ID, and we don't want to create a store
    // for credentials that turn out to be wrong.
    let probe = probe_client(server).await?;
    let response = probe
        .matrix_auth()
        .login_username(username, password)
        .initial_device_display_name(DEVICE_NAME)
        .request_refresh_token()
        .send()
        .await
        .map_err(|e| Error::Auth(friendly_login_error(&e)))?;

    let homeserver = probe.homeserver().to_string();
    let session = MatrixSession::from(&response);
    finish_login(app, state, data_dir, &homeserver, session).await
}

/// SSO sign-in. The SDK spins up a loopback listener for the redirect; we just
/// have to get the user's browser to the URL.
pub async fn login_sso(
    app: tauri::AppHandle,
    state: &AppState,
    data_dir: &Path,
    server: &str,
    provider_id: Option<String>,
) -> Result<SessionInfo> {
    // Serialised against restore and against a second click; building two
    // sessions on one store is what `session_gate` exists to prevent.
    let _gate = state.session_gate.lock().await;

    use tauri_plugin_opener::OpenerExt;

    let probe = probe_client(server).await?;
    let opener = app.clone();

    let mut builder = probe.matrix_auth().login_sso(move |url: String| {
        let opener = opener.clone();
        async move {
            opener
                .opener()
                .open_url(url, None::<&str>)
                .map_err(|e| matrix_sdk::Error::UnknownError(Box::new(e)))
        }
    });
    builder = builder.initial_device_display_name(DEVICE_NAME);
    if let Some(provider_id) = &provider_id {
        builder = builder.identity_provider_id(provider_id);
    }

    let response =
        builder.send().await.map_err(|e| Error::Auth(format!("sso sign-in failed: {e}")))?;

    let homeserver = probe.homeserver().to_string();
    let session = MatrixSession::from(&response);
    finish_login(app, state, data_dir, &homeserver, session).await
}

/// Shared tail of both login flows: build the real, store-backed client,
/// restore the just-issued session into it, persist, and start syncing.
async fn finish_login(
    app: tauri::AppHandle,
    state: &AppState,
    data_dir: &Path,
    homeserver: &str,
    matrix_session: MatrixSession,
) -> Result<SessionInfo> {
    let store_path = session::new_store_path(data_dir, &matrix_session.meta.user_id)?;
    let passphrase = session::generate_passphrase();

    let client = Client::builder()
        .homeserver_url(homeserver)
        .sqlite_store(&store_path, Some(&passphrase))
        .user_agent(user_agent())
        .with_encryption_settings(encryption_settings())
        .handle_refresh_tokens()
        .build()
        .await?;

    client.matrix_auth().restore_session(matrix_session.clone(), RoomLoadSettings::All).await?;

    let pointer = session::save(data_dir, homeserver, &matrix_session, &passphrase)?;

    let core = bootstrap(app, client, pointer, data_dir.to_path_buf()).await?;
    let info = core.session_info().await?;
    state.set(core).await;
    Ok(info)
}

/// Bring back a previously saved session, if there is one.
pub async fn restore(
    app: tauri::AppHandle,
    state: &AppState,
    data_dir: &Path,
) -> Result<Option<SessionInfo>> {
    // One restore at a time, and never a second session on top of a live one.
    let _gate = state.session_gate.lock().await;
    if let Some(existing) = state.core.read().await.clone() {
        return Ok(Some(existing.session_info().await?));
    }

    let Some(restored) = session::load(data_dir)? else {
        return Ok(None);
    };

    let client = Client::builder()
        .homeserver_url(&restored.pointer.homeserver)
        .sqlite_store(&restored.store_path, Some(&restored.store_passphrase))
        .user_agent(user_agent())
        .with_encryption_settings(encryption_settings())
        .handle_refresh_tokens()
        .build()
        .await?;

    client.matrix_auth().restore_session(restored.session, RoomLoadSettings::All).await?;

    let core = bootstrap(app, client, restored.pointer, data_dir.to_path_buf()).await?;
    let info = core.session_info().await?;
    state.set(core).await;
    Ok(Some(info))
}

/// Sign out. `wipe` also deletes the local encrypted store — offer it when the
/// user is leaving a shared machine, but it destroys any room keys that aren't
/// in server-side key backup, so it must never be the default.
pub async fn logout(state: &AppState, data_dir: &Path, wipe: bool) -> Result<()> {
    let Some(core) = state.take().await else {
        return Ok(());
    };

    core.shutdown().await;

    // Tell the server to invalidate the token. If it fails (offline, token
    // already dead) we still drop the local session — refusing to sign out
    // because the network is down would be the wrong call.
    if let Err(e) = core.client.matrix_auth().logout().await {
        tracing::warn!("server-side logout failed, clearing locally anyway: {e}");
    }

    session::clear(data_dir, Some(&core.pointer.user_id))?;
    if wipe {
        session::wipe_store(data_dir, &core.pointer)?;
    }
    Ok(())
}

/// Marker recording that the poisoned-media purge below has already run.
const MEDIA_CACHE_FIX_MARKER: &str = "media-cache-decryption-fix";

/// Throw away media cached before attachments were decrypted properly.
///
/// The SDK keys its media cache by `mxc://` URI alone — a plain and an
/// encrypted source for the same URI share one entry. An earlier version of
/// this app fetched encrypted attachments as if they were plain, so the cache
/// holds *ciphertext* under those keys. Fetching correctly now still finds that
/// entry and returns the ciphertext, and a video made of ciphertext simply
/// refuses to play.
///
/// Nothing distinguishes a poisoned entry from a good one, so the only reliable
/// fix is to drop the lot once. Media is a cache: the worst case is
/// re-downloading it.
async fn purge_poisoned_media_cache(client: &Client, data_dir: &Path) {
    use matrix_sdk::media::MediaRetentionPolicy;

    let marker = data_dir.join(MEDIA_CACHE_FIX_MARKER);
    if marker.exists() {
        return;
    }

    let media = client.media();
    let previous = media.media_retention_policy().await.unwrap_or_default();

    // A zero-byte ceiling makes the cleanup evict everything.
    let purge = MediaRetentionPolicy::empty().with_max_cache_size(Some(0));
    if media.set_media_retention_policy(purge).await.is_ok() {
        if let Err(e) = media.clean().await {
            tracing::warn!("couldn't purge the media cache: {e}");
            return;
        }
    }

    let _ = media.set_media_retention_policy(previous).await;

    // Only mark it done once the purge actually succeeded, so a failure here
    // gets another attempt next launch rather than leaving media broken.
    if let Err(e) = std::fs::write(&marker, b"done") {
        tracing::warn!("couldn't record the media cache fix: {e}");
    }
    tracing::info!("purged media cached before attachment decryption was fixed");
}

/// Construct the `MatrixCore` and start every background stream the UI relies on.
pub async fn bootstrap(
    app: tauri::AppHandle,
    client: Client,
    pointer: session::SessionPointer,
    data_dir: PathBuf,
) -> Result<Arc<MatrixCore>> {
    purge_poisoned_media_cache(&client, &data_dir).await;

    let sync_service = SyncService::builder(client.clone())
        .with_offline_mode()
        .build()
        .await
        .map_err(|e| Error::Other(format!("couldn't start sync: {e}")))?;
    let sync_service = Arc::new(sync_service);
    let room_list = sync_service.room_list_service();

    let core = Arc::new(MatrixCore {
        client,
        sync_service: sync_service.clone(),
        room_list,
        pointer,
        data_dir,
        rooms: Mutex::new(Default::default()),
        timelines: Mutex::new(Default::default()),
        presence: Default::default(),
        tasks: Mutex::new(Vec::new()),
        shutting_down: AtomicBool::new(false),
    });

    sync_service.start().await;

    let mut tasks = Vec::new();
    tasks.push(events::spawn_sync_status_task(app.clone(), core.clone()));
    tasks.push(rooms::spawn_room_list_task(app.clone(), core.clone()));
    tasks.push(events::spawn_verification_task(app.clone(), core.clone()));
    tasks.push(presence::spawn_presence_task(app.clone(), core.clone()));
    core.tasks.lock().await.extend(tasks);

    Ok(core)
}

/// Turn the SDK's error into something worth showing a person.
///
/// Anything we don't specifically recognise falls through to the server's own
/// message, which is usually more informative than a generic "login failed".
fn friendly_login_error(error: &matrix_sdk::Error) -> String {
    use matrix_sdk::ruma::api::error::ErrorKind;

    match error.client_api_error_kind() {
        Some(ErrorKind::Forbidden { .. }) => "wrong username or password".to_owned(),
        Some(ErrorKind::UserDeactivated) => "that account has been deactivated".to_owned(),
        Some(ErrorKind::LimitExceeded { .. }) => "too many attempts — wait a moment".to_owned(),
        _ => error.to_string(),
    }
}

/// Resolve a room alias or ID to a room ID.
pub async fn resolve_room(
    client: &Client,
    alias_or_id: &str,
) -> Result<matrix_sdk::ruma::OwnedRoomId> {
    if let Ok(room_id) = RoomId::parse(alias_or_id) {
        return Ok(room_id);
    }
    let alias = matrix_sdk::ruma::RoomAliasId::parse(alias_or_id)?;
    Ok(client.resolve_room_alias(&alias).await?.room_id)
}
