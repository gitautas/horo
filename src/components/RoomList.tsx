import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { accentFor, formatStamp, localpart } from "../lib/display";
import type { RoomSummary } from "../lib/types";
import {
  filterRooms,
  groupRooms,
  selectUnreadTotal,
  useStore,
  type RoomFilter,
} from "../store";
import { PresenceDot } from "./Presence";
import { Avatar, Button, ChannelBadge, dragOff, dragRegion, HoverRow, Icon } from "./ui";
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

  // The chips are hidden until asked for — they're a rarely-used control and
  // they crowd the top of the list.
  const filtered = filter !== "all";
  const [filtersOpen, setFiltersOpen] = useState(filtered);

  // Both of these allocate, so they have to be memoised rather than derived
  // inside a store selector.
  const groups = useMemo(
    () =>
      groupRooms(
        filterRooms(allRooms, { activeSpaceId, filter, search, spaces }),
      ),
    [allRooms, activeSpaceId, filter, search, spaces],
  );

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
              <span>{group.rooms.length}</span>
            </div>

            {group.rooms.map((room) => (
              <RoomRow
                key={room.id}
                room={room}
                active={room.id === activeRoomId}
                onSelect={() => void selectRoom(room.id)}
              />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
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
        {!room.isDirect && (
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

function previewFor(room: RoomSummary): string {
  if (room.membership === "invited") return "invited you~";
  if (!room.latest) return room.topic ?? "no messages yet";

  const who = room.latest.senderName ?? localpart(room.latest.sender);
  // In a DM the other person's name is already the room name, so repeating it
  // in the preview just wastes the line.
  return room.isDirect ? room.latest.body : `${who}: ${room.latest.body}`;
}
