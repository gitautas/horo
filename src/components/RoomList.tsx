import { useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { call, type CallParticipantView } from "../lib/call";
import type { NotifyMode } from "../lib/bindings";
import { accentFor, displayNameFor, formatStamp, localpart } from "../lib/display";
import { haptic } from "../lib/haptics";
import * as ipc from "../lib/ipc";
import { cachedProfile, loadProfile } from "../lib/profiles";
import type { Profile, PublicRoom, RoomSummary } from "../lib/types";
import { useIsMobile, useLongPress } from "../lib/viewport";
import {
  filterRooms,
  groupRooms,
  withUnjoined,
  selectUnreadTotal,
  useStore,
  type RoomFilter,
} from "../store";
import { useCallState } from "./CallBar";
import { ContextMenu, type MenuEntry } from "./ContextMenu";
import { waitForRoom } from "./CreateRoom";
import { PresenceDot } from "./Presence";
import { VoicePanel } from "./VoicePanel";
import {
  Avatar,
  Button,
  ChannelBadge,
  dragOff,
  dragRegion,
  HoverRow,
  Icon,
  Spinner,
} from "./ui";
import { corner, headingSize, labelText, space, textSize } from "../lib/scale";

const FILTERS: RoomFilter[] = ["all", "unread", "dms", "muted"];

/** `onOpenSpaces` is passed only by the mobile shell, where this list is the
    root view and the spaces rail is a drawer rather than a permanent column.
    See `ChatPane` for why the prop doubles as the layout switch. */
export function RoomList({
  onOpenSpaces,
  width = 298,
}: { onOpenSpaces?: () => void; width?: number } = {}) {
  const {
    allRooms,
    activeRoomId,
    activeSpaceId,
    spaces,
    filter,
    search,
    setFilter,
    setSearch,
    selectRoom,
    openCreateRoom,
  } = useStore(
    useShallow((s) => ({
      allRooms: s.rooms,
      activeRoomId: s.activeRoomId,
      activeSpaceId: s.activeSpaceId,
      spaces: s.spaces,
      filter: s.filter,
      search: s.search,
      setFilter: s.setFilter,
      setSearch: s.setSearch,
      selectRoom: s.selectRoom,
      openCreateRoom: s.openCreateRoom,
    })),
  );

  const unreadTotal = useStore(selectUnreadTotal);
  const hideMuted = useStore(
    (s) => !!activeSpaceId && s.settings.hideMutedInSpaces.includes(activeSpaceId),
  );

  // The chips are hidden until asked for — they're a rarely-used control and
  // they crowd the top of the list.
  const filtered = filter !== "all";
  const [filtersOpen, setFiltersOpen] = useState(filtered);

  // Both of these allocate, so they have to be memoised rather than derived
  // inside a store selector.
  const activeSpace = spaces.find((sp) => sp.id === activeSpaceId);
  const unjoined = useUnjoinedRooms(activeSpace?.id ?? null, activeSpace?.children ?? []);

  const groups = useMemo(() => {
    const joined = groupRooms(
      filterRooms(allRooms, { activeSpaceId, filter, search, spaces, hideMuted }),
      activeSpace?.children,
    );
    // Only when the list is showing everything: a filter for unread or muted
    // rooms has nothing to say about rooms we aren't in.
    if (filter !== "all") return joined;
    const needle = search.trim().toLowerCase();
    const showing = unjoined.filter(
      (r) =>
        !allRooms.some(
          (x) => x.id === r.id && (x.membership === "joined" || x.membership === "invited"),
        ) && (!needle || (r.name ?? "").toLowerCase().includes(needle)),
    );
    return withUnjoined(joined, showing, activeSpace?.children);
  }, [allRooms, activeSpaceId, activeSpace, filter, search, spaces, hideMuted, unjoined]);

  const heading =
    spaces.find((s) => s.id === activeSpaceId)?.name.toLowerCase() ??
    "everything";

  return (
    <div
      style={{
        position: "relative",
        zIndex: 1,
        width: onOpenSpaces ? "100%" : width,
        flex: onOpenSpaces ? 1 : "none",
        display: "flex",
        flexDirection: "column",
        borderRight: "1px solid var(--border-subtle)",
        background: "color-mix(in srgb, var(--ink-900) 72%, transparent)",
      }}
    >
      <div
        {...dragRegion(!onOpenSpaces)}
        style={{
          padding: space(onOpenSpaces
            ? "calc(var(--safe-top) + 12px) 14px 12px"
            : "18px 18px 12px"),
        }}
      >
        <div
          style={{
            display: "flex",
            // A button won't sit on a text baseline; centre the row once one
            // is in it.
            alignItems: onOpenSpaces ? "center" : "baseline",
            justifyContent: "space-between",
            gap: space(8),
          }}
        >
          {onOpenSpaces && (
            <button
              onClick={onOpenSpaces}
              aria-label="spaces"
              {...dragOff}
              style={{
                flex: "none",
                width: 44,
                height: 44,
                marginLeft: -12,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                cursor: "pointer",
              }}
            >
              <Icon name="squares-four" size={20} color="var(--text-secondary)" />
            </button>
          )}

          <div
            className="uwu-ellipsis"
            style={{
              fontFamily: "var(--font-heading)",
              fontWeight: 800,
              fontSize: headingSize(22),
              letterSpacing: "-0.02em",
            }}
          >
            {heading}~
          </div>
          <div
            {...dragOff}
            style={{ display: "flex", alignItems: "center", gap: space(10) }}
          >
            <span
              style={{
                fontFamily: "var(--font-mono)",
                fontSize: textSize(11),
                color: "var(--text-tertiary)",
                whiteSpace: "nowrap",
              }}
            >
              {unreadTotal > 0 ? `${unreadTotal} unread` : "all caught up"}
            </span>
            <button
              onClick={openCreateRoom}
              title="make or join a room"
              aria-label="make or join a room"
              style={{
                width: 26,
                height: 26,
                flex: "none",
                borderRadius: 999,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                cursor: "pointer",
                background: "var(--surface-card)",
                border: "1px solid var(--border-default)",
                transition: "transform var(--dur-fast) var(--ease-bounce)",
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.transform = "rotate(90deg)";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.transform = "";
              }}
            >
              <Icon name="plus" size={13} color="var(--text-secondary)" />
            </button>
          </div>
        </div>

        <div
          {...dragOff}
          style={{
            marginTop: 12,
            display: "flex",
            alignItems: "center",
            gap: space(8),
            background: "var(--surface-inset)",
            border: "1px solid var(--border-subtle)",
            borderRadius: 999,
            padding: space("9px 14px"),
          }}
        >
          <Icon
            name="magnifying-glass"
            size={14}
            color="var(--text-tertiary)"
          />
          <input
            className="selectable"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="find a room or friend~"
            style={{
              flex: 1,
              background: "transparent",
              border: "none",
              outline: "none",
              color: "var(--text-primary)",
              fontSize: textSize(13),
            }}
          />
          {search && (
            <button
              onClick={() => setSearch("")}
              style={{ cursor: "pointer", display: "flex" }}
            >
              <Icon name="x-circle" size={14} color="var(--text-tertiary)" />
            </button>
          )}
          <button
            onClick={() => setFiltersOpen((v) => !v)}
            title={filtersOpen ? "hide filters" : "filter rooms"}
            style={{ cursor: "pointer", display: "flex", position: "relative" }}
          >
            <Icon
              name="funnel"
              size={15}
              color={
                filtered || filtersOpen
                  ? "var(--accent-primary)"
                  : "var(--text-tertiary)"
              }
            />
            {/* A dot, so an active filter is visible while the chips are hidden. */}
            {filtered && !filtersOpen && (
              <span
                style={{
                  position: "absolute",
                  top: -2,
                  right: -2,
                  width: 6,
                  height: 6,
                  borderRadius: "50%",
                  background: "var(--accent-primary)",
                }}
              />
            )}
          </button>
        </div>

        {filtersOpen && (
          <div
            {...dragOff}
            style={{ display: "flex", gap: space(6), marginTop: 12 }}
          >
            {FILTERS.map((f) => (
              <button
                key={f}
                onClick={() => setFilter(f)}
                style={{
                  fontFamily: "var(--font-rave)",
                  fontSize: textSize(9.5),
                  fontWeight: 800,
                  letterSpacing: "0.1em",
                  textTransform: "uppercase",
                  padding: space("6px 11px"),
                  borderRadius: corner(12),
                  cursor: "pointer",
                  transition: "all var(--dur-fast) var(--ease-bounce)",
                  ...(filter === f
                    ? {
                        background: "var(--accent-primary)",
                        color: "var(--text-on-accent)",
                        border: "1px solid var(--ink-950)",
                      }
                    : {
                        background: "var(--surface-card-raised)",
                        color: "var(--text-tertiary)",
                        border: "1px solid var(--border-subtle)",
                      }),
                }}
              >
                {f}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="uwu-scroll" style={{ flex: 1, padding: space("0 10px 16px") }}>
        {groups.length === 0 && (
          <div
            style={{
              padding: space("40px 16px"),
              textAlign: "center",
              color: "var(--text-tertiary)",
              fontSize: textSize(13),
              lineHeight: 1.6,
            }}
          >
            {search ? (
              "nothing matches that~"
            ) : filter !== "all" ? (
              `no ${filter} rooms right now`
            ) : (
              <>
                <div>
                  {activeSpaceId
                    ? "nothing here yet — this space has no rooms you've joined"
                    : "no rooms yet~"}
                </div>
                {/* This used to say "join one to get started" and then offer no
                    way to do it. */}
                <div style={{ marginTop: 14 }}>
                  <Button size="sm" onClick={openCreateRoom}>
                    make or join a room
                  </Button>
                </div>
              </>
            )}
          </div>
        )}

        {groups.map((group) => (
          <div key={group.title}>
            <div
              style={{
                padding: space("14px 8px 6px"),
                ...labelText(9.5),
                color: "var(--text-tertiary)",
                display: "flex",
                alignItems: "center",
                gap: space(8),
              }}
            >
              <span>{group.title}</span>
              <span
                style={{
                  flex: 1,
                  height: 1,
                  background: "var(--border-subtle)",
                }}
              />
              <span>{group.rooms.length + (group.unjoined?.length ?? 0)}</span>
            </div>

            {group.rooms.map((room) => (
              <RoomEntry
                key={room.id}
                room={room}
                active={room.id === activeRoomId}
                onSelect={() => {
                  void selectRoom(room.id);
                  if (isVoiceChannel(room)) void joinVoice(room.id);
                }}
              />
            ))}
            {group.unjoined?.map((room) => (
              <UnjoinedRow key={room.id} room={room} via={activeSpace?.id ?? null} />
            ))}
          </div>
        ))}
      </div>

      <VoicePanel />
    </div>
  );
}

/**
 * A room in the list, with its context menu — right-click, or press and hold
 * on a phone, the same as a space in the rail.
 */
function RoomEntry({
  room,
  active,
  onSelect,
}: {
  room: RoomSummary;
  active: boolean;
  onSelect: () => void;
}) {
  const isMobile = useIsMobile();
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const row = useRef<HTMLDivElement>(null);
  // A long press ends in a touchend, which the browser follows with a click
  // that would open the room behind the menu. Swallow that one click.
  const pressed = useRef(false);

  const longPress = useLongPress(
    isMobile
      ? () => {
          haptic("medium");
          pressed.current = true;
          const r = row.current?.getBoundingClientRect();
          if (r) setMenuAt({ x: r.left + 24, y: r.bottom });
        }
      : undefined,
  );

  return (
    <div
      ref={row}
      {...longPress}
      onContextMenu={(e) => {
        e.preventDefault();
        // On a phone the long press already opened it; Android fires
        // contextmenu for the same hold.
        if (!pressed.current) setMenuAt({ x: e.clientX, y: e.clientY });
      }}
      onClickCapture={(e) => {
        if (pressed.current) {
          pressed.current = false;
          e.stopPropagation();
        }
      }}
    >
      <RoomRow room={room} active={active} onSelect={onSelect} />
      {isVoiceChannel(room) && <VoiceRoster room={room} />}
      {menuAt && <RoomMenu room={room} at={menuAt} onClose={() => setMenuAt(null)} />}
    </div>
  );
}

const NOTIFY_LABELS: Record<NotifyMode, string> = {
  all: "all messages",
  mentions: "only @mentions",
  mute: "nothing",
};

function RoomMenu({
  room,
  at,
  onClose,
}: {
  room: RoomSummary;
  at: { x: number; y: number };
  onClose: () => void;
}) {
  const { openRoomDialog, showBanner } = useStore(
    useShallow((s) => ({ openRoomDialog: s.openRoomDialog, showBanner: s.showBanner })),
  );
  // Asked for when the menu opens: "delete" only belongs on it for someone
  // who can actually do it, and that takes the room's power levels.
  const [canDelete, setCanDelete] = useState(false);
  const invited = room.membership === "invited";

  useEffect(() => {
    if (room.isDirect || invited) return;
    let cancelled = false;
    ipc
      .getRoomPermissions(room.id)
      .then((p) => !cancelled && setCanDelete(p.canDelete))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [room.id, room.isDirect, invited]);

  function run(work: Promise<void>) {
    work.catch((e) => showBanner("error", ipc.asUwuError(e).message));
  }

  const mode = room.notifyMode ?? "all";

  const items: MenuEntry[] = invited
    ? [{ label: "decline invite", danger: true, onSelect: () => openRoomDialog("leave", room.id) }]
    : [
        {
          label: "mark as read",
          disabled: !room.hasUnread,
          onSelect: () => run(ipc.markRoomsRead([room.id])),
        },
        { kind: "separator" },
        {
          kind: "check",
          label: room.isDirect ? "mute chat" : "mute room",
          checked: room.isMuted,
          onToggle: (next) => run(ipc.setRoomsNotifyMode([room.id], next ? "mute" : null)),
        },
        {
          kind: "submenu",
          label: "notification settings",
          hint: NOTIFY_LABELS[mode],
          items: (["all", "mentions", "mute"] as const).map((m) => ({
            kind: "radio" as const,
            label: NOTIFY_LABELS[m],
            checked: mode === m,
            onSelect: () => run(ipc.setRoomsNotifyMode([room.id], m)),
          })),
        },
        { kind: "separator" },
        {
          label: room.isDirect ? "chat settings" : "edit room",
          icon: "pencil-simple",
          onSelect: () => openRoomDialog("settings", room.id),
        },
        {
          label: room.isDirect ? "leave chat" : "leave room",
          icon: "sign-out",
          danger: true,
          onSelect: () => openRoomDialog("leave", room.id),
        },
        ...(canDelete
          ? [
              {
                label: "delete room",
                icon: "trash",
                danger: true,
                onSelect: () => openRoomDialog("delete", room.id),
              } satisfies MenuEntry,
            ]
          : []),
      ];

  items.push(
    { kind: "separator" },
    {
      label: "copy room id",
      icon: "identification-card",
      onSelect: () =>
        run(
          navigator.clipboard
            .writeText(room.id)
            .then(() => showBanner("info", "room id copied")),
        ),
    },
  );

  return <ContextMenu at={at} items={items} onClose={onClose} />;
}

function RoomRow({
  room,
  active,
  onSelect,
}: {
  room: RoomSummary;
  active: boolean;
  onSelect: () => void;
}) {
  const accent = accentFor(room.id);
  const preview = previewFor(room);

  return (
    <HoverRow
      onClick={onSelect}
      active={active}
      title={room.canonicalAlias ?? room.id}
      style={{
        display: "flex",
        gap: space(10),
        alignItems: "center",
        padding: space("9px 10px"),
        borderRadius: corner(16),
        marginBottom: 2,
        ...(active ? { boxShadow: `inset 3px 0 0 ${accent}` } : {}),
      }}
    >
      <div style={{ position: "relative", flex: "none" }}>
        <Avatar
          id={room.id}
          name={room.name}
          mxc={room.avatarUrl}
          size={36}
          radius={13}
        />
        {/* A space invite is a server, not a channel. */}
        {!room.isDirect && !room.isSpace && (
          <ChannelBadge kind={room.isVideoRoom ? "video" : "text"} />
        )}
        {/* A live call owns the same corner and is the more urgent of the two,
            so presence stands down while one is running. */}
        {room.dmUserId && !room.hasActiveCall && (
          <PresenceDot userId={room.dmUserId} size={12} ring="var(--ink-950)" />
        )}
        {room.hasActiveCall && (
          <div
            style={{
              position: "absolute",
              right: -3,
              bottom: -3,
              width: 14,
              height: 14,
              borderRadius: "50%",
              background: "var(--status-online)",
              border: "2px solid var(--ink-900)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
            }}
            title="call in progress"
          >
            <Icon name="phone-call" size={7} color="var(--ink-950)" />
          </div>
        )}
      </div>

      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: space(5) }}>
          <span
            className="uwu-ellipsis"
            style={{
              fontFamily: "var(--font-display)",
              fontWeight: 700,
              fontSize: textSize(14.5),
              color: room.hasUnread
                ? "var(--text-primary)"
                : "var(--text-secondary)",
            }}
          >
            {room.name}
          </span>
          {room.isEncrypted && (
            <Icon name="shield-check" size={12} color="var(--accent-primary)" />
          )}
          {room.isMuted && (
            <Icon name="bell-slash" size={12} color="var(--text-tertiary)" />
          )}
        </div>
        <div
          className="uwu-ellipsis"
          style={{ fontSize: textSize(12), color: "var(--text-tertiary)", marginTop: 1 }}
        >
          {preview}
        </div>
      </div>

      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "flex-end",
          gap: space(4),
          flex: "none",
        }}
      >
        <span
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10),
            color: "var(--text-tertiary)",
          }}
        >
          {formatStamp(room.latest?.timestamp ?? 0)}
        </span>
        {room.hasUnread && (
          <span
            style={{
              minWidth: 19,
              height: 19,
              padding: space("0 6px"),
              borderRadius: 999,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontFamily: "var(--font-rave)",
              fontSize: textSize(9.5),
              fontWeight: 800,
              color: "var(--text-on-accent)",
              background:
                room.highlightCount > 0
                  ? "var(--accent-secondary)"
                  : "var(--accent-primary)",
            }}
          >
            {room.notificationCount > 0
              ? room.notificationCount > 99
                ? "99+"
                : room.notificationCount
              : "·"}
          </span>
        )}
      </div>
    </HoverRow>
  );
}

