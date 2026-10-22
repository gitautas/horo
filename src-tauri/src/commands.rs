//! Every Tauri command the frontend can call.
//!
//! These are deliberately thin: parse and validate arguments, then delegate.
//! Anything with real logic lives in `matrix::*`, `rtc` or `verification`.

use matrix_sdk::ruma::{OwnedRoomId, RoomId};
use tauri::{AppHandle, Manager, State};

use crate::{
    dto::{
        HomeserverInfo, NotifyMode, RegisterAuth, RegisterEmailToken, RegisterStep, RoomMemberDto, RoomsSnapshot,
        SendOptions, SessionInfo, SpaceSummary, StickerOptions, TimelineItemDto,
    },
    error::{Error, Result},
    matrix::{auth, core::AppState, directory, media, packs, presence, profile, rooms, timeline},
    rtc, verification,
};

/// Where the session pointer and the encrypted stores live.
fn data_dir(app: &AppHandle) -> Result<std::path::PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| Error::Other(format!("no app data directory: {e}")))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn parse_room_id(raw: &str) -> Result<OwnedRoomId> {
    RoomId::parse(raw).map_err(Error::Id)
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn discover_homeserver(server: String) -> Result<HomeserverInfo> {
    auth::discover(&server).await
}

#[tauri::command]
pub async fn login_password(
    app: AppHandle,
    state: State<'_, AppState>,
    server: String,
    username: String,
    password: String,
) -> Result<SessionInfo> {
    let dir = data_dir(&app)?;
    auth::login_password(app.clone(), &state, &dir, &server, &username, &password).await
}

#[tauri::command]
pub async fn login_sso(
    app: AppHandle,
    state: State<'_, AppState>,
    server: String,
    provider_id: Option<String>,
) -> Result<SessionInfo> {
    let dir = data_dir(&app)?;
    auth::login_sso(app.clone(), &state, &dir, &server, provider_id).await
}

#[tauri::command]
pub async fn register_account(
    app: AppHandle,
    state: State<'_, AppState>,
    server: String,
    username: String,
    password: String,
    session: Option<String>,
    auth: Option<RegisterAuth>,
) -> Result<RegisterStep> {
    let dir = data_dir(&app)?;
    auth::register(
        app.clone(),
        &state,
        &dir,
        &server,
        &username,
        &password,
        session,
        auth,
    )
    .await
}

#[tauri::command]
pub async fn request_register_email(
    server: String,
    email: String,
    client_secret: Option<String>,
    send_attempt: u32,
) -> Result<RegisterEmailToken> {
    auth::request_register_email(&server, &email, client_secret, send_attempt).await
}

#[tauri::command]
pub async fn restore_session(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<SessionInfo>> {
    let dir = data_dir(&app)?;
    auth::restore(app.clone(), &state, &dir).await
}

#[tauri::command]
pub async fn logout(app: AppHandle, state: State<'_, AppState>, wipe: bool) -> Result<()> {
    let dir = data_dir(&app)?;
    auth::logout(&state, &dir, wipe).await
}

#[tauri::command]
pub async fn current_session(state: State<'_, AppState>) -> Result<Option<SessionInfo>> {
    match state.core.read().await.clone() {
        Some(core) => Ok(Some(core.session_info().await?)),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// rooms
// ---------------------------------------------------------------------------

/// The current room list.
///
/// Sync begins when the session is restored, so the initial push can land
/// before the UI is listening. The UI calls this once it has subscribed, and
/// applies diffs from then on.
#[tauri::command]
pub async fn get_rooms(state: State<'_, AppState>) -> Result<RoomsSnapshot> {
    let core = state.core().await?;
    let mirror = core.rooms.lock().await;
    Ok(RoomsSnapshot { seq: mirror.seq, rooms: mirror.rooms.clone() })
}

#[tauri::command]
pub async fn get_spaces(state: State<'_, AppState>) -> Result<Vec<SpaceSummary>> {
    rooms::spaces(&*state.core().await?).await
}

#[tauri::command]
pub async fn get_members(
    state: State<'_, AppState>,
    room_id: String,
) -> Result<Vec<RoomMemberDto>> {
    rooms::members(&*state.core().await?, &parse_room_id(&room_id)?).await
}

/// Make a room, optionally filing it under the space that's open.
#[tauri::command]
pub async fn create_room(
    state: State<'_, AppState>,
    room: rooms::NewRoom,
) -> Result<rooms::NewRoomResult> {
    rooms::create(&*state.core().await?, room).await
}

/// Join by alias or ID. `via` names servers to try, for a room ID ours
/// doesn't know yet — one found in another server's directory, say.
#[tauri::command]
pub async fn join_room(
    state: State<'_, AppState>,
    alias_or_id: String,
    via: Option<Vec<String>>,
) -> Result<String> {
    rooms::join(&*state.core().await?, &alias_or_id, &via.unwrap_or_default()).await
}

/// One page of public spaces from a server's directory — ours when `server` is
/// omitted.
#[tauri::command]
pub async fn get_public_spaces(
    state: State<'_, AppState>,
    server: Option<String>,
    search: Option<String>,
    since: Option<String>,
) -> Result<directory::DirectoryPage> {
    directory::public_spaces(&*state.core().await?, server, search, since).await
}

/// The rooms inside a space, readable before joining it.
#[tauri::command]
pub async fn get_space_rooms(
    state: State<'_, AppState>,
    space_id: String,
) -> Result<Vec<directory::PublicRoom>> {
    directory::space_rooms(&*state.core().await?, &parse_room_id(&space_id)?).await
}

/// People the user directory matches, for suggesting while an ID is typed.
#[tauri::command]
pub async fn search_users(
    state: State<'_, AppState>,
    term: String,
) -> Result<Vec<directory::UserSuggestion>> {
    directory::search_users(&*state.core().await?, &term).await
}

/// Join a space and the rooms in it — see `rooms::join_space`.
#[tauri::command]
pub async fn join_space(
    state: State<'_, AppState>,
    space_id: String,
    via: Option<Vec<String>>,
) -> Result<rooms::JoinSpaceResult> {
    rooms::join_space(&*state.core().await?, &parse_room_id(&space_id)?, &via.unwrap_or_default())
        .await
}

/// Let a space's members into its invite-only rooms.
#[tauri::command]
pub async fn open_space_rooms(
    state: State<'_, AppState>,
    space_id: String,
) -> Result<rooms::OpenedRooms> {
    rooms::open_space_rooms(&*state.core().await?, &parse_room_id(&space_id)?).await
}

/// Leave a room. Doesn't forget it — see `rooms::leave`.
#[tauri::command]
pub async fn leave_room(state: State<'_, AppState>, room_id: String) -> Result<()> {
    rooms::leave(&*state.core().await?, &parse_room_id(&room_id)?).await
}

/// Delete a room as far as Matrix allows — see `rooms::delete`.
#[tauri::command]
pub async fn delete_room(state: State<'_, AppState>, room_id: String) -> Result<()> {
    rooms::delete(&*state.core().await?, &parse_room_id(&room_id)?).await
}

/// Rename a room or change its topic. Omitted fields are left alone.
#[tauri::command]
pub async fn update_room(
    state: State<'_, AppState>,
    room_id: String,
    name: Option<String>,
    topic: Option<String>,
) -> Result<()> {
    rooms::update(&*state.core().await?, &parse_room_id(&room_id)?, name, topic).await
}

/// What this account may change about a room.
#[tauri::command]
pub async fn get_room_permissions(
    state: State<'_, AppState>,
    room_id: String,
) -> Result<rooms::RoomPermissions> {
    rooms::permissions(&*state.core().await?, &parse_room_id(&room_id)?).await
}

#[tauri::command]
pub async fn invite_user(
    state: State<'_, AppState>,
    room_id: String,
    user_id: String,
) -> Result<()> {
    rooms::invite(&*state.core().await?, &parse_room_id(&room_id)?, &user_id).await
}

#[tauri::command]
pub async fn set_typing(
    state: State<'_, AppState>,
    room_id: String,
    typing: bool,
) -> Result<()> {
    rooms::set_typing(&*state.core().await?, &parse_room_id(&room_id)?, typing).await
}

#[tauri::command]
pub async fn set_room_muted(
    app: AppHandle,
    state: State<'_, AppState>,
    room_id: String,
    muted: bool,
) -> Result<()> {
    let core = state.core().await?;
    let room_id = parse_room_id(&room_id)?;

    rooms::set_muted(&core, &room_id, muted).await?;
    // Muting writes push rules, which the room-list stream has no reason to
    // report; without this the toggle only catches up when something else about
    // the room changes.
    rooms::emit_room_refresh(&app, &core, &room_id).await;
    Ok(())
}

/// Set the notification level on several rooms at once — every room in a
/// space, say. `None` returns them to the default.
#[tauri::command]
pub async fn set_rooms_notify_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    room_ids: Vec<String>,
    mode: Option<NotifyMode>,
) -> Result<()> {
    let core = state.core().await?;
    each_room(&app, &core, &room_ids, |core, room_id| async move {
        match mode {
            Some(mode) => rooms::set_notify_mode(core, &room_id, mode).await,
            // No mode: back to the default, which is what unmuting means.
            None => rooms::set_muted(core, &room_id, false).await,
        }
    })
    .await
}

/// Mark several rooms read at once — a whole space, say — without opening any.
#[tauri::command]
pub async fn mark_rooms_read(
    app: AppHandle,
    state: State<'_, AppState>,
    room_ids: Vec<String>,
) -> Result<()> {
    let core = state.core().await?;
    each_room(&app, &core, &room_ids, |core, room_id| async move {
        rooms::mark_read(core, &room_id).await
    })
    .await
}

/// Run `action` on each room in turn, then refresh it in the sidebar.
///
/// One at a time: push rules are a single account-wide ruleset the SDK caches,
/// and concurrent writes to it race. A room that fails doesn't stop the rest;
/// the first error is reported once they've all had their turn.
async fn each_room<'a, F, Fut>(
    app: &AppHandle,
    core: &'a crate::matrix::MatrixCore,
    room_ids: &[String],
    action: F,
) -> Result<()>
where
    F: Fn(&'a crate::matrix::MatrixCore, OwnedRoomId) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let mut first_error = None;
    for raw in room_ids {
        let room_id = parse_room_id(raw)?;
        if let Err(e) = action(core, room_id.clone()).await {
            tracing::warn!("{room_id}: {e}");
            first_error.get_or_insert(e);
        }
        rooms::emit_room_refresh(app, core, &room_id).await;
    }
    first_error.map_or(Ok(()), Err)
}

#[tauri::command]
pub async fn set_room_marked_unread(
    state: State<'_, AppState>,
    room_id: String,
    unread: bool,
) -> Result<()> {
    rooms::set_marked_unread(&*state.core().await?, &parse_room_id(&room_id)?, unread).await
}

// ---------------------------------------------------------------------------
// image packs
// ---------------------------------------------------------------------------

/// Every custom emote and sticker pack usable right now.
///
/// `room_id` is the room being looked at, whose own packs are available whether
/// or not they've been enabled globally.
#[tauri::command]
pub async fn get_image_packs(
    state: State<'_, AppState>,
    room_id: Option<String>,
) -> Result<Vec<packs::ImagePackDto>> {
    let room_id = room_id.as_deref().map(parse_room_id).transpose()?;
    packs::available(&*state.core().await?, room_id.as_deref()).await
}

/// Every pack on the account, including ones not turned on — what the settings
/// screen lists.
#[tauri::command]
pub async fn get_all_image_packs(state: State<'_, AppState>) -> Result<Vec<packs::ImagePackDto>> {
    packs::all(&*state.core().await?).await
}

/// Change one thing about one pack.
#[tauri::command]
pub async fn edit_image_pack(
    state: State<'_, AppState>,
    target: packs::PackTarget,
    edit: packs::PackEdit,
) -> Result<()> {
    packs::edit(&*state.core().await?, target, edit).await
}

/// Carry a room's pack everywhere, or stop.
#[tauri::command]
pub async fn set_pack_everywhere(
    state: State<'_, AppState>,
    room_id: String,
    state_key: String,
    everywhere: bool,
) -> Result<()> {
    packs::set_everywhere(
        &*state.core().await?,
        &parse_room_id(&room_id)?,
        &state_key,
        everywhere,
    )
    .await
}

/// Make a pack of your own, in a room created to hold it.
#[tauri::command]
pub async fn create_personal_pack(state: State<'_, AppState>, name: String) -> Result<String> {
    packs::create_personal_pack(&*state.core().await?, &name).await
}

/// Rooms this account may create or edit packs in.
#[tauri::command]
pub async fn get_pack_rooms(state: State<'_, AppState>) -> Result<Vec<packs::PackRoomDto>> {
    packs::editable_rooms(&*state.core().await?).await
}

// ---------------------------------------------------------------------------
// timeline
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn open_timeline(
    app: AppHandle,
    state: State<'_, AppState>,
    room_id: String,
    thread_root: Option<String>,
) -> Result<Vec<TimelineItemDto>> {
    let core = state.core().await?;
    timeline::open(&app, &core, &parse_room_id(&room_id)?, thread_root).await
}

#[tauri::command]
pub async fn close_timeline(
    state: State<'_, AppState>,
    room_id: String,
    thread_root: Option<String>,
) -> Result<()> {
    timeline::close(&*state.core().await?, &parse_room_id(&room_id)?, thread_root.as_deref()).await
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    room_id: String,
    options: SendOptions,
) -> Result<()> {
    timeline::send(&*state.core().await?, &parse_room_id(&room_id)?, options).await
}

#[tauri::command]
pub async fn send_sticker(
    state: State<'_, AppState>,
    room_id: String,
    thread_root: Option<String>,
    sticker: StickerOptions,
) -> Result<()> {
    timeline::send_sticker(
        &*state.core().await?,
        &parse_room_id(&room_id)?,
        thread_root.as_deref(),
        sticker,
    )
    .await
}

#[tauri::command]
pub async fn paginate_back(
    state: State<'_, AppState>,
    room_id: String,
    thread_root: Option<String>,
) -> Result<bool> {
    timeline::paginate_back(&*state.core().await?, &parse_room_id(&room_id)?, thread_root.as_deref()).await
}

#[tauri::command]
pub async fn toggle_reaction(
    state: State<'_, AppState>,
    room_id: String,
    thread_root: Option<String>,
    event_id: String,
    key: String,
) -> Result<bool> {
    timeline::toggle_reaction(
        &*state.core().await?,
        &parse_room_id(&room_id)?,
        thread_root.as_deref(),
        &event_id,
        &key,
    )
    .await
}

#[tauri::command]
pub async fn edit_message(
    state: State<'_, AppState>,
    room_id: String,
    thread_root: Option<String>,
    event_id: String,
    body: String,
    markdown: bool,
) -> Result<()> {
    timeline::edit(
        &*state.core().await?,
        &parse_room_id(&room_id)?,
        thread_root.as_deref(),
        &event_id,
        &body,
        markdown,
    )
    .await
}

#[tauri::command]
pub async fn redact_event(
    state: State<'_, AppState>,
    room_id: String,
    thread_root: Option<String>,
    event_id: String,
    reason: Option<String>,
) -> Result<()> {
    timeline::redact(
        &*state.core().await?,
        &parse_room_id(&room_id)?,
        thread_root.as_deref(),
        &event_id,
        reason.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn mark_room_read(state: State<'_, AppState>, room_id: String) -> Result<()> {
    timeline::mark_as_read(&*state.core().await?, &parse_room_id(&room_id)?).await
}

#[tauri::command]
pub async fn send_read_receipt(
    state: State<'_, AppState>,
    room_id: String,
    event_id: String,
) -> Result<()> {
    timeline::send_read_receipt(&*state.core().await?, &parse_room_id(&room_id)?, &event_id).await
}

#[tauri::command]
pub async fn send_attachment(
    state: State<'_, AppState>,
    room_id: String,
    thread_root: Option<String>,
    path: String,
    caption: Option<String>,
) -> Result<()> {
    timeline::send_attachment(
        &*state.core().await?,
        &parse_room_id(&room_id)?,
        thread_root.as_deref(),
        &path,
        caption,
    )
    .await
}

/// Send an attachment the WebView holds as bytes rather than as a path — a
/// pasted screenshot, a file copied out of another app.
///
/// The bytes ride in the request body instead of the JSON arguments: a
/// serialised byte array costs several times the size of the file it carries,
/// and a pasted video is not small. Everything else travels as headers, which
/// are ASCII, so the filename is percent-encoded the same way the frontend
/// encodes an mxc URI.
#[tauri::command]
pub async fn send_attachment_bytes(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<()> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(Error::Other("expected the file's bytes as the request body".into()));
    };

    let header = |name: &str| {
        request.headers().get(name).and_then(|v| v.to_str().ok()).filter(|v| !v.is_empty())
    };
    let decoded = |name: &str| header(name).and_then(media::percent_decode);

    let room_id = header("x-room-id")
        .ok_or_else(|| Error::Other("no room to send that to".into()))?;
    let filename = decoded("x-filename").unwrap_or_else(|| "attachment".to_owned());

    timeline::send_attachment_data(
        &*state.core().await?,
        &parse_room_id(room_id)?,
        header("x-thread-root"),
        &filename,
        header("x-mime"),
        bytes.clone(),
        decoded("x-caption"),
    )
    .await
}

#[tauri::command]
pub async fn get_pinned_events(
    state: State<'_, AppState>,
    room_id: String,
) -> Result<Vec<TimelineItemDto>> {
    timeline::pinned_events(&*state.core().await?, &parse_room_id(&room_id)?).await
}

#[tauri::command]
pub async fn get_event_body(
    state: State<'_, AppState>,
    room_id: String,
    event_id: String,
) -> Result<Option<String>> {
    timeline::event_body(&*state.core().await?, &parse_room_id(&room_id)?, &event_id).await
}

// ---------------------------------------------------------------------------
// media
// ---------------------------------------------------------------------------

/// The raw bytes of a piece of media, for `<video>` and `<audio>`.
///
/// Images load fine from the `horo://` scheme, but WKWebView hands media
/// elements to AVFoundation, which never consults a custom scheme handler — the
/// request simply never arrives and the element reports "format not supported".
/// So media comes over IPC instead and the frontend wraps it in a `blob:` URL,
/// which AVFoundation will load.
///
/// It's also how the frontend reads an image's pixels: one loaded from
/// `horo://` is cross-origin, so a canvas it's drawn into can't be read back.
#[tauri::command]
pub async fn get_media_bytes(
    state: State<'_, AppState>,
    mxc: String,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<tauri::ipc::Response> {
    // A size asks for the same thumbnail an `<img>` would get, so a caller that
    // wants to look at what's already on screen hits the media cache.
    let size = width.zip(height);
    let media = media::fetch(&state.core().await?.client, &mxc, size, None).await?;
    Ok(tauri::ipc::Response::new(media.bytes))
}

// ---------------------------------------------------------------------------
// presence
// ---------------------------------------------------------------------------

/// Replace the set of people whose presence the UI wants pushed to it.
///
/// The frontend sends whoever is on screen; see `lib/presence.ts`.
#[tauri::command]
pub async fn watch_presence(
    state: State<'_, AppState>,
    user_ids: Vec<String>,
) -> Result<()> {
    presence::watch(&*state.core().await?, user_ids).await
}

/// Everything known right now, for a frontend that has just mounted.
#[tauri::command]
pub async fn get_presence(
    state: State<'_, AppState>,
) -> Result<Vec<presence::PresenceDto>> {
    presence::snapshot(&*state.core().await?).await
}

/// Publish our own presence. Driven by the frontend's idle timer.
#[tauri::command]
pub async fn set_own_presence(
    state: State<'_, AppState>,
    presence: presence::OwnPresence,
) -> Result<()> {
    crate::matrix::presence::set_own(&*state.core().await?, presence).await
}

// ---------------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_profile(
    state: State<'_, AppState>,
    user_id: Option<String>,
) -> Result<profile::ProfileDto> {
    profile::get_profile(&*state.core().await?, user_id).await
}

#[tauri::command]
pub async fn set_profile(
    state: State<'_, AppState>,
    update: profile::ProfileUpdate,
) -> Result<()> {
    profile::set_profile(&*state.core().await?, update).await
}

/// What we know about a person locally — verification, shared rooms, whether a
/// DM already exists. Companion to `get_profile`, which is the server's half.
#[tauri::command]
pub async fn get_user_context(
    state: State<'_, AppState>,
    user_id: String,
) -> Result<profile::UserContextDto> {
    profile::get_user_context(&*state.core().await?, user_id).await
}

/// The DM with this person, created if there isn't one yet.
#[tauri::command]
pub async fn open_dm(state: State<'_, AppState>, user_id: String) -> Result<String> {
    profile::open_dm(&*state.core().await?, user_id).await
}

#[tauri::command]
pub async fn upload_media(state: State<'_, AppState>, path: String) -> Result<String> {
    profile::upload_media(&*state.core().await?, &path).await
}

#[tauri::command]
pub async fn save_media(
    state: State<'_, AppState>,
    mxc: String,
    destination: String,
) -> Result<()> {
    media::save_to(&state.core().await?.client, &mxc, &destination).await
}

// ---------------------------------------------------------------------------
// verification
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn request_verification(
    app: AppHandle,
    state: State<'_, AppState>,
    user_id: Option<String>,
) -> Result<()> {
    verification::request(&app, &state.core().await?, user_id).await
}

#[tauri::command]
pub async fn accept_verification(
    state: State<'_, AppState>,
    user_id: String,
    flow_id: String,
) -> Result<()> {
    verification::accept(&*state.core().await?, &user_id, &flow_id).await
}

#[tauri::command]
pub async fn confirm_verification(
    state: State<'_, AppState>,
    user_id: String,
    flow_id: String,
) -> Result<()> {
    verification::confirm(&*state.core().await?, &user_id, &flow_id).await
}

#[tauri::command]
pub async fn mismatch_verification(
    state: State<'_, AppState>,
    user_id: String,
    flow_id: String,
) -> Result<()> {
    verification::mismatch(&*state.core().await?, &user_id, &flow_id).await
}

#[tauri::command]
pub async fn cancel_verification(
    state: State<'_, AppState>,
    user_id: String,
    flow_id: String,
) -> Result<()> {
    verification::cancel(&*state.core().await?, &user_id, &flow_id).await
}

#[tauri::command]
pub async fn get_own_devices(
    state: State<'_, AppState>,
) -> Result<Vec<verification::DeviceInfo>> {
    verification::own_devices(&*state.core().await?).await
}

#[tauri::command]
pub async fn verify_device(
    app: AppHandle,
    state: State<'_, AppState>,
    device_id: String,
) -> Result<()> {
    verification::verify_device(&app, &state.core().await?, &device_id).await
}

#[tauri::command]
pub async fn get_recovery_status(
    state: State<'_, AppState>,
) -> Result<verification::RecoveryStatus> {
    verification::recovery_status(&*state.core().await?).await
}

#[tauri::command]
pub async fn enable_recovery(state: State<'_, AppState>) -> Result<String> {
    verification::enable_recovery(&*state.core().await?).await
}

#[tauri::command]
pub async fn recover_with_key(state: State<'_, AppState>, recovery_key: String) -> Result<()> {
    verification::recover(&state.core().await?, &recovery_key).await
}

// ---------------------------------------------------------------------------
// calls
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn join_call(
    state: State<'_, AppState>,
    room_id: String,
    video: Option<bool>,
) -> Result<rtc::CallCredentials> {
    rtc::join(&*state.core().await?, &parse_room_id(&room_id)?, video.unwrap_or(false)).await
}

#[tauri::command]
pub async fn leave_call(state: State<'_, AppState>, room_id: String) -> Result<()> {
    rtc::leave(&*state.core().await?, &parse_room_id(&room_id)?).await
}

#[tauri::command]
pub async fn refresh_call_membership(
    state: State<'_, AppState>,
    room_id: String,
) -> Result<()> {
    rtc::refresh(&*state.core().await?, &parse_room_id(&room_id)?).await
}

#[tauri::command]
pub async fn get_call_participants(
    state: State<'_, AppState>,
    room_id: String,
) -> Result<rtc::CallParticipants> {
    rtc::participants(&*state.core().await?, &parse_room_id(&room_id)?).await
}

#[tauri::command]
pub async fn get_active_calls(state: State<'_, AppState>) -> Result<Vec<String>> {
    Ok(rtc::active_calls(&*state.core().await?)
        .await?
        .into_iter()
        .map(|id| id.to_string())
        .collect())
}
