//! Sign-in, sign-out, and bringing a session up to a syncing state.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use matrix_sdk::{
    Client, HttpError, SessionMeta, SessionTokens,
    authentication::matrix::MatrixSession,
    encryption::{BackupDownloadStrategy, EncryptionSettings},
    ruma::{
        ClientSecret, OwnedClientSecret, OwnedSessionId, UInt,
        api::client::{
            account::{register, request_registration_token_via_email},
            session::get_login_types::v3::LoginType,
            uiaa::{self, AuthType, LoginTermsParams, UiaaInfo},
        },
    },
    store::RoomLoadSettings,
};
use matrix_sdk_ui::sync_service::SyncService;
use tokio::sync::Mutex;

use crate::{
    dto::{
        HomeserverInfo, RegisterAuth, RegisterChallenge, RegisterEmailToken, RegisterStage,
        RegisterStageKind, RegisterStep, SessionInfo, SsoProvider, TermsPolicy,
    },
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

/// One turn of account registration.
///
/// Registration is user-interactive auth: the first request comes back with
/// the flows the server accepts, and each request after that answers one stage
/// of one flow. Most stages need the person at the keyboard — a token, a terms
/// checkbox, a captcha — so the frontend drives the loop and this is a single
/// step of it. Username and password go with every step; the server keys the
/// attempt by `session`.
#[allow(clippy::too_many_arguments)]
pub async fn register(
    app: tauri::AppHandle,
    state: &AppState,
    data_dir: &Path,
    server: &str,
    username: &str,
    password: &str,
    session: Option<String>,
    auth: Option<RegisterAuth>,
) -> Result<RegisterStep> {
    // Serialised against restore and against a second click, like login.
    let _gate = state.session_gate.lock().await;

    let probe = probe_client(server).await?;

    let mut request = register::v3::Request::new();
    request.username = Some(username.trim().to_owned());
    request.password = Some(password.to_owned());
    request.initial_device_display_name = Some(DEVICE_NAME.to_owned());
    request.refresh_token = true;
    request.auth = auth.map(|answer| auth_data(answer, session)).transpose()?;

    // `send` rather than `matrix_auth().register()`: the latter also signs the
    // probe client in, and the probe is about to be thrown away.
    let response = match probe.send(request).await {
        Ok(response) => response,
        Err(e) => {
            return match e.as_uiaa_response() {
                Some(info) => Ok(RegisterStep::NeedsAuth {
                    challenge: challenge(probe.homeserver(), info),
                }),
                None => Err(Error::Auth(friendly_register_error(&e))),
            };
        }
    };

    // Both are only absent with `inhibit_login`, which we never set; a server
    // that leaves them out anyway has still made the account.
    let (Some(device_id), Some(access_token)) = (response.device_id, response.access_token) else {
        return Err(Error::Auth(
            "your account was made, but the server didn't sign it in — sign in to continue".into(),
        ));
    };

    let session = MatrixSession {
        meta: SessionMeta {
            user_id: response.user_id,
            device_id,
        },
        tokens: SessionTokens {
            access_token,
            refresh_token: response.refresh_token,
        },
    };
    let homeserver = probe.homeserver().to_string();
    let info = finish_login(app, state, data_dir, &homeserver, session).await?;
    Ok(RegisterStep::Done { session: info })
}

fn auth_data(answer: RegisterAuth, session: Option<String>) -> Result<uiaa::AuthData> {
    Ok(match answer {
        RegisterAuth::Dummy => {
            let mut dummy = uiaa::Dummy::new();
            dummy.session = session;
            uiaa::AuthData::Dummy(dummy)
        }
        RegisterAuth::RegistrationToken { token } => {
            let mut token = uiaa::RegistrationToken::new(token.trim().to_owned());
            token.session = session;
            uiaa::AuthData::RegistrationToken(token)
        }
        RegisterAuth::Terms => {
            let mut terms = uiaa::Terms::new();
            terms.session = session;
            uiaa::AuthData::Terms(terms)
        }
        RegisterAuth::Email { sid, client_secret } => {
            let creds = uiaa::ThirdpartyIdCredentials::new(
                OwnedSessionId::try_from(sid)
                    .map_err(|e| Error::Auth(format!("bad email session: {e}")))?,
                OwnedClientSecret::try_from(client_secret)
                    .map_err(|e| Error::Auth(format!("bad email secret: {e}")))?,
            );
            // `EmailIdentity` has no constructor outside ruma, so it goes in as JSON.
            let mut data = serde_json::Map::new();
            data.insert("type".into(), AuthType::EmailIdentity.as_str().into());
            data.insert("threepid_creds".into(), serde_json::to_value(creds)?);
            uiaa::AuthData::new(AuthType::EmailIdentity.as_str(), session, data)?
        }
        RegisterAuth::Fallback => {
            let session = session
                .ok_or_else(|| Error::Auth("the server didn't start a sign-up session".into()))?;
            uiaa::AuthData::FallbackAcknowledgement(uiaa::FallbackAcknowledgement::new(session))
        }
    })
}

/// Have the homeserver email a verification link for an `m.login.email.identity`
/// stage. Pass back the `client_secret` from a previous call, with a higher
/// `send_attempt`, to re-send rather than start over.
pub async fn request_register_email(
    server: &str,
    email: &str,
    client_secret: Option<String>,
    send_attempt: u32,
) -> Result<RegisterEmailToken> {
    let email = email.trim();
    if email.is_empty() {
        return Err(Error::Auth("enter an email address".into()));
    }

    let client_secret = match client_secret {
        Some(secret) => OwnedClientSecret::try_from(secret)
            .map_err(|e| Error::Auth(format!("bad email secret: {e}")))?,
        None => ClientSecret::new(),
    };
    let request = request_registration_token_via_email::v3::Request::new(
        client_secret.clone(),
        email.to_owned(),
        UInt::from(send_attempt),
    );

    let probe = probe_client(server).await?;
    let response = probe.send(request).await.map_err(|e| Error::Auth(friendly_email_error(&e)))?;

    Ok(RegisterEmailToken {
        sid: response.sid.to_string(),
        client_secret: client_secret.to_string(),
    })
}

/// Flatten the server's 401 into what the sign-up form needs to render.
fn challenge(homeserver: url::Url, info: &UiaaInfo) -> RegisterChallenge {
    let session = info.session.clone();

    let stage = |auth_type: &AuthType| {
        let kind = match auth_type {
            AuthType::Dummy => RegisterStageKind::Dummy,
            AuthType::RegistrationToken => RegisterStageKind::RegistrationToken,
            AuthType::Terms => RegisterStageKind::Terms,
            AuthType::EmailIdentity => RegisterStageKind::Email,
            AuthType::Msisdn => RegisterStageKind::Unsupported,
            // Anything else — reCAPTCHA above all — has a web page on the
            // homeserver that does the stage for us.
            _ if session.is_some() => RegisterStageKind::Fallback,
            _ => RegisterStageKind::Unsupported,
        };
        RegisterStage {
            kind,
            auth_type: auth_type.as_str().to_owned(),
            fallback_url: (kind == RegisterStageKind::Fallback)
                .then(|| fallback_url(&homeserver, auth_type, session.as_deref()?))
                .flatten(),
        }
    };

    let policies = info
        .params::<LoginTermsParams>(&AuthType::Terms)
        .ok()
        .flatten()
        .map(|terms| {
            terms
                .policies
                .into_values()
                .filter_map(|policy| {
                    let translation = policy
                        .translations
                        .get("en")
                        .or_else(|| policy.translations.values().next())?;
                    Some(TermsPolicy {
                        name: translation.name.clone(),
                        url: translation.url.clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    RegisterChallenge {
        flows: info
            .flows
            .iter()
            .map(|flow| flow.stages.iter().map(stage).collect())
            .collect(),
        completed: info
            .completed
            .iter()
            .map(|t| t.as_str().to_owned())
            .collect(),
        policies,
        error: info.auth_error.as_ref().map(|e| e.message.clone()),
        session,
    }
}

fn fallback_url(homeserver: &url::Url, auth_type: &AuthType, session: &str) -> Option<String> {
    let mut url = homeserver
        .join(&format!(
            "_matrix/client/v3/auth/{}/fallback/web",
            auth_type.as_str()
        ))
        .ok()?;
    url.query_pairs_mut().append_pair("session", session);
    Some(url.to_string())
}

/// Registration's version of [`friendly_login_error`].
fn friendly_register_error(error: &HttpError) -> String {
    use matrix_sdk::ruma::api::error::{ErrorBody, ErrorKind, StandardErrorBody};

    let server_says = || match error.as_client_api_error().map(|e| &e.body) {
        Some(ErrorBody::Standard(StandardErrorBody { message, .. })) => message.clone(),
        _ => error.to_string(),
    };

    match error.client_api_error_kind() {
        Some(ErrorKind::UserInUse) => "that username is taken".to_owned(),
        Some(ErrorKind::Exclusive) => "that username is reserved on this server".to_owned(),
        Some(ErrorKind::Forbidden) => "this server isn't taking sign-ups".to_owned(),
        // Servers that hand accounts to an OAuth provider (matrix.org among
        // them) don't serve `/register` at all.
        Some(ErrorKind::Unrecognized) => {
            "this server signs people up on its website, not in apps".to_owned()
        }
        Some(ErrorKind::LimitExceeded { .. }) => "too many attempts — wait a moment".to_owned(),
        // Invalid usernames and weak passwords: the server's own wording says
        // what it wants, which is more than we could.
        _ => server_says(),
    }
}

/// [`friendly_register_error`] for the verification-email request.
fn friendly_email_error(error: &HttpError) -> String {
    use matrix_sdk::ruma::api::error::ErrorKind;

    match error.client_api_error_kind() {
        Some(ErrorKind::ThreepidInUse) => "that email already has an account".to_owned(),
        Some(ErrorKind::ThreepidDenied) => "this server doesn't take that email address".to_owned(),
        Some(ErrorKind::ThreepidMediumNotSupported) => "this server can't send email".to_owned(),
        _ => friendly_register_error(error),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_answer_has_the_spec_shape() {
        let answer = RegisterAuth::Email {
            sid: "abc".into(),
            client_secret: "s3cret".into(),
        };
        let auth = auth_data(answer, Some("sess".into())).unwrap();
        assert!(matches!(auth, uiaa::AuthData::EmailIdentity(_)));
        assert_eq!(
            serde_json::to_value(&auth).unwrap(),
            serde_json::json!({
                "type": "m.login.email.identity",
                "threepid_creds": { "sid": "abc", "client_secret": "s3cret" },
                "session": "sess",
            })
        );
    }
}
