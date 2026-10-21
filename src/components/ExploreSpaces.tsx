import { useEffect, useRef, useState } from "react";

import type { PublicRoom } from "../lib/bindings";
import * as ipc from "../lib/ipc";
import { useStore } from "../store";
import { waitForRoom } from "./CreateRoom";
import { Avatar, Button, HoverRow, Icon, Modal, RaveLabel, Spinner, Tag } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

/** How long typing has to pause before the directory is asked again. */
const SEARCH_DEBOUNCE_MS = 350;

/**
 * Browsing public spaces: a server's directory, searchable, with a look at the
 * rooms inside each space before you commit to joining it.
 *
 * Mounted from the shell; opened from the compass in the spaces rail.
 */
export function ExploreSpaces() {
  const open = useStore((s) => s.showExplore);
  if (!open) return null;
  return <Dialog />;
}

function Dialog() {
  const close = useStore((s) => s.closeExplore);
  const ownServer = useStore((s) => s.session?.userId.split(":").slice(1).join(":") ?? "");

  const [search, setSearch] = useState("");
  const [server, setServer] = useState("");
  // What the list is actually showing, so typing doesn't refetch per keystroke.
  const [query, setQuery] = useState({ search: "", server: "" });

  const [spaces, setSpaces] = useState<PublicRoom[]>([]);
  const [nextBatch, setNextBatch] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);

  // Every fetch takes a ticket; a response whose ticket is stale belongs to a
  // query the user has already moved on from, and is dropped.
  const ticket = useRef(0);

  useEffect(() => {
    const next = { search: search.trim(), server: server.trim() };
    const t = setTimeout(
      // Same values, same object: a new one would refetch what's on screen.
      () =>
        setQuery((cur) =>
          cur.search === next.search && cur.server === next.server ? cur : next,
        ),
      SEARCH_DEBOUNCE_MS,
    );
    return () => clearTimeout(t);
  }, [search, server]);

  async function load(since?: string) {
    const mine = ++ticket.current;
    setLoading(true);
    setError(null);
    try {
      const page = await ipc.getPublicSpaces({
        server: query.server || undefined,
        search: query.search || undefined,
        since,
      });
      if (mine !== ticket.current) return;
      setSpaces((prev) => (since ? [...prev, ...page.rooms] : page.rooms));
      setNextBatch(page.nextBatch);
    } catch (e) {
      if (mine !== ticket.current) return;
      if (!since) setSpaces([]);
      setNextBatch(null);
      setError(ipc.asUwuError(e).message);
    } finally {
      if (mine === ticket.current) setLoading(false);
    }
  }

  useEffect(() => {
    setExpanded(null);
    void load();
    // `load` reads `query` and nothing else that changes.
  }, [query]);

  return (
    <Modal title="explore spaces" onClose={close} width={560}>
      <div style={{ display: "flex", gap: space(8), marginBottom: 16, flexWrap: "wrap" }}>
        <div style={{ flex: "2 1 220px", minWidth: 0 }}>
          <RaveLabel style={{ marginBottom: 6 }}>search</RaveLabel>
          <TextInput
            value={search}
            onChange={setSearch}
            placeholder="name or topic"
            icon="magnifying-glass"
            autoFocus
          />
        </div>
        <div style={{ flex: "1 1 150px", minWidth: 0 }}>
          <RaveLabel style={{ marginBottom: 6 }}>server</RaveLabel>
          <TextInput value={server} onChange={setServer} placeholder={ownServer || "yours"} />
        </div>
      </div>

      {/* A floor under the results, so the dialog doesn't shrink and jump
          under the pointer while a search narrows them down. */}
      <div style={{ minHeight: 320 }}>
        {error && (
          <div
            style={{
              padding: space("10px 13px"),
              marginBottom: 12,
              borderRadius: corner(14),
              background: "color-mix(in srgb, var(--status-danger) 12%, transparent)",
              color: "var(--text-secondary)",
              fontSize: textSize(12.5),
              lineHeight: 1.45,
            }}
          >
            {query.server
              ? `couldn't read ${query.server}'s directory — ${error}`
              : `couldn't load the directory — ${error}`}
          </div>
        )}

        {!error && !loading && spaces.length === 0 && (
          <div
            style={{
              padding: space("28px 8px"),
              textAlign: "center",
              color: "var(--text-tertiary)",
              fontSize: textSize(13),
              lineHeight: 1.5,
            }}
          >
            {query.search
              ? `no public spaces match "${query.search}"`
              : `${query.server || "your server"} doesn't list any public spaces. try another server~`}
          </div>
        )}

        <div style={{ display: "flex", flexDirection: "column", gap: space(4) }}>
          {spaces.map((sp) => (
            <SpaceRow
              key={sp.id}
              space={sp}
              expanded={expanded === sp.id}
              onToggle={() => setExpanded((cur) => (cur === sp.id ? null : sp.id))}
            />
          ))}
        </div>

        {loading && (
          <div style={{ display: "flex", justifyContent: "center", padding: space(16) }}>
            <Spinner />
          </div>
        )}

        {!loading && nextBatch && (
          <div style={{ display: "flex", justifyContent: "center", marginTop: 12 }}>
            <Button variant="ghost" size="sm" onClick={() => void load(nextBatch)}>
              show more
            </Button>
          </div>
        )}
      </div>
    </Modal>
  );
}

