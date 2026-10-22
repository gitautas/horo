/**
 * Typed wrappers over the Tauri command surface.
 *
 * Every backend call goes through here so components never touch `invoke`
 * directly and never have to remember an argument name.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  CallCredentials,
  CallParticipants,
  DeviceInfo,
  DirectoryPage,
  Profile,
  ProfileUpdate,
  HomeserverInfo,
  ImagePack,
  NewRoom,
  NewRoomResult,
  NotifyMode,
  OwnPresence,
  PackEdit,
  PackRoom,
  PackTarget,
  Presence,
  PresenceUpdate,
  PublicRoom,
  RecentPhotos,
  RecoveryStatus,
  RegisterAuth,
  RegisterEmailToken,
  RegisterStep,
  RoomMember,
  RoomPermissions,
  RoomsSnapshot,
  RoomsUpdate,
  SasStateInfo,
  SendOptions,
  SessionInfo,
  SpaceSummary,
  StickerOptions,
  SyncStatus,
  TimelineItem,
  TimelineUpdate,
  TypingUpdate,
  LatestRelease,
  UpdateMode,
  UserContext,
  UwuError,
  WebrtcDiagnosis,
  VerificationRequestInfo,
} from "./types";

/** Rust errors arrive as `{ kind, message }`; anything else is a real surprise. */
export function asUwuError(error: unknown): UwuError {
  if (
    typeof error === "object" &&
    error !== null &&
    "kind" in error &&
    "message" in error
  ) {
    return error as UwuError;
  }
  return { kind: "other", message: String(error) };
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

export const discoverHomeserver = (server: string) =>
  invoke<HomeserverInfo>("discover_homeserver", { server });

export const loginPassword = (server: string, username: string, password: string) =>
  invoke<SessionInfo>("login_password", { server, username, password });

export const loginSso = (server: string, providerId?: string) =>
  invoke<SessionInfo>("login_sso", { server, providerId: providerId ?? null });

/**
 * One step of creating an account. Start with no `session`/`auth`; answer each
 * `needsAuth` with the session it carried and the stage just completed.
 */
export const registerAccount = (args: {
  server: string;
  username: string;
  password: string;
  session?: string | null;
  auth?: RegisterAuth | null;
}) =>
  invoke<RegisterStep>("register_account", {
    ...args,
    session: args.session ?? null,
    auth: args.auth ?? null,
  });

/**
 * Have the server email a sign-up verification link. Pass the previous
 * `clientSecret` with a higher `sendAttempt` to re-send to the same address.
 */
export const requestRegisterEmail = (args: {
  server: string;
  email: string;
  clientSecret?: string | null;
  sendAttempt: number;
}) =>
  invoke<RegisterEmailToken>("request_register_email", {
    ...args,
    clientSecret: args.clientSecret ?? null,
  });

export const restoreSession = () => invoke<SessionInfo | null>("restore_session");

export const currentSession = () => invoke<SessionInfo | null>("current_session");

export const logout = (wipe = false) => invoke<void>("logout", { wipe });

// ---------------------------------------------------------------------------
// rooms
// ---------------------------------------------------------------------------

/**
 * A snapshot of the room list.
 *
 * Sync starts when the session is restored, which is before the UI mounts, so
 * the backend's first push has no listener. Call this once after subscribing;
 * diffs keep it current from then on.
 */
export const getRooms = () => invoke<RoomsSnapshot>("get_rooms");

export const getSpaces = () => invoke<SpaceSummary[]>("get_spaces");

export const getMembers = (roomId: string) =>
  invoke<RoomMember[]>("get_members", { roomId });

/** Make a room. Returns its ID, plus a warning if filing it under a space failed. */
export const createRoom = (room: NewRoom) =>
  invoke<NewRoomResult>("create_room", { room });

/**
 * Join by alias or ID. `via` names servers to ask — needed for a room ID found
 * on another server, which ours has no other way to locate.
 */
export const joinRoom = (aliasOrId: string, via: string[] = []) =>
  invoke<string>("join_room", { aliasOrId, via });

/** One page of public spaces from `server`'s directory, ours when omitted. */
export const getPublicSpaces = (opts: { server?: string; search?: string; since?: string }) =>
  invoke<DirectoryPage>("get_public_spaces", {
    server: opts.server ?? null,
    search: opts.search ?? null,
    since: opts.since ?? null,
  });

/** The rooms directly inside a space, readable without joining it. */
export const getSpaceRooms = (spaceId: string) =>
  invoke<PublicRoom[]>("get_space_rooms", { spaceId });

/**
 * Leave a room. It stops being listed, but nothing local is deleted — this
 * isn't "forget", which PLAN.md explains at length is not safe to offer.
 */
export const leaveRoom = (roomId: string) => invoke<void>("leave_room", { roomId });

/** Rename a room or change its topic. Omitted fields are left alone. */
export const updateRoom = (roomId: string, patch: { name?: string; topic?: string }) =>
  invoke<void>("update_room", {
    roomId,
    name: patch.name ?? null,
    topic: patch.topic ?? null,
  });

export const getRoomPermissions = (roomId: string) =>
  invoke<RoomPermissions>("get_room_permissions", { roomId });

export const inviteUser = (roomId: string, userId: string) =>
  invoke<void>("invite_user", { roomId, userId });

export const setTyping = (roomId: string, typing: boolean) =>
  invoke<void>("set_typing", { roomId, typing });


export const setRoomMuted = (roomId: string, muted: boolean) =>
  invoke<void>("set_room_muted", { roomId, muted });

/**
 * Set the notification level on several rooms — a whole space's worth. `null`
 * returns them to the default, which is how a mute is undone.
 */
export const setRoomsNotifyMode = (roomIds: string[], mode: NotifyMode | null) =>
  invoke<void>("set_rooms_notify_mode", { roomIds, mode });

/** Mark several rooms read without opening them. */
export const markRoomsRead = (roomIds: string[]) => invoke<void>("mark_rooms_read", { roomIds });

export const setRoomMarkedUnread = (roomId: string, unread: boolean) =>
  invoke<void>("set_room_marked_unread", { roomId, unread });

// ---------------------------------------------------------------------------
// timeline
// ---------------------------------------------------------------------------

export const openTimeline = (roomId: string, threadRoot?: string) =>
  invoke<TimelineItem[]>("open_timeline", { roomId, threadRoot: threadRoot ?? null });

export const closeTimeline = (roomId: string, threadRoot?: string) =>
  invoke<void>("close_timeline", { roomId, threadRoot: threadRoot ?? null });

export const sendMessage = (roomId: string, options: SendOptions) =>
  invoke<void>("send_message", {
    roomId,
    options: {
      body: options.body,
      markdown: options.markdown ?? false,
      replyTo: options.replyTo ?? null,
      threadRoot: options.threadRoot ?? null,
      msgtype: options.msgtype ?? null,
      emotes: options.emotes ?? [],
    },
  });

export const sendSticker = (
  roomId: string,
  sticker: StickerOptions,
  threadRoot?: string,
) =>
  invoke<void>("send_sticker", {
    roomId,
    threadRoot: threadRoot ?? null,
    sticker: {
      body: sticker.body,
      url: sticker.url,
      width: sticker.width ?? null,
      height: sticker.height ?? null,
      size: sticker.size ?? null,
      mimetype: sticker.mimetype ?? null,
    },
  });

/**
 * Custom emote and sticker packs usable right now.
 *
 * `roomId` is the room being looked at: its own packs are available there
 * whether or not they've been enabled everywhere.
 */
export const getImagePacks = (roomId?: string) =>
  invoke<ImagePack[]>("get_image_packs", { roomId: roomId ?? null });

/** Every pack on the account, including ones not turned on. */
export const getAllImagePacks = () => invoke<ImagePack[]>("get_all_image_packs");

/** Rooms this account may put a shared pack in. */
export const getPackRooms = () => invoke<PackRoom[]>("get_pack_rooms");

export const editImagePack = (target: PackTarget, edit: PackEdit) =>
  invoke<void>("edit_image_pack", {
    target: { roomId: target.roomId ?? null, stateKey: target.stateKey ?? null },
    edit,
  });

export const setPackEverywhere = (roomId: string, stateKey: string, everywhere: boolean) =>
  invoke<void>("set_pack_everywhere", { roomId, stateKey, everywhere });

/**
 * Make a pack of your own.
 *
 * Packs live in rooms, so this makes a private one to hold it and turns the
 * pack on everywhere. Resolves to the room's ID, which the UI has no reason to
 * show anyone.
 */
export const createPersonalPack = (name: string) =>
  invoke<string>("create_personal_pack", { name });

/** Resolves to `true` once there's nothing older left to load. */
export const paginateBack = (roomId: string, threadRoot?: string) =>
  invoke<boolean>("paginate_back", { roomId, threadRoot: threadRoot ?? null });

export const toggleReaction = (
  roomId: string,
  eventId: string,
  key: string,
  threadRoot?: string,
) =>
  invoke<boolean>("toggle_reaction", {
    roomId,
    eventId,
    key,
    threadRoot: threadRoot ?? null,
  });

export const editMessage = (
  roomId: string,
  eventId: string,
  body: string,
  markdown = false,
  threadRoot?: string,
) =>
  invoke<void>("edit_message", {
    roomId,
    eventId,
    body,
    markdown,
    threadRoot: threadRoot ?? null,
  });

export const redactEvent = (
  roomId: string,
  eventId: string,
  reason?: string,
  threadRoot?: string,
) =>
  invoke<void>("redact_event", {
    roomId,
    eventId,
    reason: reason ?? null,
    threadRoot: threadRoot ?? null,
  });

export const markRoomRead = (roomId: string) => invoke<void>("mark_room_read", { roomId });

export const sendReadReceipt = (roomId: string, eventId: string) =>
  invoke<void>("send_read_receipt", { roomId, eventId });

export const sendAttachment = (
  roomId: string,
  path: string,
  caption?: string,
  threadRoot?: string,
) =>
  invoke<void>("send_attachment", {
    roomId,
    path,
    caption: caption ?? null,
    threadRoot: threadRoot ?? null,
  });

/**
 * Send a file the WebView holds as bytes — a pasted screenshot, an image
 * dragged out of another app.
 *
 * The bytes go in the request body rather than the arguments: as JSON they'd
 * serialise to a numeric array several times the size of the file. Everything
 * else rides in headers, which have to be ASCII, so text is percent-encoded.
 */
export const sendAttachmentBytes = (
  roomId: string,
  file: { name: string; type: string; bytes: ArrayBuffer },
  threadRoot?: string,
  caption?: string,
) =>
  invoke<void>("send_attachment_bytes", file.bytes, {
    headers: {
      "x-room-id": roomId,
      "x-filename": encodeURIComponent(file.name),
      "x-mime": file.type,
      "x-thread-root": threadRoot ?? "",
      "x-caption": caption ? encodeURIComponent(caption) : "",
    },
  });

export const getPinnedEvents = (roomId: string) =>
  invoke<TimelineItem[]>("get_pinned_events", { roomId });

export const getEventBody = (roomId: string, eventId: string) =>
  invoke<string | null>("get_event_body", { roomId, eventId });

// ---------------------------------------------------------------------------
// presence
// ---------------------------------------------------------------------------

/**
 * Declare who the UI is drawing. Replaces the previous set.
 *
 * Presence is polled per user (see `presence.rs`), so this is the difference
 * between a handful of requests a minute and one per person you've ever met.
 * `lib/presence.ts` owns the set; nothing else should call this.
 */
export const watchPresence = (userIds: string[]) =>
  invoke<void>("watch_presence", { userIds });

/** Everything the backend already knows, for a freshly-mounted UI. */
export const getPresence = () => invoke<Presence[]>("get_presence");

/** Publish our own availability — the idle timer drives this. */
export const setOwnPresence = (presence: OwnPresence) =>
  invoke<void>("set_own_presence", { presence });

// ---------------------------------------------------------------------------
// media
// ---------------------------------------------------------------------------

export const saveMedia = (mxc: string, destination: string) =>
  invoke<void>("save_media", { mxc, destination });

/**
 * Turn an `mxc://` URI into something an `<img>` can load.
 *
 * The bytes are served by the Rust `horo://` protocol handler, which fetches
 * through the SDK's media cache and lets the WebView do the rest.
 */
export function mediaUrl(
  mxc: string | null | undefined,
  size?: { width: number; height: number },
): string | undefined {
  if (!mxc || !mxc.startsWith("mxc://")) return undefined;

  // WebView2 serves Tauri custom protocols as http://<scheme>.localhost/..., not
  // <scheme>://. The Rust handler (media.rs) and the CSP already accept both forms.
  const base = navigator.userAgent.includes("Windows")
    ? `http://horo.localhost/media/${encodeURIComponent(mxc)}`
    : `horo://media/${encodeURIComponent(mxc)}`;
  if (!size) return base;

  const { width, height } = thumbnailSize(size);
  return `${base}?w=${width}&h=${height}`;
}

/**
 * The thumbnail actually requested for an image shown at `size`.
 *
 * Exported so anything else fetching the same picture asks for the same bytes
 * and shares the cache entry.
 */
export function thumbnailSize(size: { width: number; height: number }): {
  width: number;
  height: number;
} {
  // Ask for 2× so the image stays sharp on retina displays.
  const scale = Math.min(window.devicePixelRatio || 1, 2);
  return { width: snapUp(size.width * scale), height: snapUp(size.height * scale) };
}

/** The only thumbnail sizes we ever ask a homeserver for. */
const THUMBNAIL_STEPS = [32, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048];

/**
 * Round a requested dimension up to the next standard step.
 *
 * The size is part of the URL, so it's also the cache key — in the WebView, in
 * the SDK's media store, and on the homeserver. Asking for 68px in one place
 * and 76px in another means the same avatar is fetched twice and cached twice,
 * and for *remote* media each miss is a federation round trip that can take
 * seconds or fail outright. A dozen buckets is plenty of sharpness and turns
 * every avatar in the app into one shared fetch.
 */
function snapUp(value: number): number {
  return (
    THUMBNAIL_STEPS.find((step) => step >= value) ??
    THUMBNAIL_STEPS[THUMBNAIL_STEPS.length - 1]
  );
}

// ---------------------------------------------------------------------------
// verification
// ---------------------------------------------------------------------------

export const requestVerification = (userId?: string) =>
  invoke<void>("request_verification", { userId: userId ?? null });

export const acceptVerification = (userId: string, flowId: string) =>
  invoke<void>("accept_verification", { userId, flowId });

export const confirmVerification = (userId: string, flowId: string) =>
  invoke<void>("confirm_verification", { userId, flowId });

export const mismatchVerification = (userId: string, flowId: string) =>
  invoke<void>("mismatch_verification", { userId, flowId });

export const cancelVerification = (userId: string, flowId: string) =>
  invoke<void>("cancel_verification", { userId, flowId });

export const getProfile = (userId?: string) =>
  invoke<Profile>("get_profile", { userId: userId ?? null });

export const setProfile = (update: ProfileUpdate) =>
  invoke<void>("set_profile", { update });

/** Verification, shared rooms and any existing DM — our side of a profile. */
export const getUserContext = (userId: string) =>
  invoke<UserContext>("get_user_context", { userId });

/** The DM with this person, created if there isn't one yet. */
export const openDm = (userId: string) => invoke<string>("open_dm", { userId });

/** Upload a local file to the media repo, returning its `mxc://` URI. */
export const uploadMedia = (path: string) => invoke<string>("upload_media", { path });

export const getOwnDevices = () => invoke<DeviceInfo[]>("get_own_devices");

export const verifyDevice = (deviceId: string) =>
  invoke<void>("verify_device", { deviceId });

export const getRecoveryStatus = () => invoke<RecoveryStatus>("get_recovery_status");

export const enableRecovery = () => invoke<string>("enable_recovery");

export const recoverWithKey = (recoveryKey: string) =>
  invoke<void>("recover_with_key", { recoveryKey });

// ---------------------------------------------------------------------------
// calls
// ---------------------------------------------------------------------------

export const joinCall = (roomId: string, video = false) =>
  invoke<CallCredentials>("join_call", { roomId, video });

export const leaveCall = (roomId: string) => invoke<void>("leave_call", { roomId });

export const refreshCallMembership = (roomId: string) =>
  invoke<void>("refresh_call_membership", { roomId });

export const getCallParticipants = (roomId: string) =>
  invoke<CallParticipants>("get_call_participants", { roomId });

export const getActiveCalls = () => invoke<string[]>("get_active_calls");

// ---------------------------------------------------------------------------
// pushed events
// ---------------------------------------------------------------------------

/** The "settings…" menu item — on macOS that's where `cmd+,` arrives. */
export const onOpenSettings = (fn: () => void): Promise<UnlistenFn> =>
  listen("horo://open-settings", () => fn());

export type ZoomDirection = "in" | "out" | "reset";

/** The View menu's zoom items — on macOS that's where `cmd+=`/`cmd+-`/`cmd+0` arrive. */
export const onZoom = (fn: (direction: ZoomDirection) => void): Promise<UnlistenFn> =>
  listen<ZoomDirection>("horo://zoom", (e) => fn(e.payload));

export const onRooms = (fn: (update: RoomsUpdate) => void): Promise<UnlistenFn> =>
  listen<RoomsUpdate>("matrix://rooms", (e) => fn(e.payload));

export const onTimeline = (fn: (update: TimelineUpdate) => void): Promise<UnlistenFn> =>
  listen<TimelineUpdate>("matrix://timeline", (e) => fn(e.payload));

export const onTyping = (fn: (update: TypingUpdate) => void): Promise<UnlistenFn> =>
  listen<TypingUpdate>("matrix://typing", (e) => fn(e.payload));

export const onPresence = (fn: (update: PresenceUpdate) => void): Promise<UnlistenFn> =>
  listen<PresenceUpdate>("matrix://presence", (e) => fn(e.payload));

export const onSyncStatus = (fn: (status: SyncStatus) => void): Promise<UnlistenFn> =>
  listen<SyncStatus>("matrix://sync-status", (e) => fn(e.payload));

export const onVerificationState = (
  fn: (payload: { state: string }) => void,
): Promise<UnlistenFn> =>
  listen<{ state: string }>("matrix://verification-state", (e) => fn(e.payload));

export const onVerificationRequest = (
  fn: (request: VerificationRequestInfo) => void,
): Promise<UnlistenFn> =>
  listen<VerificationRequestInfo>("matrix://verification-request", (e) => fn(e.payload));

export const onVerificationUpdate = (
  fn: (update: VerificationRequestInfo | SasStateInfo) => void,
): Promise<UnlistenFn> =>
  listen<VerificationRequestInfo | SasStateInfo>("matrix://verification-update", (e) =>
    fn(e.payload),
  );

/**
 * The newest photos and videos on the device, with thumbnails.
 *
 * iOS only — everywhere else this answers `supported: false` rather than
 * failing, so the caller can fall back to the file picker without a try/catch
 * around ordinary platform differences.
 */
export const photosRecent = (limit: number) =>
  invoke<RecentPhotos>("photos_recent", { limit });

/**
 * Copy one library item to a temp file and return its path.
 *
 * A path, not bytes: it rejoins the same upload route the file picker and
 * drag-and-drop already use.
 */
export const photosExport = (id: string) => invoke<string>("photos_export", { id });

// ---------------------------------------------------------------------------
// updates
// ---------------------------------------------------------------------------

/** Whether this build can install its own updates. See `update.rs`. */
export const updateMode = () => invoke<UpdateMode>("update_mode");

/** The newest published release, for builds that have to be updated by hand. */
export const latestRelease = () => invoke<LatestRelease>("latest_release");

/** Is `latest` newer than the version this binary was built as? */
export const updateAvailable = (latest: string) =>
  invoke<boolean>("update_available", { latest });

/** What the native side knows about WebRTC here; `null` where it always works. */
export const webrtcDiagnosis = () => invoke<WebrtcDiagnosis | null>("webrtc_diagnosis");
