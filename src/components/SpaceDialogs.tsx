import { useEffect, useMemo, useState } from "react";

import { displayNameFor } from "../lib/display";
import * as ipc from "../lib/ipc";
import type { UserSuggestion } from "../lib/types";
import { roomsOnlyInSpace, useStore } from "../store";
import { SpaceRooms } from "./ExploreSpaces";
import { Avatar, Button, Modal, RaveLabel, Spinner, Toggle } from "./ui";
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
 * Only the space: accepting it joins them to every room the space lets its
 * members into, the way joining a Discord server does.
 *
 * Typing a name suggests people from the homeserver's user directory, so
 * nobody has to remember an exact `@name:server`. A full ID still works for
 * someone the directory doesn't know.
 */
function InviteToSpace({ spaceId, name }: { spaceId: string; name: string }) {
  const close = useStore((s) => s.closeSpaceDialog);
  const showBanner = useStore((s) => s.showBanner);
  const [userId, setUserId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Asked once on open, so someone without the power finds out before typing
  // a name rather than after. Unknown counts as allowed: the server decides.
  const [canInvite, setCanInvite] = useState(true);
  // Who's already in or invited, so a suggestion can say so instead of
  // offering an invite the server will turn down.
  const [members, setMembers] = useState<Map<string, string>>(new Map());

  useEffect(() => {
    let live = true;
    ipc
      .getRoomPermissions(spaceId)
      .then((p) => live && setCanInvite(p.canInvite))
      .catch(() => {});
    ipc
      .getMembers(spaceId)
      .then((list) => live && setMembers(new Map(list.map((m) => [m.userId, m.membership]))))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [spaceId]);

  const trimmed = userId.trim();
  // `@name:server` — loose on purpose; the server has the final say.
  const plausible = /^@[^:\s]+:\S+$/.test(trimmed);

  const { suggestions, clear } = useUserSuggestions(trimmed);
  const [highlight, setHighlight] = useState(0);
  // Start on the first person who can actually be invited, so Enter picks them.
  useEffect(() => {
    setHighlight(Math.max(0, suggestions.findIndex((s) => !members.has(s.userId))));
  }, [suggestions, members]);

  function pick(suggestion: UserSuggestion) {
    if (members.has(suggestion.userId)) return;
    setUserId(suggestion.userId);
    setError(null);
    clear(suggestion.userId);
  }

  async function invite() {
    if (!plausible || busy || !canInvite) return;
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
        placeholder="a name, or @someone:server.tld"
        role="combobox"
        aria-expanded={suggestions.length > 0}
        aria-controls="invite-suggestions"
        onChange={(e) => {
          setUserId(e.target.value);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (suggestions.length > 0 && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
            e.preventDefault();
            const step = e.key === "ArrowDown" ? 1 : -1;
            setHighlight((h) => (h + step + suggestions.length) % suggestions.length);
          } else if (e.key === "Enter") {
            const chosen = suggestions[highlight];
            // A suggestion is picked first; Enter again sends the invite.
            if (chosen && chosen.userId !== trimmed && !members.has(chosen.userId)) pick(chosen);
            else void invite();
          }
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

      {suggestions.length > 0 && (
        <div
          id="invite-suggestions"
          role="listbox"
          style={{
            marginTop: 6,
            padding: space(4),
            borderRadius: corner(14),
            background: "var(--surface-inset)",
            border: "1px solid var(--border-subtle)",
            display: "flex",
            flexDirection: "column",
            gap: space(2),
          }}
        >
          {suggestions.map((suggestion, i) => (
            <SuggestionRow
              key={suggestion.userId}
              suggestion={suggestion}
              membership={members.get(suggestion.userId)}
              highlighted={i === highlight}
              onHover={() => setHighlight(i)}
              onPick={() => pick(suggestion)}
            />
          ))}
        </div>
      )}

      <div
        style={{
          fontSize: textSize(12),
          color: error || !canInvite ? "var(--status-danger)" : "var(--text-tertiary)",
          lineHeight: 1.45,
          marginTop: 8,
        }}
      >
        {error ??
          (canInvite
            ? "they get an invite to the space. joining it puts them in every room its members can get into."
            : `you can't invite people to ${name} — that takes more power than you have there. ask one of its admins.`)}
      </div>
      <div style={{ display: "flex", justifyContent: "flex-end", marginTop: 20 }}>
        <Button onClick={() => void invite()} disabled={!plausible || busy || !canInvite}>
          {busy ? <Spinner size={14} color="var(--text-on-accent)" /> : "invite"}
        </Button>
      </div>
    </Modal>
  );
}

/** How long typing has to pause before the directory is asked. */
const SEARCH_DELAY_MS = 200;

/**
 * Directory matches for what's been typed, after a short pause so every
 * keystroke isn't a request. Stays quiet for a term that's just been picked
 * from the list — the suggestion has done its job.
 */
function useUserSuggestions(term: string) {
  const [suggestions, setSuggestions] = useState<UserSuggestion[]>([]);
  const [picked, setPicked] = useState<string | null>(null);

  useEffect(() => {
    if (term.length < 2 || term === picked) {
      setSuggestions([]);
      return;
    }
    let live = true;
    const timer = window.setTimeout(() => {
      ipc
        .searchUsers(term)
        .then((found) => live && setSuggestions(found))
        // A directory that won't answer just means no suggestions; a full ID
        // can still be typed and invited.
        .catch(() => live && setSuggestions([]));
    }, SEARCH_DELAY_MS);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [term, picked]);

  return {
    suggestions,
    clear: (chosen: string) => {
      setPicked(chosen);
      setSuggestions([]);
    },
  };
}

function SuggestionRow({
  suggestion,
  membership,
  highlighted,
  onHover,
  onPick,
}: {
  suggestion: UserSuggestion;
  /** `join` or `invite` when they're already in the space or on their way. */
  membership?: string;
  highlighted: boolean;
  onHover: () => void;
  onPick: () => void;
}) {
  const label = displayNameFor(suggestion.userId, suggestion.displayName);
  const taken = membership === "join" ? "already in" : membership === "invite" ? "invited" : null;

  return (
    <div
      role="option"
      aria-selected={highlighted}
      aria-disabled={!!taken}
      onMouseEnter={onHover}
      // `mousedown` rather than `click`, so the input keeps focus.
      onMouseDown={(e) => {
        e.preventDefault();
        onPick();
      }}
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(10),
        padding: space("6px 8px"),
        borderRadius: corner(10),
        cursor: taken ? "default" : "pointer",
        opacity: taken ? 0.5 : 1,
        background: highlighted && !taken ? "rgba(255,255,255,.06)" : "transparent",
      }}
    >
      <Avatar id={suggestion.userId} name={label} mxc={suggestion.avatarUrl} size={28} radius={10} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div className="uwu-ellipsis" style={{ fontSize: textSize(13.5), fontWeight: 600 }}>
          {label}
        </div>
        <div
          className="uwu-ellipsis"
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(11),
            color: "var(--text-tertiary)",
          }}
        >
          {suggestion.userId}
        </div>
      </div>
      {taken && (
        <span style={{ fontSize: textSize(11), color: "var(--text-tertiary)", flex: "none" }}>
          {taken}
        </span>
      )}
    </div>
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
