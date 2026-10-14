import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import * as ipc from "../lib/ipc";
import type { RoomPermissions, RoomSummary } from "../lib/types";
import { useStore } from "../store";
import { Button, Modal } from "./ui";
import { corner, space, textSize } from "../lib/scale";

/**
 * Renaming a room.
 *
 * A dialog rather than another section in the room panel: that panel is a
 * narrow column that already carries the member list.
 *
 * The fields only appear when this account can actually change the thing — a
 * disabled input you can't act on is just clutter. Permissions are read once
 * per room rather than carried on every room summary.
 *
 * Leaving lives at the bottom, behind its own confirmation. It's deliberately
 * two clicks down a dialog nobody opens by accident: leaving is not reversible
 * from this side — an invite-only room you leave needs someone to invite you
 * back — and it should never sit next to anything you'd click in a hurry.
 */
export function RoomSettingsDialog({
  room,
  onClose,
}: {
  room: RoomSummary;
  onClose: () => void;
}) {
  const [permissions, setPermissions] = useState<RoomPermissions | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPermissions(null);
    ipc
      .getRoomPermissions(room.id)
      .then((p) => !cancelled && setPermissions(p))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [room.id]);

  const canEdit = permissions?.canRename || permissions?.canSetTopic;

  return (
    <Modal title={room.isDirect ? "this chat" : "room settings"} onClose={onClose}>
      {canEdit ? (
        <RoomEditor room={room} permissions={permissions!} />
      ) : (
        permissions && (
          <div style={{ fontSize: textSize(12.5), color: "var(--text-tertiary)", lineHeight: 1.5 }}>
            you can't rename this one — that needs more power than you have here.
          </div>
        )
      )}

      <LeaveRoom room={room} onLeft={onClose} />
    </Modal>
  );
}

/**
 * The way out, kept quiet until it's asked for.
 *
 * Two steps rather than a `confirm()`: the first click is a plain link at the
 * foot of the dialog, and only then does the room's name appear on a button
 * that does the thing. Nothing is deleted — leaving a room drops it from your
 * list and leaves everyone else's copy alone.
 */
function LeaveRoom({ room, onLeft }: { room: RoomSummary; onLeft: () => void }) {
  const { selectRoom, showBanner } = useStore(
    useShallow((s) => ({ selectRoom: s.selectRoom, showBanner: s.showBanner })),
  );
  const [asked, setAsked] = useState(false);
  const [leaving, setLeaving] = useState(false);

  // An invite you haven't taken up is declined rather than left, and a dm is a
  // person rather than a place. Same call underneath; different sentence.
  const invited = room.membership === "invited";
  const verb = invited ? "decline invite" : room.isDirect ? "leave this chat" : "leave room";

  async function leave() {
    if (leaving) return;
    setLeaving(true);
    try {
      await ipc.leaveRoom(room.id);
      // Close before deselecting: the room is about to vanish from the list
      // underneath this dialog, and a modal anchored to a room that's gone is
      // a stale thing to leave on screen.
      onLeft();
      await selectRoom(null);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
      setLeaving(false);
      setAsked(false);
    }
  }

  return (
    <div
      style={{
        marginTop: 20,
        paddingTop: space(14),
        borderTop: "1px solid var(--border-subtle)",
      }}
    >
      {asked ? (
        <>
          <div
            style={{
              fontSize: textSize(12.5),
              color: "var(--text-secondary)",
              lineHeight: 1.55,
              marginBottom: 10,
            }}
          >
            {invited
              ? `decline the invite to ${room.name}?`
              : `leave ${room.name}? it drops off your list, and you'd need an invite to get back into a private room.`}
          </div>
          <div style={{ display: "flex", gap: space(8), justifyContent: "flex-end" }}>
            <Button size="sm" variant="ghost" onClick={() => setAsked(false)}>
              stay
            </Button>
            <Button
              size="sm"
              variant="danger"
              disabled={leaving}
              onClick={() => void leave()}
            >
              {leaving ? "leaving…" : verb}
            </Button>
          </div>
        </>
      ) : (
        <button
          onClick={() => setAsked(true)}
          style={{
            cursor: "pointer",
            background: "none",
            border: "none",
            padding: 0,
            fontFamily: "var(--font-body)",
            fontSize: textSize(12.5),
            color: "var(--status-danger)",
          }}
        >
          {verb}…
        </button>
      )}
    </div>
  );
}

function RoomEditor({
  room,
  permissions,
}: {
  room: RoomSummary;
  permissions: RoomPermissions;
}) {
  const showBanner = useStore((s) => s.showBanner);
  const [name, setName] = useState(room.name);
  const [topic, setTopic] = useState(room.topic ?? "");
  const [saving, setSaving] = useState(false);

  // The room can be renamed by someone else while this panel is open; follow
  // the server rather than leaving a stale draft sitting in the box.
  useEffect(() => {
    setName(room.name);
    setTopic(room.topic ?? "");
  }, [room.id, room.name, room.topic]);

  const dirty =
    (permissions.canRename && name.trim() !== room.name) ||
    (permissions.canSetTopic && topic.trim() !== (room.topic ?? ""));

  async function save() {
    if (!dirty || saving) return;
    setSaving(true);
    try {
      await ipc.updateRoom(room.id, {
        name: permissions.canRename && name.trim() !== room.name ? name.trim() : undefined,
        topic:
          permissions.canSetTopic && topic.trim() !== (room.topic ?? "")
            ? topic.trim()
            : undefined,
      });
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      {permissions.canRename && (
        <input
          className="selectable"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="room name"
          style={inputStyle}
        />
      )}

      {permissions.canSetTopic && (
        <textarea
          className="selectable"
          value={topic}
          onChange={(e) => setTopic(e.target.value)}
          placeholder="what's it for?"
          rows={2}
          style={{ ...inputStyle, marginTop: 8, resize: "none", lineHeight: 1.5 }}
        />
      )}

      {dirty && (
        <div style={{ display: "flex", justifyContent: "flex-end", marginTop: 8 }}>
          <Button size="sm" onClick={() => void save()} disabled={saving}>
            {saving ? "saving…" : "save"}
          </Button>
        </div>
      )}
    </>
  );
}

const inputStyle = {
  width: "100%",
  padding: space("9px 12px"),
  borderRadius: corner(14),
  background: "var(--surface-inset)",
  border: "1px solid var(--border-subtle)",
  outline: "none",
  color: "var(--text-primary)",
  fontSize: textSize(13),
  fontFamily: "var(--font-body)",
} as const;
