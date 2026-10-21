import { useMemo, useState } from "react";

import * as ipc from "../lib/ipc";
import { roomsOnlyInSpace, useStore } from "../store";
import { SpaceRooms } from "./ExploreSpaces";
import { Button, Modal, RaveLabel, Spinner, Toggle } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

/**
 * The dialogs a space's context menu opens: leave, invite, and browse its rooms.
 *
 * Mounted from the shell; the menu is on each space in the rail — right-click,
 * or press and hold on a phone.
 */
export function SpaceDialogs() {
  const dialog = useStore((s) => s.spaceDialog);
  const sp = useStore((s) => s.spaces.find((x) => x.id === dialog?.spaceId));
  // Gone from under us — left elsewhere, or the rail refreshed it away.
  if (!dialog || !sp) return null;

  switch (dialog.kind) {
    case "leave":
      return <LeaveSpace spaceId={sp.id} />;
    case "invite":
      return <InviteToSpace spaceId={sp.id} name={sp.name} />;
    case "browse":
      return <BrowseRooms spaceId={sp.id} name={sp.name} />;
  }
}

/**
 * Leaving a space, and optionally the rooms you joined through it.
 *
 * A space is only a folder of rooms, so leaving it alone takes nothing away —
 * the rooms stay in "everything". Taking them too is a choice, off by default,
 * and only ever covers rooms no other space of yours holds.
 */
