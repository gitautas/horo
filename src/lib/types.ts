/**
 * Mirrors `src-tauri/src/dto.rs`. Keep the two in step — the Rust side is the
 * source of truth for shapes, this is the source of truth for how we use them.
 */

export interface UwuError {
  kind:
    | "not_signed_in"
    | "auth"
    | "unknown_room"
    | "no_timeline"
    | "matrix"
    | "bad_id"
    | "client_build"
    | "timeline"
    | "room_list"
    | "io"
    | "json"
    | "http"
    | "other";
  message: string;
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

export interface SsoProvider {
  id: string;
  name: string;
  icon: string | null;
}

export interface HomeserverInfo {
  serverName: string;
  homeserverUrl: string;
  supportsPassword: boolean;
  supportsSso: boolean;
  ssoProviders: SsoProvider[];
  livekitServiceUrl: string | null;
}

export interface SessionInfo {
  userId: string;
  deviceId: string;
  homeserver: string;
  displayName: string | null;
  avatarUrl: string | null;
  insecureStorage: boolean;
}

// ---------------------------------------------------------------------------
// rooms
// ---------------------------------------------------------------------------

export interface LatestEvent {
  sender: string;
  senderName: string | null;
  body: string;
  timestamp: number;
}

export interface RoomSummary {
  id: string;
  name: string;
  topic: string | null;
  canonicalAlias: string | null;
  avatarUrl: string | null;
  isDirect: boolean;
  /**
   * The other person, when a DM has exactly one of them — what the sidebar's
   * presence dot hangs off. Null for group rooms.
   */
  dmUserId: string | null;
  isEncrypted: boolean;
  isSpace: boolean;
  /** A room that holds data rather than a conversation — an image pack, say. */
  isUtility: boolean;
  isMuted: boolean;
  /** The level set for this room, if one has been; null is the account default. */
  notifyMode: "all" | "mentions" | "mute" | null;
  membership: "joined" | "invited" | "left" | "knocked" | "banned";
  notificationCount: number;
  highlightCount: number;
  hasUnread: boolean;
  isMarkedUnread: boolean;
  memberCount: number;
  latest: LatestEvent | null;
  recency: number;
  parentSpaces: string[];
  hasActiveCall: boolean;
  /** An MSC3417 video room — a call you join, not a text room. */
  isVideoRoom: boolean;
}

/** What the create-room dialog collects. */
export interface NewRoom {
  name: string;
  topic?: string;
  /** Anyone can find and join it, and it's listed in the directory. */
  isPublic?: boolean;
  /** Alias localpart for a public room — `movies`, not `#movies:server`. */
  alias?: string;
  encrypted?: boolean;
  invite?: string[];
  /** The space to file it under, if one was open. */
  parentSpace?: string | null;
}

export interface NewRoomResult {
  roomId: string;
  /** The room was made but couldn't be filed under the space. Not an error. */
  spaceWarning: string | null;
}

/** What this account may change about a room. */
export interface RoomPermissions {
  canRename: boolean;
  canSetTopic: boolean;
  canInvite: boolean;
}

export interface SpaceSummary {
  id: string;
  name: string;
  avatarUrl: string | null;
  children: string[];
  notificationCount: number;
  highlightCount: number;
}

export interface RoomMember {
  userId: string;
  displayName: string | null;
  avatarUrl: string | null;
  powerLevel: number;
  membership: string;
  isIgnored: boolean;
  verification: "verified" | "unverified" | "unknown";
}

// ---------------------------------------------------------------------------
// timeline
// ---------------------------------------------------------------------------

export interface MediaInfo {
  mxc: string | null;
  mimetype: string | null;
  size: number | null;
  width: number | null;
  height: number | null;
  durationMs: number | null;
  blurhash: string | null;
  thumbnailMxc: string | null;
  waveform: number[] | null;
  isVoice: boolean;
}

export type Content =
  | { kind: "text"; body: string; formatted: string | null }
  | { kind: "emote"; body: string; formatted: string | null }
  | { kind: "notice"; body: string; formatted: string | null }
  | { kind: "image"; body: string; media: MediaInfo }
  | { kind: "video"; body: string; media: MediaInfo }
  | { kind: "audio"; body: string; media: MediaInfo }
  | { kind: "file"; body: string; media: MediaInfo }
  | { kind: "location"; body: string; geoUri: string }
  | { kind: "sticker"; body: string; media: MediaInfo }
  | { kind: "poll"; question: string; answers: string[]; ended: boolean }
  | { kind: "redacted" }
  | { kind: "unableToDecrypt"; reason: string }
  | { kind: "membership"; change: string; userId: string; displayName: string | null }
  | { kind: "profileChange"; summary: string }
  | { kind: "state"; eventType: string; stateKey: string; summary: string }
  | { kind: "callInvite" }
  | { kind: "rtcNotification"; intent: string | null }
  | { kind: "unsupported"; eventType: string };

export type SendState =
  | { status: "notSentYet"; progress: number | null }
  | { status: "failed"; error: string; isRecoverable: boolean }
  | { status: "sent"; eventId: string };

export interface Shield {
  colour: "red" | "grey";
  message: string;
}

export interface Reaction {
  key: string;
  count: number;
  mine: boolean;
  pending: boolean;
  senders: string[];
}

export interface ReplyInfo {
  eventId: string;
  sender: string | null;
  senderName: string | null;
  body: string | null;
}

export interface ThreadSummary {
  numReplies: number;
  latestSender: string | null;
  latestBody: string | null;
}

export interface EventItem {
  eventId: string | null;
  transactionId: string | null;
  sender: string;
  senderName: string | null;
  senderAvatar: string | null;
  timestamp: number;
  isOwn: boolean;
  isEditable: boolean;
  canReply: boolean;
  isHighlighted: boolean;
  isEdited: boolean;
  isEmojiOnly: boolean;
  sendState: SendState | null;
  shield: Shield | null;
  content: Content;
  reactions: Reaction[];
  reply: ReplyInfo | null;
  threadRoot: string | null;
  threadSummary: ThreadSummary | null;
  readReceipts: string[];
}

export interface TimelineItem {
  id: string;
  kind: "event" | "dateDivider" | "readMarker" | "timelineStart";
  timestamp?: number;
  event?: EventItem;
}

// ---------------------------------------------------------------------------
// diffs & pushed events
// ---------------------------------------------------------------------------

export type Diff<T> =
  | { op: "append"; values: T[] }
  | { op: "clear" }
  | { op: "pushFront"; value: T }
  | { op: "pushBack"; value: T }
  | { op: "popFront" }
  | { op: "popBack" }
  | { op: "insert"; index: number; value: T }
  | { op: "set"; index: number; value: T }
  | { op: "remove"; index: number }
  | { op: "truncate"; length: number }
  | { op: "reset"; values: T[] };

/** A numbered batch of room-list changes. */
export interface RoomsUpdate {
  seq: number;
  diffs: Diff<RoomSummary>[];
}

/** The room list as it stands, and the last batch folded into it. */
export interface RoomsSnapshot {
  seq: number;
  rooms: RoomSummary[];
}

export interface TimelineUpdate {
  /** Room ID, or `<roomId>|<threadRoot>` for a thread timeline. */
  roomId: string;
  diffs: Diff<TimelineItem>[];
}

export interface TypingUser {
  userId: string;
  displayName: string | null;
}

export interface TypingUpdate {
  roomId: string;
  users: TypingUser[];
}

// ---------------------------------------------------------------------------
// presence
// ---------------------------------------------------------------------------

/**
 * What the server says about someone's availability.
 * The shapes that cross the bridge.
 *
 * Generated from the Rust wire types into `./bindings` by `npm run bindings`,
 * so there is nothing here to keep in step by hand: change the Rust type,
 * regenerate, and the compiler finds every use that no longer fits. A new type
 * the frontend needs gets `#[derive(TS)]` in Rust rather than a copy here.
 *
 * What does live here is how the frontend reads them.
 */

import type { SasStateInfo, VerificationRequestInfo } from "./bindings";

export type * from "./bindings";

/** The two verification payloads share an event channel; this tells them apart. */
export function isSasUpdate(
  update: VerificationRequestInfo | SasStateInfo,
): update is SasStateInfo {
  return "emoji" in update;
}

/**
 * Is this verification still running?
 *
 * A finished flow stays in the store on purpose, so the user can read the
 * outcome before dismissing it. That makes "there is a request" a useless test
 * for "a verification is in progress" — this is the one that means it.
 */
export function isLiveVerification(
  request: VerificationRequestInfo | null,
): request is VerificationRequestInfo {
  return !!request && request.state !== "done" && request.state !== "cancelled";
}
