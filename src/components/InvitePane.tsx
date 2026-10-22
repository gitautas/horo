import { useState } from "react";

import { localpart } from "../lib/display";
import * as ipc from "../lib/ipc";
import type { RoomSummary } from "../lib/types";
import { useStore } from "../store";
import { waitForRoom } from "./CreateRoom";
import { waitForSpace } from "./ExploreSpaces";
import { Avatar, Button, Icon, Spinner } from "./ui";
import { headingSize, space, textSize } from "../lib/scale";

/**
 * An invite, where the room would be: who asked, and whether to go in.
 *
 * Accepting a space is joining a Discord server — it brings its rooms along,
 * so the person lands somewhere with things in it rather than an empty
 * folder.
 */
export function InvitePane({ room, onBack }: { room: RoomSummary; onBack?: () => void }) {
  const [busy, setBusy] = useState(false);
  const openRoomDialog = useStore((s) => s.openRoomDialog);
  // For an invite, the room's latest event is the invite itself.
  const inviter = room.latest?.senderName ?? (room.latest ? localpart(room.latest.sender) : null);
  const kind = room.isSpace ? "space" : room.isDirect ? "chat" : "room";

  async function accept() {
    if (busy) return;
    setBusy(true);
    const { selectRoom, setActiveSpace, refreshSpaces, showBanner } = useStore.getState();
    try {
      if (room.isSpace) {
        const result = await ipc.joinSpace(room.id);
        await waitForSpace(room.id, refreshSpaces);
        await selectRoom(null);
        setActiveSpace(room.id);
        if (result.leftOut > 0) {
          showBanner(
            "info",
            `you're in ${room.name}. ${result.leftOut === 1 ? "one room is" : `${result.leftOut} rooms are`} invite-only — ask someone there to let you in`,
          );
        }
      } else {
        await ipc.joinRoom(room.id);
        await waitForJoin(room.id);
        // Reopen it now it's a room with a timeline to read.
        await selectRoom(null);
        await selectRoom(room.id);
      }
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
      setBusy(false);
    }
  }

  return (
    <div
      style={{
        flex: 1,
        minWidth: 0,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: space(14),
        padding: space(24),
        textAlign: "center",
        position: "relative",
      }}
    >
      {onBack && (
        <button
          onClick={onBack}
          title="back"
          style={{
            position: "absolute",
            top: "calc(var(--safe-top) + 14px)",
            left: 14,
            cursor: "pointer",
          }}
        >
          <Icon name="caret-left" size={20} color="var(--text-secondary)" />
        </button>
      )}

      <Avatar id={room.id} name={room.name} mxc={room.avatarUrl} size={76} />
      <div>
        <div
          style={{
            fontFamily: "var(--font-display)",
            fontWeight: 800,
            fontSize: headingSize(20),
          }}
        >
          {room.name}
        </div>
        <div style={{ fontSize: textSize(13), color: "var(--text-secondary)", marginTop: 4 }}>
          {inviter ? `${inviter} invited you to this ${kind}` : `you're invited to this ${kind}`}
        </div>
        {room.isSpace && (
          <div
            style={{
              fontSize: textSize(12),
              color: "var(--text-tertiary)",
              marginTop: 6,
              maxWidth: 320,
              lineHeight: 1.45,
            }}
          >
            joining it puts you in its rooms too.
          </div>
        )}
      </div>

      <div style={{ display: "flex", gap: space(8), marginTop: 6 }}>
        <Button variant="ghost" onClick={() => openRoomDialog("leave", room.id)} disabled={busy}>
          decline
        </Button>
        <Button onClick={() => void accept()} disabled={busy}>
          {busy ? <Spinner size={14} color="var(--text-on-accent)" /> : "join"}
        </Button>
      </div>
    </div>
  );
}

/** Until sync says we're in: the join call returns before the room list knows. */
async function waitForJoin(roomId: string, timeoutMs = 5000): Promise<void> {
  await waitForRoom(roomId, timeoutMs);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const room = useStore.getState().rooms.find((r) => r.id === roomId);
    if (room?.membership === "joined") return;
    await new Promise((resolve) => setTimeout(resolve, 120));
  }
}
