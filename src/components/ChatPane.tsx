import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { call } from "../lib/call";
import * as ipc from "../lib/ipc";
import type { RoomSummary } from "../lib/types";
import { useStore } from "../store";
import { CallBar, useCallState } from "./CallBar";
import { PresenceLine, PresenceDot } from "./Presence";
import { CallStage } from "./CallStage";
import { Composer } from "./Composer";
import { TimelineView } from "./TimelineView";
import { VerifyBanner } from "./VerificationModal";
import { Avatar, ChannelBadge, dragOff, dragRegion, Icon } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

/**
 * `onBack` is supplied only by the mobile shell, where this pane is the whole
 * screen rather than a column. Its presence is what switches the pane into
 * full-bleed mode — there is no separate `isMobile` check, so the two can never
 * disagree about which layout is on screen.
 */
export function ChatPane({ room, onBack }: { room: RoomSummary; onBack?: () => void }) {
  const { threadRoot, showInfo, toggleInfo, openThread, showBanner } = useStore(
    useShallow((s) => ({
      threadRoot: s.activeThreadRoot,
      showInfo: s.showInfo,
      toggleInfo: s.toggleInfo,
      openThread: s.openThread,
      showBanner: s.showBanner,
    })),
  );

  const callState = useCallState();
  const inThisCall = callState.roomId === room.id && callState.status !== "idle";

  async function toggleCall() {
    try {
      if (inThisCall) {
        await call.leave();
      } else {
        // A video room is a call you join, so bring the camera with you.
        await call.join(room.id, { video: room.isVideoRoom });
      }
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  return (
    <div
      style={{
        position: "relative",
        zIndex: 1,
        flex: 1,
        // The desktop floor keeps the timeline readable beside the other
        // columns; on a phone this pane *is* the window, so it takes what it
        // gets. `0` rather than `auto` so a long unbroken message can't push
        // the pane wider than the screen.
        minWidth: onBack ? 0 : 480,
        display: "flex",
        flexDirection: "column",
        background: "var(--surface-canvas)",
      }}
    >
      <div
        {...dragRegion(!onBack)}
        style={{
          display: "flex",
          alignItems: "center",
          gap: space(onBack ? 10 : 14),
          // The header is the top of the screen on a phone, so it carries the
          // notch inset itself rather than letting the status bar overlap it.
          padding: space(onBack ? "calc(var(--safe-top) + 8px) 12px 8px" : "14px 22px"),
          borderBottom: "1px solid var(--border-subtle)",
          background: "color-mix(in srgb, var(--ink-900) 60%, transparent)",
        }}
      >
        {onBack && (
          <button
            onClick={onBack}
            aria-label="back to rooms"
            style={{
              flex: "none",
              // 44 is Apple's minimum touch target; the icon inside is 18.
              width: 44,
              height: 44,
              marginLeft: -10,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              cursor: "pointer",
            }}
          >
            <Icon name="caret-left" size={18} color="var(--text-secondary)" />
          </button>
        )}

        <div style={{ position: "relative", flex: "none" }}>
          <Avatar
            id={room.id}
            name={room.name}
            mxc={room.avatarUrl}
            size={onBack ? 32 : 42}
            radius={onBack ? 11 : 15}
          />
          {!room.isDirect && (
            <ChannelBadge
              kind={room.isVideoRoom ? "video" : "text"}
              size={18}
              ring="var(--ink-950)"
            />
          )}
          {room.dmUserId && (
            <PresenceDot userId={room.dmUserId} size={13} ring="var(--ink-950)" />
          )}
        </div>

        <div style={{ minWidth: 0, flex: onBack ? 1 : undefined }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: space(8),
              minWidth: 0,
            }}
          >
            <span
              className="uwu-ellipsis"
              style={{
                fontFamily: "var(--font-heading)",
                fontWeight: 800,
                fontSize: headingSize(19),
                letterSpacing: "-0.01em",
              }}
            >
              {room.name}
            </span>
            {/* A lock is the whole message. Spelling out "e2e" beside it said
                the same thing twice, and did it in the row that has the least
                space to spare. The title carries the detail for anyone who
                wants it. */}
            {room.isEncrypted && (
              <span
                title="encrypted end-to-end — only people in this room can read it"
                aria-label="end-to-end encrypted"
                style={{ display: "flex", flex: "none" }}
              >
                <Icon name="lock-key" size={14} color="var(--accent-primary)" />
              </span>
            )}
          </div>
          {/* A DM's subtitle is the person, not the room: a topic and an
              alias are things group rooms have. */}
          {room.dmUserId ? (
            <PresenceLine
              userId={room.dmUserId}
              style={{
                marginTop: 3,
                fontSize: textSize(11.5),
                // "last seen 3h ago" wraps to three lines in a narrow header
                // and doubles its height. `PresenceLine` is a flex row, so
                // `text-overflow` would do nothing here — keeping it on one
                // line and clipping the overflow is the part that works.
                ...(onBack
                  ? { whiteSpace: "nowrap" as const, overflow: "hidden", minWidth: 0 }
                  : {}),
              }}
            />
          ) : (
            <div
              className="uwu-ellipsis"
              style={{
                fontSize: textSize(12.5),
                color: "var(--text-secondary)",
                maxWidth: 520,
              }}
            >
              {room.topic}{" "}
              <span
                style={{
                  fontFamily: "var(--font-mono)",
                  color: "var(--text-tertiary)",
                  fontSize: textSize(11.5),
                }}
              >
                {room.canonicalAlias ?? ""}
              </span>
            </div>
          )}
        </div>

        <div
          {...dragOff}
          style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: space(8) }}
        >
          {!onBack && (
            <span
              style={{
                fontFamily: "var(--font-mono)",
                fontSize: textSize(11),
                color: "var(--text-tertiary)",
                marginRight: 4,
              }}
            >
              {room.memberCount}
            </span>
          )}
          <HeaderButton
            icon={inThisCall ? "phone-x" : "phone-call"}
            title={inThisCall ? "leave the call" : "start a voice call"}
            highlight={inThisCall || room.hasActiveCall}
            danger={inThisCall}
            onClick={() => void toggleCall()}
          />
          <HeaderButton
            icon="info"
            title="room info"
            highlight={showInfo}
            onClick={toggleInfo}
          />
        </div>
      </div>

      <VerifyBanner />

      {threadRoot && (
        <ThreadHeader roomId={room.id} threadRoot={threadRoot} onClose={() => void openThread(null)} />
      )}

      <CallStage roomId={room.id} />

      <TimelineView roomId={room.id} threadRoot={threadRoot ?? undefined} />

      {/* A call in some other room stays as the compact strip, so it's still
          reachable without leaving the room you're reading. */}
      {callState.roomId !== room.id && <CallBar />}

      <Composer
        roomId={room.id}
        roomName={room.canonicalAlias ?? room.name}
        threadRoot={threadRoot ?? undefined}
        encrypted={room.isEncrypted}
      />
    </div>
  );
}

