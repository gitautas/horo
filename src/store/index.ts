/**
 * The app's single store.
 *
 * Rust owns the truth; this holds a projection of it plus the bits of UI state
 * that don't belong on the server (which room is selected, what's in the
 * composer). Backend pushes arrive as diffs and are applied in place.
 */

import { create } from "zustand";

import { applyDiffs } from "../lib/diff";
import * as ipc from "../lib/ipc";
import * as prefs from "../lib/settings";
import type {
  ImagePack,
  Presence,
  PresenceUpdate,
  RoomsSnapshot,
  RoomsUpdate,
  RoomSummary,
  SasStateInfo,
  SessionInfo,
  SpaceSummary,
  SyncStatus,
  TimelineItem,
  TimelineUpdate,
  TypingUpdate,
  TypingUser,
  VerificationRequestInfo,
} from "../lib/types";
import { isLiveVerification, isSasUpdate } from "../lib/types";

export type RoomFilter = "all" | "unread" | "dms" | "muted";

/** Which timeline a view is showing — a room, or a thread inside one. */
export interface TimelineKey {
  roomId: string;
  threadRoot?: string;
}

export function timelineKey({ roomId, threadRoot }: TimelineKey): string {
  return threadRoot ? `${roomId}|${threadRoot}` : roomId;
}

interface Draft {
  body: string;
  replyTo: string | null;
  editing: string | null;
}

const EMPTY_DRAFT: Draft = { body: "", replyTo: null, editing: null };

interface State {
  // session
  session: SessionInfo | null;
  bootstrapped: boolean;
  syncStatus: SyncStatus;

  // rooms
  rooms: RoomSummary[];
  /**
   * The last room-list batch folded into `rooms`.
   *
   * The snapshot and the diff stream arrive over different channels and can
   * cross in flight, so both carry this number: a batch at or below it is
   * already in the list, and a batch more than one ahead means one went
   * missing and the list can no longer be trusted to index correctly.
   */
  roomsSeq: number;
  /** A resync is in flight; diffs are unreliable until it lands. */
  roomsResyncing: boolean;
  spaces: SpaceSummary[];
  activeSpaceId: string | null;
  activeRoomId: string | null;
  activeThreadRoot: string | null;
  filter: RoomFilter;
  search: string;

  // timelines, keyed by `timelineKey`
  timelines: Record<string, TimelineItem[]>;
  /** Timelines that have reached the start of history. */
  exhausted: Record<string, boolean>;
  paginating: Record<string, boolean>;
  typing: Record<string, TypingUser[]>;
  drafts: Record<string, Draft>;

  /**
   * Who's online, keyed by user ID — only the people something on screen has
   * asked about (`lib/presence.ts` owns that set).
   *
   * Entries are replaced individually so a person's object keeps its identity
   * while they haven't changed: every avatar reads its own key as a selector,
   * and a fresh object per poll would re-render all of them every 30 seconds.
   */
  presence: Record<string, Presence>;
  /**
   * False once the homeserver has refused presence outright. The indicators
   * disappear rather than claiming everyone is offline, which is what a server
   * with the feature disabled would otherwise look like.
   */
  presenceSupported: boolean;

  // panels
  showInfo: boolean;
  showSettings: boolean;
  /**
   * The open profile card, if any. Anchored to whatever avatar was clicked —
   * one card at a time, owned here rather than by each avatar, so opening a
   * second one closes the first without either knowing about the other.
   */
  profileCard: { userId: string; anchor: DOMRect } | null;
  /** The picture being looked at full-size, if any. */
  lightbox: { mxc: string; name: string } | null;
  showCreateRoom: boolean;

  /**
   * Custom emote and sticker packs usable in the room that's open.
   *
   * Loaded per room rather than once, because a room's own packs count there
   * whether or not they've been enabled everywhere.
   */
  packs: ImagePack[];

  /** Machine-local preferences, persisted outside the account. */
  settings: prefs.Settings;

  // verification
  verificationRequest: VerificationRequestInfo | null;
  sasState: SasStateInfo | null;

  // calls
  /**
   * A DM whose call is ringing at us right now, and when it started ringing.
   * Group calls never land here — they get a banner and a single chirp, the
   * way they always have; a room of forty people all ringing is not a feature.
   */
  incomingCall: { roomId: string; since: number } | null;

  // toasts / errors
  banner: { tone: "error" | "info"; message: string } | null;
}

