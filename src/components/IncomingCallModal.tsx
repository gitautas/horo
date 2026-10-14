import { useShallow } from "zustand/react/shallow";

import { call } from "../lib/call";
import * as ipc from "../lib/ipc";
import { useStore } from "../store";
import { Avatar, Button, Icon } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

/**
 * Someone is calling.
 *
 * Only DMs get this far (see `notify.ts`), so there is exactly one person on
 * the other end and the room *is* them — which is why this shows their avatar
 * and their name rather than a room and a participant count.
 */
export function IncomingCallModal() {
  const { incoming, rooms, stop, selectRoom, showBanner } = useStore(
    useShallow((s) => ({
      incoming: s.incomingCall,
      rooms: s.rooms,
      stop: s.stopIncomingCall,
      selectRoom: s.selectRoom,
      showBanner: s.showBanner,
    })),
  );

  const room = incoming ? rooms.find((r) => r.id === incoming.roomId) : undefined;
  // A room we can't find is a call we can't describe or answer, so there is
  // nothing honest to put on screen. The ringing stops with the state either
  // way — `stopIncomingCall` is driven by the room list, not by this.
  if (!incoming || !room) return null;

  async function answer(): Promise<void> {
    if (!room) return;
    // Silence first. Joining takes a round trip to the SFU, and a ringtone that
    // carries on through it makes the app feel like it missed the tap.
    stop(room.id);
    try {
      await selectRoom(room.id);
      await call.join(room.id, { video: room.isVideoRoom });
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: "var(--surface-overlay)",
        backdropFilter: "var(--blur-glass)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        // Above the lightbox (200) and the panels, below the verification modal
        // (250): a verification has a timeout running on someone else's device
        // and cannot be re-raised, whereas a caller who gives up rings again.
        zIndex: 240,
      }}
    >
      <div
        style={{
          width: "min(380px, calc(100vw - 32px))",
          padding: space(28),
          borderRadius: corner(28),
          textAlign: "center",
          background: "var(--surface-card)",
          border: "1px solid color-mix(in srgb, var(--acid-500) 35%, transparent)",
          boxShadow: "var(--shadow-pop), 0 0 40px color-mix(in srgb, var(--acid-500) 14%, transparent)",
        }}
      >
        <div
          style={{
            display: "inline-flex",
            borderRadius: "50%",
            // The pulse is the ringing made visible — the one cue that survives
            // a muted device, which is most phones most of the time.
            animation: "uwuPulse 1.4s infinite",
          }}
        >
          <Avatar id={room.dmUserId ?? room.id} name={room.name} mxc={room.avatarUrl} size={78} />
        </div>

        <div
          style={{
            marginTop: 16,
            fontFamily: "var(--font-heading)",
            fontWeight: 800,
            fontSize: headingSize(20),
          }}
        >
          {room.name}
        </div>
        <div
          style={{
            marginTop: 4,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            gap: space(6),
            fontSize: textSize(13.5),
            color: "var(--text-secondary)",
          }}
        >
          <Icon name="phone-call" size={14} color="var(--accent-primary)" />
          {room.isVideoRoom ? "incoming video call~" : "incoming call~"}
        </div>

        <div style={{ marginTop: 26, display: "flex", gap: space(10), justifyContent: "center" }}>
          <Button variant="danger" onClick={() => stop(room.id)}>
            decline
          </Button>
          <Button onClick={() => void answer()}>answer~</Button>
        </div>

        <div
          style={{
            marginTop: 14,
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10.5),
            color: "var(--text-tertiary)",
          }}
        >
          declining only stops the ringing here — it doesn't hang up on them
        </div>
      </div>
    </div>
  );
}