/**
 * The open space's rooms that anyone in it can join, fetched from the space's
 * hierarchy — the room list only knows rooms we're already in.
 *
 * Asked again when the space's children change, so a room someone adds shows
 * up without reopening the space.
 */
function useUnjoinedRooms(spaceId: string | null, children: string[]): PublicRoom[] {
  const [found, setFound] = useState<{ spaceId: string; rooms: PublicRoom[] } | null>(null);
  const childrenKey = children.join(",");

  useEffect(() => {
    if (!spaceId) return;
    let live = true;
    ipc
      .getSpaceRooms(spaceId)
      .then((rooms) => live && setFound({ spaceId, rooms }))
      // Not worth a banner: the joined rooms are all still there.
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [spaceId, childrenKey]);

  return useMemo(
    () =>
      found && found.spaceId === spaceId
        ? found.rooms.filter(
            // Invite-only rooms can't be walked into, and Discord doesn't show
            // you channels you can't open either.
            (r) => !r.isSpace && (r.joinRule === "public" || r.joinRule === "restricted"),
          )
        : [],
    [found, spaceId],
  );
}

/**
 * A room in the space we haven't joined. Clicking it joins, then opens it —
 * and for a voice room, connects, as clicking any voice room does.
 */
function UnjoinedRow({ room, via }: { room: PublicRoom; via: string | null }) {
  const [busy, setBusy] = useState(false);
  const name = room.name ?? room.alias ?? "a room";

  async function join() {
    if (busy) return;
    setBusy(true);
    const { selectRoom, showBanner } = useStore.getState();
    try {
      // A child that names no servers is on the space's own, which ours knows.
      await ipc.joinRoom(room.id, room.via.length ? room.via : via ? [serverOf(via)] : []);
      await waitForRoom(room.id);
      await selectRoom(room.id);
      if (room.isVoice) void joinVoice(room.id);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
      setBusy(false);
    }
  }

  return (
    <HoverRow
      onClick={() => void join()}
      title={`join ${name}`}
      style={{
        display: "flex",
        gap: space(10),
        alignItems: "center",
        padding: space("9px 10px"),
        borderRadius: corner(16),
        marginBottom: 2,
      }}
    >
      <div style={{ position: "relative", flex: "none", opacity: 0.55 }}>
        <Avatar id={room.id} name={name} mxc={room.avatarUrl} size={36} radius={13} />
        <ChannelBadge kind={room.isVoice ? "video" : "text"} />
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          className="uwu-ellipsis"
          style={{
            fontFamily: "var(--font-display)",
            fontWeight: 700,
            fontSize: textSize(14.5),
            color: "var(--text-tertiary)",
          }}
        >
          {name}
        </div>
        <div
          className="uwu-ellipsis"
          style={{ fontSize: textSize(12), color: "var(--text-tertiary)", marginTop: 1 }}
        >
          {room.isVoice ? "click to join and hop in" : "click to join"}
        </div>
      </div>
      {busy ? (
        <Spinner size={14} />
      ) : (
        <Icon name="plus-circle" size={16} color="var(--text-tertiary)" />
      )}
    </HoverRow>
  );
}

/** `!abc:veil.gg` → `veil.gg`. */
function serverOf(roomId: string): string {
  return roomId.slice(roomId.indexOf(":") + 1);
}

/** A video room we're in, which the sidebar treats as a Discord voice channel. */
function isVoiceChannel(room: RoomSummary): boolean {
  return room.isVideoRoom && room.membership === "joined";
}

/**
 * Clicking a voice channel is joining it, the way Discord does it. Already
 * being in it means the click only opens the room, and being in another call
 * means moving over — `call.join` leaves the old one first.
 *
 * Mic only: walking into a channel shouldn't turn a camera on. The camera
 * button in the call is one tap away.
 */
async function joinVoice(roomId: string) {
  const { roomId: current, status } = call.getState();
  if (current === roomId && status !== "idle" && status !== "failed") return;
  try {
    await call.join(roomId, { video: false });
  } catch (e) {
    useStore.getState().showBanner("error", ipc.asUwuError(e).message);
  }
}

/**
 * Who's in a voice channel, listed under it.
 *
 * The room's call membership state says who is in any channel. For the one
 * we're connected to, LiveKit knows sooner and knows more — who's talking,
 * who's muted — so that list wins while we're in it.
 */
function VoiceRoster({ room }: { room: RoomSummary }) {
  const state = useCallState();
  const live = state.roomId === room.id && state.status === "connected";

  const entries = live
    ? dedupeByUser(state.participants).map((p) => ({
        userId: p.userId,
        isSpeaking: p.isSpeaking,
        isMuted: p.isMuted,
      }))
    : room.callParticipants.map((userId) => ({
        userId,
        isSpeaking: false,
        isMuted: false,
      }));

  if (entries.length === 0) return null;

  return (
    <div style={{ padding: space("0 0 6px 30px") }}>
      {entries.map((entry) => (
        <RosterEntry key={entry.userId} {...entry} />
      ))}
    </div>
  );
}

/** One row per person: someone in on two devices is still one person. */
function dedupeByUser(participants: CallParticipantView[]): CallParticipantView[] {
  const seen = new Set<string>();
  return participants.filter((p) => {
    if (seen.has(p.userId)) return false;
    seen.add(p.userId);
    return true;
  });
}

function RosterEntry({
  userId,
  isSpeaking,
  isMuted,
}: {
  userId: string;
  isSpeaking: boolean;
  isMuted: boolean;
}) {
  const [profile, setProfile] = useState<Profile | null>(
    () => cachedProfile(userId) ?? null,
  );

  useEffect(() => {
    let cancelled = false;
    loadProfile(userId)
      .then((p) => !cancelled && setProfile(p))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [userId]);

  const name = displayNameFor(userId, profile?.displayName);

  return (
    <div
      title={userId}
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(8),
        padding: space("3px 10px"),
      }}
    >
      <div
        style={{
          flex: "none",
          borderRadius: corner(8),
          outline: isSpeaking ? "2px solid var(--accent-primary)" : "2px solid transparent",
          outlineOffset: 1,
          transition: "outline-color var(--dur-fast) var(--ease-out)",
        }}
      >
        <Avatar id={userId} name={name} mxc={profile?.avatarUrl} size={20} radius={7} />
      </div>
      <span
        className="uwu-ellipsis"
        style={{
          flex: 1,
          minWidth: 0,
          fontSize: textSize(12.5),
          color: isSpeaking ? "var(--text-primary)" : "var(--text-secondary)",
        }}
      >
        {name}
      </span>
      {isMuted && <Icon name="microphone-slash" size={12} color="var(--status-danger)" />}
    </div>
  );
}

function previewFor(room: RoomSummary): string {
  if (room.membership === "invited") return "invited you~";
  if (!room.latest) return room.topic ?? "no messages yet";

  const who = room.latest.senderName ?? localpart(room.latest.sender);
  // In a DM the other person's name is already the room name, so repeating it
  // in the preview just wastes the line.
  return room.isDirect ? room.latest.body : `${who}: ${room.latest.body}`;
}