interface Actions {
  setSession(session: SessionInfo | null): void;
  setRooms(snapshot: RoomsSnapshot): void;
  setBootstrapped(v: boolean): void;
  applyRoomDiffs(update: RoomsUpdate): void;
  setSpaces(spaces: SpaceSummary[]): void;
  /** Re-read the whole room list after a dropped batch. */
  resyncRooms(): Promise<void>;
  /** Re-read the spaces now rather than waiting for the slow poll. */
  refreshSpaces(): Promise<void>;
  setActiveSpace(id: string | null): void;
  selectRoom(roomId: string | null): Promise<void>;
  openThread(threadRoot: string | null): Promise<void>;
  setFilter(filter: RoomFilter): void;
  setSearch(search: string): void;
  toggleInfo(): void;
  /** Close the info panel without touching the saved layout preference. */
  closeInfo(): void;
  openSettings(): void;
  closeSettings(): void;
  /** Clicking the same avatar again closes the card, like every other popover. */
  toggleProfile(userId: string, anchor: DOMRect): void;
  closeProfile(): void;
  openLightbox(mxc: string, name: string): void;
  closeLightbox(): void;
  openCreateRoom(): void;
  closeCreateRoom(): void;
  updateSettings(patch: Partial<prefs.Settings>): void;
  /** Move a reaction to the front of the recents row. */
  noteReactionUsed(key: string): void;
  /** Re-read the packs available in a room. */
  loadPacks(roomId: string | null): Promise<void>;

  applyTimelineUpdate(update: TimelineUpdate): void;
  setTimeline(key: string, items: TimelineItem[]): void;
  loadOlder(): Promise<void>;
  applyTyping(update: TypingUpdate): void;
  applyPresence(update: PresenceUpdate): void;
  setPresenceSnapshot(users: Presence[]): void;

  setDraft(key: string, patch: Partial<Draft>): void;
  clearDraft(key: string): void;

  setSyncStatus(status: SyncStatus): void;
  setVerificationRequest(request: VerificationRequestInfo | null): void;
  applyVerificationUpdate(update: VerificationRequestInfo | SasStateInfo): void;
  /** Start ringing for a DM call. Ignored if something is already ringing. */
  ringIncomingCall(roomId: string): void;
  /** Stop ringing — for one room, or unconditionally when `roomId` is omitted. */
  stopIncomingCall(roomId?: string): void;
  showBanner(tone: "error" | "info", message: string): void;
  dismissBanner(): void;
  reset(): void;
}

const initial: State = {
  session: null,
  bootstrapped: false,
  syncStatus: { state: "idle", message: null },
  rooms: [],
  roomsSeq: 0,
  roomsResyncing: false,
  spaces: [],
  activeSpaceId: null,
  activeRoomId: null,
  activeThreadRoot: null,
  filter: "all",
  search: "",
  timelines: {},
  exhausted: {},
  paginating: {},
  typing: {},
  drafts: {},
  presence: {},
  presenceSupported: true,
  showInfo: prefs.load().showInfoPanel,
  showSettings: false,
  profileCard: null,
  lightbox: null,
  showCreateRoom: false,
  packs: [],
  settings: prefs.load(),
  verificationRequest: null,
  sasState: null,
  incomingCall: null,
  banner: null,
};