/** One space from the directory, which opens to show the rooms inside it. */
function SpaceRow({
  space: sp,
  expanded,
  onToggle,
}: {
  space: PublicRoom;
  expanded: boolean;
  onToggle: () => void;
}) {
  const joined = useStore((s) => s.spaces.some((x) => x.id === sp.id));
  const close = useStore((s) => s.closeExplore);
  const setActiveSpace = useStore((s) => s.setActiveSpace);
  const refreshSpaces = useStore((s) => s.refreshSpaces);
  const showBanner = useStore((s) => s.showBanner);
  const [busy, setBusy] = useState(false);

  const name = displayName(sp);

  async function join() {
    if (busy) return;
    setBusy(true);
    try {
      await ipc.joinRoom(sp.id, sp.via);
      await waitForSpace(sp.id, refreshSpaces);
      setActiveSpace(sp.id);
      close();
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  function open() {
    setActiveSpace(sp.id);
    close();
  }

  return (
    <div
      style={{
        borderRadius: corner(16),
        border: `1px solid ${expanded ? "var(--border-default)" : "transparent"}`,
        background: expanded ? "var(--surface-inset)" : "transparent",
        overflow: "hidden",
      }}
    >
      <HoverRow
        onClick={onToggle}
        style={{ display: "flex", gap: space(12), padding: space("10px 10px"), alignItems: "flex-start" }}
      >
        <Avatar id={sp.id} name={name} mxc={sp.avatarUrl} size={44} radius={14} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: "flex", alignItems: "center", gap: space(6) }}>
            <span
              style={{
                fontFamily: "var(--font-heading)",
                fontWeight: 800,
                fontSize: headingSize(14.5),
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {name}
            </span>
            <Icon
              name={expanded ? "caret-up" : "caret-down"}
              size={12}
              color="var(--text-tertiary)"
            />
          </div>
          <Meta room={sp} />
          {sp.topic && <Topic text={sp.topic} lines={expanded ? 6 : 2} />}
        </div>
        <div onClick={(e) => e.stopPropagation()} style={{ flex: "none", paddingTop: space(2) }}>
          {joined ? (
            <Button size="sm" variant="ghost" onClick={open}>
              open
            </Button>
          ) : sp.joinRule === "public" ? (
            <>
              <Button size="sm" onClick={() => void join()} disabled={busy}>
                {busy ? <Spinner size={12} color="var(--text-on-accent)" /> : "join"}
              </Button>
              {busy && <SlowJoinNote />}
            </>
          ) : (
            <JoinRuleTag rule={sp.joinRule} />
          )}
        </div>
      </HoverRow>

      {expanded && (
        <div style={{ padding: space("2px 10px 12px 66px") }}>
          <SpaceRooms spaceId={sp.id} via={sp.via} onOpen={close} />
        </div>
      )}
    </div>
  );
}

/**
 * The rooms inside a space, each joinable on its own.
 *
 * Also what "browse rooms" in a space's context menu shows. `onOpen` runs when
 * the user opens a room or space they're in, so the dialog around it can close.
 */
export function SpaceRooms({
  spaceId,
  via,
  onOpen,
}: {
  spaceId: string;
  /** Servers to join through when a child doesn't name its own. */
  via: string[];
  onOpen: () => void;
}) {
  const [rooms, setRooms] = useState<PublicRoom[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    ipc
      .getSpaceRooms(spaceId)
      .then((r) => live && setRooms(r))
      .catch((e) => live && setError(ipc.asUwuError(e).message));
    return () => {
      live = false;
    };
  }, [spaceId]);

  return (
    <div>
      <RaveLabel style={{ marginBottom: 6 }}>inside</RaveLabel>
      {error ? (
        <div style={{ fontSize: textSize(12), color: "var(--text-tertiary)", lineHeight: 1.45 }}>
          couldn't look inside — {error}
        </div>
      ) : rooms === null ? (
        <Spinner size={14} />
      ) : rooms.length === 0 ? (
        <div style={{ fontSize: textSize(12), color: "var(--text-tertiary)" }}>
          nothing public in here yet
        </div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: space(2) }}>
          {rooms.map((room) => (
            <ChildRow key={room.id} room={room} fallbackVia={via} onOpen={onOpen} />
          ))}
        </div>
      )}
    </div>
  );
}

