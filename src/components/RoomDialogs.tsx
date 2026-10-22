import { useState } from "react";

import * as ipc from "../lib/ipc";
import type { RoomSummary } from "../lib/types";
import { useStore } from "../store";
import { RoomSettingsDialog } from "./RoomSettings";
import { Button, Modal, Spinner } from "./ui";
import { space, textSize } from "../lib/scale";

/**
 * The dialogs a room's context menu opens: its settings, leaving it, and
 * deleting it.
 *
 * Mounted from the shell; the menu is on each room in the sidebar —
 * right-click, or press and hold on a phone.
 */
export function RoomDialogs() {
  const dialog = useStore((s) => s.roomDialog);
  const close = useStore((s) => s.closeRoomDialog);
  const room = useStore((s) => s.rooms.find((r) => r.id === dialog?.roomId));
  // Gone from under us — left or deleted elsewhere.
  if (!dialog || !room) return null;

  switch (dialog.kind) {
    case "settings":
      return <RoomSettingsDialog room={room} onClose={close} />;
    case "leave":
      return <LeaveRoom room={room} />;
    case "delete":
      return <DeleteRoom room={room} />;
  }
}

/** Leave, straight from the menu: the menu item was the first click. */
function LeaveRoom({ room }: { room: RoomSummary }) {
  const close = useStore((s) => s.closeRoomDialog);
  const [leaving, setLeaving] = useState(false);

  const invited = room.membership === "invited";
  const verb = invited ? "decline" : "leave";

  async function leave() {
    if (leaving) return;
    setLeaving(true);
    const { activeRoomId, selectRoom, showBanner } = useStore.getState();
    try {
      await ipc.leaveRoom(room.id);
      close();
      if (activeRoomId === room.id) await selectRoom(null);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
      setLeaving(false);
    }
  }

  return (
    <Modal title={invited ? "decline the invite?" : `leave ${room.name}?`} onClose={close}>
      <Body>
        {invited
          ? `you won't be in ${room.name}, and they'd have to invite you again.`
          : "it drops off your list, and you'd need an invite to get back into a private room. everyone else keeps it."}
      </Body>
      <Actions>
        <Button variant="ghost" onClick={close}>
          stay
        </Button>
        <Button variant="danger" onClick={() => void leave()} disabled={leaving}>
          {leaving ? <Spinner size={14} color="var(--text-on-accent)" /> : verb}
        </Button>
      </Actions>
    </Modal>
  );
}

/**
 * Deleting a room, Discord's "delete channel".
 *
 * Unlike leaving, this happens to everyone, so the sentence says exactly what
 * it does — including the part Matrix can't do, which is erase the history
 * from the servers that already have it.
 */
function DeleteRoom({ room }: { room: RoomSummary }) {
  const close = useStore((s) => s.closeRoomDialog);
  const [deleting, setDeleting] = useState(false);

  async function remove() {
    if (deleting) return;
    setDeleting(true);
    const { activeRoomId, selectRoom, refreshSpaces, showBanner } = useStore.getState();
    try {
      await ipc.deleteRoom(room.id);
      close();
      if (activeRoomId === room.id) await selectRoom(null);
      // It just came out of its spaces, which only re-read on a slow poll.
      void refreshSpaces();
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
      setDeleting(false);
    }
  }

  return (
    <Modal title={`delete ${room.name}?`} onClose={close}>
      <Body>
        everyone is removed, it comes out of its spaces, and nobody can get back in. this
        can't be undone. the messages stay on the servers that already have them — matrix
        has no way to erase those.
      </Body>
      <Actions>
        <Button variant="ghost" onClick={close}>
          keep it
        </Button>
        <Button variant="danger" onClick={() => void remove()} disabled={deleting}>
          {deleting ? <Spinner size={14} color="var(--text-on-accent)" /> : "delete room"}
        </Button>
      </Actions>
    </Modal>
  );
}

function Body({ children }: { children: React.ReactNode }) {
  return (
    <div
      style={{
        fontSize: textSize(13),
        color: "var(--text-secondary)",
        lineHeight: 1.55,
      }}
    >
      {children}
    </div>
  );
}

function Actions({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ display: "flex", gap: space(8), justifyContent: "flex-end", marginTop: 20 }}>
      {children}
    </div>
  );
}