export const useStore = create<State & Actions>((set, get) => ({
  ...initial,

  setSession: (session) => set({ session }),

  setRooms: (snapshot) =>
    set((s) =>
      // A snapshot older than what we've already applied is stale news; it
      // would undo batches that arrived while it was being fetched.
      snapshot.seq >= s.roomsSeq
        ? { rooms: snapshot.rooms, roomsSeq: snapshot.seq, roomsResyncing: false }
        : { roomsResyncing: false },
    ),
  setBootstrapped: (bootstrapped) => set({ bootstrapped }),

  applyRoomDiffs: ({ seq, diffs }) => {
    const { roomsSeq, roomsResyncing } = get();

    // Already in the list: this batch was folded in before the snapshot we
    // hold was taken.
    if (seq <= roomsSeq) return;

    // A gap means a batch went missing, so every index in this one counts
    // against a list we don't have. Applying it would silently misplace rooms;
    // ask for the truth instead.
    if (seq > roomsSeq + 1) {
      if (!roomsResyncing) {
        set({ roomsResyncing: true });
        void get().resyncRooms();
      }
      return;
    }

    if (roomsResyncing) return;
    set((s) => ({ rooms: applyDiffs(s.rooms, diffs), roomsSeq: seq }));
  },

  resyncRooms: async () => {
    try {
      get().setRooms(await ipc.getRooms());
    } catch {
      // Leave the flag set: the next batch will try again.
      set({ roomsResyncing: false });
    }
  },

  setSpaces: (spaces) => set({ spaces }),

  refreshSpaces: async () => {
    try {
      set({ spaces: await ipc.getSpaces() });
    } catch {
      // The poll will come round again; nothing here is worth a banner.
    }
  },

  setActiveSpace: (activeSpaceId) => set({ activeSpaceId }),

  selectRoom: async (roomId) => {
    const previous = get().activeRoomId;
    const previousThread = get().activeThreadRoot;
    if (previous === roomId) return;

    // Close the old timeline so its stream task stops; leaving them all open
    // would keep every visited room streaming for the whole session.
    if (previous) {
      if (previousThread) {
        void ipc.closeTimeline(previous, previousThread).catch(() => {});
      }
      void ipc.closeTimeline(previous).catch(() => {});
    }

    set({ activeRoomId: roomId, activeThreadRoot: null, packs: [] });
    if (!roomId) return;

    // Packs are wanted before the picker opens, not when it does — opening it
    // should never be the thing that waits on a round trip.
    void get().loadPacks(roomId);

    try {
      const items = await ipc.openTimeline(roomId);
      set((s) => ({ timelines: { ...s.timelines, [roomId]: items } }));
      // Opening a room is the user reading it.
      void ipc.markRoomRead(roomId).catch(() => {});
    } catch (error) {
      get().showBanner("error", ipc.asUwuError(error).message);
    }
  },

  openThread: async (threadRoot) => {
    const roomId = get().activeRoomId;
    const previous = get().activeThreadRoot;
    if (!roomId) return;

    if (previous && previous !== threadRoot) {
      void ipc.closeTimeline(roomId, previous).catch(() => {});
    }

    set({ activeThreadRoot: threadRoot });
    if (!threadRoot) return;

    try {
      const items = await ipc.openTimeline(roomId, threadRoot);
      const key = timelineKey({ roomId, threadRoot });
      set((s) => ({ timelines: { ...s.timelines, [key]: items } }));
    } catch (error) {
      get().showBanner("error", ipc.asUwuError(error).message);
    }
  },

  setFilter: (filter) => set({ filter }),
  setSearch: (search) => set({ search }),
  toggleInfo: () =>
    set((s) => {
      const showInfo = !s.showInfo;
      // Remember the panel state — it's a layout preference, not a per-room one.
      const settings = { ...s.settings, showInfoPanel: showInfo };
      prefs.save(settings);
      return { showInfo, settings };
    }),

  // Deliberately not persisted. On mobile the info panel is a *view* rather
  // than a panel, so leaving a room closes it — and that navigation should not
  // rewrite what the user chose for the desktop layout.
  closeInfo: () => set({ showInfo: false }),

  openSettings: () => set({ showSettings: true }),
  closeSettings: () => set({ showSettings: false }),

  toggleProfile: (userId, anchor) =>
    set((s) => ({
      profileCard: s.profileCard?.userId === userId ? null : { userId, anchor },
    })),
  closeProfile: () => set({ profileCard: null }),

  openLightbox: (mxc, name) => set({ lightbox: { mxc, name } }),
  closeLightbox: () => set({ lightbox: null }),

  openCreateRoom: () => set({ showCreateRoom: true }),
  closeCreateRoom: () => set({ showCreateRoom: false }),

  updateSettings: (patch) =>
    set((s) => {
      const settings = { ...s.settings, ...patch };
      prefs.save(settings);
      if (patch.theme) prefs.applyTheme(patch.theme);
      if (patch.accent) prefs.applyAccent(patch.accent);
      if (patch.fontFamily !== undefined || patch.messageFontSize !== undefined) {
        prefs.applyTypography(settings);
      }
      if (patch.zoom !== undefined) prefs.applyZoom(patch.zoom);
      return { settings };
    }),

  loadPacks: async (roomId) => {
    try {
      set({ packs: await ipc.getImagePacks(roomId ?? undefined) });
    } catch {
      // A picker with no custom packs is still a picker; nothing here is worth
      // interrupting the user over.
      set({ packs: [] });
    }
  },

  noteReactionUsed: (key) =>
    get().updateSettings({
      recentReactions: [
        key,
        ...get().settings.recentReactions.filter((k) => k !== key),
      ].slice(0, prefs.MAX_RECENT_REACTIONS),
    }),

  applyTimelineUpdate: ({ roomId: key, diffs }) =>
    set((s) => {
      // Ignore updates for timelines we've already closed.
      if (!(key in s.timelines)) return {};
      return { timelines: { ...s.timelines, [key]: applyDiffs(s.timelines[key], diffs) } };
    }),

  setTimeline: (key, items) =>
    set((s) => ({ timelines: { ...s.timelines, [key]: items } })),

  loadOlder: async () => {
    const { activeRoomId, activeThreadRoot } = get();
    if (!activeRoomId) return;

    const key = timelineKey({
      roomId: activeRoomId,
      threadRoot: activeThreadRoot ?? undefined,
    });
    // Nothing to paginate until `open_timeline` has come back.
    if (get().timelines[key] === undefined) return;
    if (get().paginating[key] || get().exhausted[key]) return;

    set((s) => ({ paginating: { ...s.paginating, [key]: true } }));
    try {
      const reachedStart = await ipc.paginateBack(
        activeRoomId,
        activeThreadRoot ?? undefined,
      );
      set((s) => ({ exhausted: { ...s.exhausted, [key]: reachedStart } }));
    } catch (error) {
      get().showBanner("error", ipc.asUwuError(error).message);
    } finally {
      set((s) => ({ paginating: { ...s.paginating, [key]: false } }));
    }
  },

  applyTyping: ({ roomId, users }) =>
    set((s) => ({ typing: { ...s.typing, [roomId]: users } })),

  applyPresence: ({ users, supported }) =>
    set((s) => {
      if (users.length === 0) return { presenceSupported: supported };
      const presence = { ...s.presence };
      for (const user of users) presence[user.userId] = user;
      return { presence, presenceSupported: supported };
    }),

  setPresenceSnapshot: (users) =>
    set((s) => {
      const presence = { ...s.presence };
      for (const user of users) presence[user.userId] = user;
      return { presence };
    }),

  setDraft: (key, patch) =>
    set((s) => ({
      drafts: { ...s.drafts, [key]: { ...(s.drafts[key] ?? EMPTY_DRAFT), ...patch } },
    })),

  clearDraft: (key) =>
    set((s) => ({ drafts: { ...s.drafts, [key]: { ...EMPTY_DRAFT } } })),

  setSyncStatus: (syncStatus) => set({ syncStatus }),

  setVerificationRequest: (verificationRequest) =>
    set((s) => {
      // A request arriving while another is still running does not replace it.
      // Abandoned flows are common — a retry leaves the first attempt open, and
      // the other client announces a fresh one every time you press verify —
      // and whichever arrived last used to win. That put a *stale* flow's id in
      // the modal, so the buttons then acted on a verification nobody was
      // looking at. `null` always wins: that is the user closing the dialog.
      if (verificationRequest && isLiveVerification(s.verificationRequest)) {
        return s.verificationRequest.flowId === verificationRequest.flowId ? s : {};
      }
      return { verificationRequest, sasState: null };
    }),

  applyVerificationUpdate: (update) =>
    set((s) => {
      // Updates are addressed to a flow, and until now nothing checked which.
      // Every abandoned verification keeps emitting — a superseded request is
      // cancelled by the other side, sometimes minutes later — and that cancel
      // would overwrite whatever was on screen. The symptom is the one that
      // sent us looking: a verification that is going fine suddenly reads "not
      // verified — the user cancelled", reporting the death of a flow the user
      // had already forgotten about.
      if (!s.verificationRequest || s.verificationRequest.flowId !== update.flowId) {
        return {};
      }
      if (isSasUpdate(update)) {
        // Only the SAS half moves. The request is deliberately left alone so
        // the modal stays up on completion and the user sees the outcome; it
        // dismisses on acknowledgement.
        return { sasState: update };
      }
      // Not `verificationRequest: null` on a finished request. Dropping it
      // closes the modal the instant the flow ends, which meant a verification
      // that failed just made the dialog vanish — no reason, nothing to read —
      // while the other client sat there saying "verification failed". Keep it
      // up and let the user close it once they've seen the outcome.
      return { verificationRequest: update };
    }),

  ringIncomingCall: (roomId) =>
    set((s) => {
      // The first ring wins. A second DM calling while one is already ringing
      // must not silently swap the room behind the answer button — the same
      // way a second verification request does not displace a running one.
      if (s.incomingCall) return {};
      return { incomingCall: { roomId, since: Date.now() } };
    }),

  stopIncomingCall: (roomId) =>
    set((s) => {
      // Addressed by room, so a call ending somewhere else cannot silence the
      // one the user is being asked about.
      if (!s.incomingCall) return {};
      if (roomId && s.incomingCall.roomId !== roomId) return {};
      return { incomingCall: null };
    }),

  showBanner: (tone, message) => set({ banner: { tone, message } }),
  dismissBanner: () => set({ banner: null }),

  reset: () => set({ ...initial, bootstrapped: true }),
}));