function ChildRow({
  room,
  fallbackVia,
  onOpen,
}: {
  room: PublicRoom;
  fallbackVia: string[];
  onOpen: () => void;
}) {
  const joined = useStore((s) =>
    room.isSpace
      ? s.spaces.some((x) => x.id === room.id)
      : s.rooms.some((r) => r.id === room.id && r.membership === "joined"),
  );
  const selectRoom = useStore((s) => s.selectRoom);
  const setActiveSpace = useStore((s) => s.setActiveSpace);
  const refreshSpaces = useStore((s) => s.refreshSpaces);
  const showBanner = useStore((s) => s.showBanner);
  const [busy, setBusy] = useState(false);

  const name = displayName(room);

  function open() {
    if (room.isSpace) setActiveSpace(room.id);
    else void selectRoom(room.id);
    onOpen();
  }

  async function join() {
    if (busy) return;
    setBusy(true);
    try {
      await ipc.joinRoom(room.id, room.via.length ? room.via : fallbackVia);
      // Stay in the dialog: picking several rooms out of a space is the
      // common case. The row flips to "open" once sync has caught up.
      if (room.isSpace) await waitForSpace(room.id, refreshSpaces);
      else await waitForRoom(room.id);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div style={{ display: "flex", alignItems: "center", gap: space(10), padding: space("6px 0") }}>
      <Avatar id={room.id} name={name} mxc={room.avatarUrl} size={28} radius={9} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: space(5),
            fontWeight: 700,
            fontSize: textSize(13),
          }}
        >
          {room.isSpace && <Icon name="planet" size={12} color="var(--text-tertiary)" />}
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {name}
          </span>
        </div>
        <div style={{ fontSize: textSize(11), color: "var(--text-tertiary)" }}>
          {memberCount(room.members)}
        </div>
      </div>
      {joined ? (
        <Button size="sm" variant="ghost" onClick={open}>
          open
        </Button>
      ) : room.joinRule === "public" ? (
        <Button size="sm" variant="ghost" onClick={() => void join()} disabled={busy}>
          {busy ? <Spinner size={12} /> : "join"}
        </Button>
      ) : (
        <JoinRuleTag rule={room.joinRule} />
      )}
    </div>
  );
}

function Meta({ room }: { room: PublicRoom }) {
  return (
    <div
      style={{
        display: "flex",
        gap: space(8),
        fontSize: textSize(11.5),
        color: "var(--text-tertiary)",
        marginTop: 2,
        minWidth: 0,
      }}
    >
      <span style={{ flex: "none" }}>{memberCount(room.members)}</span>
      {room.alias && (
        <span
          style={{
            fontFamily: "var(--font-mono)",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {room.alias}
        </span>
      )}
    </div>
  );
}

function Topic({ text, lines }: { text: string; lines: number }) {
  return (
    <div
      style={{
        fontSize: textSize(12.5),
        color: "var(--text-secondary)",
        lineHeight: 1.45,
        marginTop: 5,
        display: "-webkit-box",
        WebkitLineClamp: lines,
        WebkitBoxOrient: "vertical",
        overflow: "hidden",
        overflowWrap: "anywhere",
      }}
    >
      {text}
    </div>
  );
}

/**
 * Joining a space on another server makes ours fetch its whole state first,
 * which can take minutes. Without a word, a spinner that long reads as stuck.
 */
function SlowJoinNote() {
  return (
    <div
      style={{
        marginTop: 6,
        maxWidth: 110,
        textAlign: "right",
        fontSize: textSize(10.5),
        lineHeight: 1.35,
        color: "var(--text-tertiary)",
      }}
    >
      big spaces can take a minute~
    </div>
  );
}

/** Why there's no join button: the room exists but isn't open to walk into. */
function JoinRuleTag({ rule }: { rule: string }) {
  const label =
    rule === "knock" || rule === "knock_restricted"
      ? "ask to join"
      : rule === "restricted"
        ? "members only"
        : "invite only";
  return (
    <Tag colour="var(--text-tertiary)" icon="lock-simple">
      {label}
    </Tag>
  );
}

function TextInput({
  value,
  onChange,
  placeholder,
  icon,
  autoFocus,
}: {
  value: string;
  onChange: (next: string) => void;
  placeholder?: string;
  icon?: string;
  autoFocus?: boolean;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(8),
        padding: space("0 13px"),
        borderRadius: corner(14),
        background: "var(--surface-inset)",
        border: "1px solid var(--border-subtle)",
      }}
    >
      {icon && <Icon name={icon} size={14} color="var(--text-tertiary)" />}
      <input
        className="selectable"
        value={value}
        autoFocus={autoFocus}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        style={{
          flex: 1,
          minWidth: 0,
          padding: space("10px 0"),
          background: "transparent",
          border: "none",
          outline: "none",
          color: "var(--text-primary)",
          fontSize: textSize(14),
        }}
      />
    </div>
  );
}

function displayName(room: PublicRoom): string {
  return room.name ?? room.alias ?? room.id;
}

function memberCount(n: number): string {
  return n === 1 ? "1 member" : `${n.toLocaleString()} members`;
}

/**
 * Wait for a just-joined space to show up in the rail, for a few seconds at
 * most.
 *
 * Spaces aren't pushed like rooms are — the list is re-read on a slow poll —
 * so this asks for it directly until the space is there.
 */
async function waitForSpace(
  spaceId: string,
  refreshSpaces: () => Promise<void>,
  timeoutMs = 5000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    await refreshSpaces();
    if (useStore.getState().spaces.some((s) => s.id === spaceId)) return;
    await new Promise((resolve) => setTimeout(resolve, 300));
  }
}