function LeaveSpace({ spaceId }: { spaceId: string }) {
  const close = useStore((s) => s.closeSpaceDialog);
  const sp = useStore((s) => s.spaces.find((x) => x.id === spaceId));
  const rooms = useStore((s) => s.rooms);
  const spaces = useStore((s) => s.spaces);

  const [withRooms, setWithRooms] = useState(false);
  const [leaving, setLeaving] = useState(false);

  const ownRooms = useMemo(
    () => roomsOnlyInSpace(rooms, spaces, spaceId),
    [rooms, spaces, spaceId],
  );

  if (!sp) return null;

  async function leave() {
    if (leaving || !sp) return;
    setLeaving(true);
    const { activeRoomId, activeSpaceId, selectRoom, setActiveSpace, refreshSpaces, showBanner } =
      useStore.getState();
    const roomIds = withRooms ? ownRooms.map((r) => r.id) : [];

    try {
      await ipc.leaveRoom(spaceId);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
      setLeaving(false);
      return;
    }

    // The space is gone either way now; a room that won't let go is reported,
    // not a reason to undo anything.
    const results = await Promise.allSettled(roomIds.map((id) => ipc.leaveRoom(id)));
    const stuck = results.filter((r) => r.status === "rejected").length;

    if (activeSpaceId === spaceId) setActiveSpace(null);
    if (activeRoomId && roomIds.includes(activeRoomId)) void selectRoom(null);
    void refreshSpaces();
    close();

    if (stuck > 0) {
      showBanner(
        "error",
        `left ${sp.name}, but ${stuck === 1 ? "one room" : `${stuck} rooms`} wouldn't let go — try leaving ${stuck === 1 ? "it" : "them"} from room settings`,
      );
    }
  }

  return (
    <Modal title={`leave ${sp.name}?`} onClose={close}>
      <div
        style={{
          fontSize: textSize(13),
          color: "var(--text-secondary)",
          lineHeight: 1.55,
          marginBottom: 16,
        }}
      >
        it drops out of your rail. rooms you're in stay in "everything" unless you take
        them with you — and if the space is private, you'd need an invite to get back.
      </div>

      {ownRooms.length > 0 && (
        <div style={{ display: "flex", alignItems: "flex-start", gap: space(12), marginBottom: 6 }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div
              style={{
                fontFamily: "var(--font-heading)",
                fontWeight: 700,
                fontSize: headingSize(13.5),
              }}
            >
              leave its rooms too
            </div>
            <div
              style={{
                fontSize: textSize(12),
                color: "var(--text-tertiary)",
                lineHeight: 1.45,
                marginTop: 2,
              }}
            >
              {ownRooms.length === 1
                ? `the 1 room you're in here: ${ownRooms[0].name}`
                : `the ${ownRooms.length} rooms you're in here: ${listNames(ownRooms.map((r) => r.name))}`}
              . rooms that are also in another of your spaces stay.
            </div>
          </div>
          <div style={{ paddingTop: space(2) }}>
            <Toggle on={withRooms} onToggle={setWithRooms} label="leave its rooms too" />
          </div>
        </div>
      )}

      <div style={{ display: "flex", gap: space(8), justifyContent: "flex-end", marginTop: 20 }}>
        <Button variant="ghost" onClick={close}>
          stay
        </Button>
        <Button variant="danger" onClick={() => void leave()} disabled={leaving}>
          {leaving ? <Spinner size={14} color="var(--text-on-accent)" /> : "leave"}
        </Button>
      </div>
    </Modal>
  );
}

/** "a, b, c and 4 more" — enough to recognise, short enough to read. */
function listNames(names: string[], shown = 3): string {
  if (names.length <= shown) return names.join(", ");
  return `${names.slice(0, shown).join(", ")} and ${names.length - shown} more`;
}

/**
 * Invite someone to a space by their Matrix ID.
 *
 * Only the space: the rooms inside it are theirs to pick once they're in, the
 * same as anyone who finds the space in the directory.
 */
function InviteToSpace({ spaceId, name }: { spaceId: string; name: string }) {
  const close = useStore((s) => s.closeSpaceDialog);
  const showBanner = useStore((s) => s.showBanner);
  const [userId, setUserId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const trimmed = userId.trim();
  // `@name:server` — loose on purpose; the server has the final say.
  const plausible = /^@[^:\s]+:\S+$/.test(trimmed);

  async function invite() {
    if (!plausible || busy) return;
    setBusy(true);
    setError(null);
    try {
      await ipc.inviteUser(spaceId, trimmed);
      close();
      showBanner("info", `invited ${trimmed} to ${name}`);
    } catch (e) {
      setError(ipc.asUwuError(e).message);
      setBusy(false);
    }
  }

  return (
    <Modal title={`invite to ${name}`} onClose={close}>
      <RaveLabel style={{ marginBottom: 6 }}>who</RaveLabel>
      <input
        className="selectable"
        value={userId}
        autoFocus
        placeholder="@someone:server.tld"
        onChange={(e) => setUserId(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void invite();
        }}
        style={{
          width: "100%",
          padding: space("10px 13px"),
          borderRadius: corner(14),
          background: "var(--surface-inset)",
          border: "1px solid var(--border-subtle)",
          outline: "none",
          color: "var(--text-primary)",
          fontSize: textSize(14),
        }}
      />
      <div
        style={{
          fontSize: textSize(12),
          color: error ? "var(--status-danger)" : "var(--text-tertiary)",
          lineHeight: 1.45,
          marginTop: 8,
        }}
      >
        {error ?? "they get an invite to the space, and pick which rooms to join once they're in."}
      </div>
      <div style={{ display: "flex", justifyContent: "flex-end", marginTop: 20 }}>
        <Button onClick={() => void invite()} disabled={!plausible || busy}>
          {busy ? <Spinner size={14} color="var(--text-on-accent)" /> : "invite"}
        </Button>
      </div>
    </Modal>
  );
}

/**
 * Every room in a space, joined or not — Discord's "show all channels".
 *
 * The room list only shows rooms you're in; this is where the rest are.
 */
function BrowseRooms({ spaceId, name }: { spaceId: string; name: string }) {
  const close = useStore((s) => s.closeSpaceDialog);
  return (
    <Modal title={`rooms in ${name}`} onClose={close} width={460}>
      <div style={{ fontSize: textSize(12.5), color: "var(--text-tertiary)", marginBottom: 12 }}>
        everything in the space, including rooms you haven't joined yet.
      </div>
      <SpaceRooms spaceId={spaceId} via={[]} onOpen={close} />
    </Modal>
  );
}