// ---------------------------------------------------------------------------
// selectors
// ---------------------------------------------------------------------------

export function selectActiveRoom(s: State): RoomSummary | undefined {
  return s.rooms.find((r) => r.id === s.activeRoomId);
}

export function selectActiveTimelineKey(s: State): string | null {
  if (!s.activeRoomId) return null;
  return timelineKey({
    roomId: s.activeRoomId,
    threadRoot: s.activeThreadRoot ?? undefined,
  });
}

export function selectDraft(s: State, key: string | null): Draft {
  return (key && s.drafts[key]) || EMPTY_DRAFT;
}

/**
 * The rooms the sidebar should show, after the space, filter and search are
 * applied. Spaces themselves are never listed as rooms — they're the rail.
 *
 * This is a plain function rather than a store selector on purpose: it builds a
 * new array every call, and zustand compares snapshots by reference, so passing
 * it to `useStore` would re-render forever. Callers memoise it instead.
 */
export function filterRooms(
  rooms: RoomSummary[],
  {
    activeSpaceId,
    filter,
    search,
    spaces,
  }: Pick<State, "activeSpaceId" | "filter" | "search" | "spaces">,
): RoomSummary[] {
  const needle = search.trim().toLowerCase();

  // Space membership is recorded twice in Matrix and neither half is reliable
  // alone: the space lists children via `m.space.child`, and a room *may* point
  // back with `m.space.parent` — but most rooms never set the parent event. So
  // the children list is the primary source and `parentSpaces` is a fallback.
  const children = activeSpaceId
    ? new Set(spaces.find((s) => s.id === activeSpaceId)?.children ?? [])
    : null;

  const visible = rooms.filter((room) => {
    if (room.isSpace) return false;
    // Rooms that exist to hold something — an image pack, say — are not places
    // anyone talks, so they don't belong in a list of conversations.
    if (room.isUtility) return false;
    if (room.membership === "left" || room.membership === "banned") return false;

    if (activeSpaceId) {
      const inSpace =
        children?.has(room.id) || room.parentSpaces.includes(activeSpaceId);
      if (!inSpace) return false;
    }

    switch (filter) {
      case "unread":
        if (!room.hasUnread) return false;
        break;
      case "dms":
        if (!room.isDirect) return false;
        break;
      case "muted":
        if (!room.isMuted) return false;
        break;
    }

    if (needle) {
      const haystack = `${room.name} ${room.canonicalAlias ?? ""} ${room.topic ?? ""}`;
      if (!haystack.toLowerCase().includes(needle)) return false;
    }

    return true;
  });

  // Sort here rather than trusting the arrival order of diffs: a snapshot and a
  // stream of updates shouldn't produce a different-looking list.
  return visible.sort((a, b) => {
    const at = a.latest?.timestamp ?? a.recency;
    const bt = b.latest?.timestamp ?? b.recency;
    return bt - at || a.name.localeCompare(b.name);
  });
}

export interface RoomGroup {
  title: string;
  rooms: RoomSummary[];
}

/**
 * Invites first (they're a question waiting on you), then people, then rooms.
 * Within each group the order comes from `filterRooms`.
 */
export function groupRooms(rooms: RoomSummary[]): RoomGroup[] {
  const invites = rooms.filter((r) => r.membership === "invited");
  const joined = rooms.filter((r) => r.membership !== "invited");

  const groups: RoomGroup[] = [];
  if (invites.length) groups.push({ title: "invites", rooms: invites });

  const dms = joined.filter((r) => r.isDirect);
  const channels = joined.filter((r) => !r.isDirect);

  if (dms.length) groups.push({ title: "direct messages", rooms: dms });
  if (channels.length) groups.push({ title: "rooms", rooms: channels });

  return groups;
}

export function selectUnreadTotal(s: State): number {
  return s.rooms.reduce((total, room) => total + room.notificationCount, 0);
}