function ThreadHeader({
  roomId,
  threadRoot,
  onClose,
}: {
  roomId: string;
  threadRoot: string;
  onClose: () => void;
}) {
  const [preview, setPreview] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc
      .getEventBody(roomId, threadRoot)
      .then((body) => {
        if (!cancelled) setPreview(body);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [roomId, threadRoot]);

  return (
    <div
      style={{
        margin: "14px 22px 0",
        display: "flex",
        alignItems: "center",
        gap: space(10),
        padding: space("10px 14px"),
        borderRadius: corner(16),
        background: "var(--surface-card)",
        border: "1px solid var(--border-subtle)",
        borderLeft: "2px solid var(--accent-quaternary)",
      }}
    >
      <Icon name="chats-circle" size={15} color="var(--accent-quaternary)" />
      <span
        className="uwu-ellipsis"
        style={{ flex: 1, fontSize: textSize(13), color: "var(--text-secondary)" }}
      >
        thread{preview ? ` · ${preview}` : ""}
      </span>
      <button onClick={onClose} title="back to the room" style={{ cursor: "pointer", display: "flex" }}>
        <Icon name="x" size={14} color="var(--text-tertiary)" />
      </button>
    </div>
  );
}

function HeaderButton({
  icon,
  title,
  onClick,
  highlight,
  danger,
}: {
  icon: string;
  title: string;
  onClick: () => void;
  highlight?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      style={{
        padding: space(9),
        borderRadius: corner(12),
        background: danger ? "var(--status-danger)" : "var(--surface-card-raised)",
        display: "flex",
        cursor: "pointer",
      }}
    >
      <Icon
        name={icon}
        size={17}
        color={
          danger
            ? "var(--ink-950)"
            : highlight
              ? "var(--accent-primary)"
              : "var(--text-secondary)"
        }
      />
    </button>
  );
}

/** Shown when nothing is selected yet. */
export function EmptyPane() {
  const rooms = useStore((s) => s.rooms);

  return (
    <div
      style={{
        position: "relative",
        zIndex: 1,
        flex: 1,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: space(14),
        background: "var(--surface-canvas)",
        color: "var(--text-tertiary)",
      }}
    >
      <div
        style={{
          width: 84,
          height: 84,
          borderRadius: corner(28),
          background: "var(--surface-card)",
          border: "2px dashed var(--border-strong)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          transform: "rotate(calc(-4deg * var(--tilt-scale)))",
        }}
      >
        <Icon name="chats-teardrop" size={34} color="var(--text-tertiary)" />
      </div>
      <div
        style={{
          fontFamily: "var(--font-heading)",
          fontWeight: 800,
          fontSize: headingSize(19),
          color: "var(--text-secondary)",
        }}
      >
        pick a room~
      </div>
      <div style={{ fontSize: textSize(13) }}>
        {rooms.length === 0
          ? "still syncing your rooms…"
          : `${rooms.filter((r) => !r.isSpace).length} rooms waiting for you`}
      </div>
    </div>
  );
}
