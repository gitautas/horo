import { useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import type { NotifyMode } from "../lib/bindings";
import { accentFor, initialsFor } from "../lib/display";
import { haptic } from "../lib/haptics";
import * as ipc from "../lib/ipc";
import { mediaUrl } from "../lib/ipc";
import { useIsMobile, useLongPress } from "../lib/viewport";
import { roomsInSpace, useStore } from "../store";
import { ContextMenu, type MenuEntry } from "./ContextMenu";
import { AvatarButton } from "./ProfileCard";
import { dragRegion, Icon, Logo, PresenceDot } from "./ui";
import { corner, space, textSize } from "../lib/scale";

/**
 * The narrow left rail: the uwu mark, one tile per space, and your own avatar
 * pinned to the bottom.
 *
 * `asDrawer` is set only by the mobile shell, where the rail slides over the
 * room list instead of standing beside it permanently. See `ChatPane` for why
 * the mobile props double as the layout switch.
 */
export function SpacesRail({
  asDrawer,
  onClose,
}: { asDrawer?: boolean; onClose?: () => void } = {}) {
  const {
    spaces,
    activeSpaceId,
    session,
    setActiveSpace,
    openSettings,
    openExplore,
  } = useStore(
    useShallow((s) => ({
      spaces: s.spaces,
      activeSpaceId: s.activeSpaceId,
      session: s.session,
      setActiveSpace: s.setActiveSpace,
      openSettings: s.openSettings,
      openExplore: s.openExplore,
    })),
  );

  // Choosing a space is the drawer's whole purpose, so it dismisses on the way
  // out. On desktop `onClose` is undefined and this is just `setActiveSpace`.
  function chooseSpace(id: string | null) {
    setActiveSpace(id);
    onClose?.();
  }

  const rail = (
    <div
      // Only meaningful as a drawer, where the scrim behind it closes on click.
      onClick={asDrawer ? (e) => e.stopPropagation() : undefined}
      style={{
        position: "relative",
        zIndex: 1,
        width: 76,
        flex: "none",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: space(14),
        // The window has no title bar, so the top of this rail sits under the
        // macOS traffic lights; leave them room. As a drawer there are no
        // traffic lights, but there is a notch.
        padding: asDrawer
          ? "calc(var(--safe-top) + 16px) 0 calc(var(--safe-bottom) + 16px)"
          : "44px 0 16px",
        height: asDrawer ? "100%" : undefined,
        borderRight: "1px solid var(--border-subtle)",
        background: "var(--ink-900)",
      }}
    >
      <div
        {...dragRegion(!asDrawer)}
        style={{ position: "absolute", top: 0, left: 0, right: 0, height: 40 }}
      />

      <Logo height={18} />
      <div
        style={{
          width: 26,
          height: 2,
          background: "var(--border-default)",
          borderRadius: 999,
        }}
      />

      <SpaceTile
        label="all"
        name="everything"
        active={activeSpaceId === null}
        accent="var(--accent-primary)"
        onClick={() => chooseSpace(null)}
      />

      <div
        className="uwu-scroll"
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: space(12),
          flex: 1,
          minHeight: 0,
          width: "100%",
          scrollbarWidth: "none",
        }}
      >
        {spaces.map((space) => (
          <SpaceTile
            key={space.id}
            label={initialsFor(space.name)}
            name={space.name}
            mxc={space.avatarUrl}
            badge={space.notificationCount}
            active={activeSpaceId === space.id}
            accent={accentFor(space.id)}
            onClick={() => chooseSpace(space.id)}
            spaceId={space.id}
          />
        ))}

        <button
          onClick={() => {
            openExplore();
            onClose?.();
          }}
          title="explore spaces"
          style={{
            width: 46,
            height: 46,
            flex: "none",
            borderRadius: corner(16),
            cursor: "pointer",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            background: "transparent",
            border: "1px dashed var(--border-default)",
          }}
        >
          <Icon name="compass" size={20} color="var(--text-tertiary)" />
        </button>
      </div>

      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: space(12),
        }}
      >
        <button
          onClick={openSettings}
          title="settings"
          style={{ cursor: "pointer", display: "flex" }}
        >
          <Icon name="gear" size={20} color="var(--text-tertiary)" />
        </button>
        <div style={{ position: "relative" }}>
          {session && (
            <AvatarButton
              userId={session.userId}
              name={session.displayName ?? session.userId}
              mxc={session.avatarUrl}
              size={38}
              radius={14}
            />
          )}
          <PresenceDot />
        </div>
      </div>
    </div>
  );

  if (!asDrawer) return rail;

  // Tapping anywhere off the rail dismisses it — the standard way out of a
  // drawer, and the only one that doesn't need a visible close button eating
  // space in a 76px column.
  return (
    <div
      onClick={onClose}
      role="presentation"
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 120,
        display: "flex",
        background: "rgba(0,0,0,.5)",
      }}
    >
      {rail}
    </div>
  );
}

function SpaceTile({
  label,
  name,
  mxc,
  badge,
  active,
  accent,
  onClick,
  spaceId,
}: {
  label: string;
  name: string;
  mxc?: string | null;
  badge?: number;
  active: boolean;
  accent: string;
  onClick: () => void;
  /** The space this tile stands for, which gives it a context menu. */
  spaceId?: string;
}) {
  const src = mediaUrl(mxc, { width: 46, height: 46 });
  const isMobile = useIsMobile();
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const tile = useRef<HTMLButtonElement>(null);
  // A long press ends in a touchend, which the browser follows with a click —
  // and a click on a tile selects the space and shuts the drawer the menu
  // just opened in. Swallow that one click.
  const pressed = useRef(false);

  const longPress = useLongPress(
    isMobile && spaceId
      ? () => {
          haptic("medium");
          pressed.current = true;
          const r = tile.current?.getBoundingClientRect();
          if (r) setMenuAt({ x: r.right + 8, y: r.top });
        }
      : undefined,
  );

  return (
    <div style={{ position: "relative", flex: "none" }}>
      <button
        ref={tile}
        {...longPress}
        onContextMenu={
          spaceId
            ? (e) => {
                e.preventDefault();
                // On a phone the long press already opened it; Android fires
                // contextmenu for the same hold.
                if (!pressed.current) setMenuAt({ x: e.clientX, y: e.clientY });
              }
            : undefined
        }
        onClick={() => {
          if (pressed.current) {
            pressed.current = false;
            return;
          }
          onClick();
        }}
        title={name}
        style={{
          width: 46,
          height: 46,
          borderRadius: corner(16),
          cursor: "pointer",
          fontFamily: "var(--font-rave)",
          fontSize: textSize(13),
          fontWeight: 800,
          letterSpacing: "0.02em",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          overflow: "hidden",
          padding: 0,
          transition: "all var(--dur-fast) var(--ease-bounce)",
          ...(active
            ? {
                background: src ? "transparent" : accent,
                color: "var(--text-on-accent)",
                border: "2px solid var(--ink-950)",
                boxShadow: "var(--shadow-sticker-ink)",
                transform: "rotate(calc(-3deg * var(--tilt-scale)))",
              }
            : {
                background: "var(--surface-card-raised)",
                color: "var(--text-secondary)",
                border: "1px solid var(--border-subtle)",
              }),
        }}
        onMouseEnter={(e) => {
          if (!active) e.currentTarget.style.transform = "translateY(-2px) rotate(calc(-3deg * var(--tilt-scale)))";
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.transform = active ? "rotate(calc(-3deg * var(--tilt-scale)))" : "";
        }}
      >
        {src ? (
          <img
            src={src}
            alt=""
            style={{ width: "100%", height: "100%", objectFit: "cover" }}
            draggable={false}
          />
        ) : (
          label.toUpperCase()
        )}
      </button>

      {!!badge && badge > 0 && (
        <span
          style={{
            position: "absolute",
            top: -4,
            right: -4,
            minWidth: 18,
            height: 18,
            padding: space("0 5px"),
            borderRadius: 999,
            background: "var(--accent-secondary)",
            color: "var(--text-on-accent)",
            border: "2px solid var(--ink-900)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontFamily: "var(--font-rave)",
            fontSize: textSize(9),
            fontWeight: 800,
          }}
        >
          {badge > 99 ? "99+" : badge}
        </span>
      )}

      {menuAt && spaceId && (
        <SpaceMenu
          spaceId={spaceId}
          at={menuAt}
          onClose={() => {
            pressed.current = false;
            setMenuAt(null);
          }}
        />
      )}
    </div>
  );
}

const NOTIFY_LABELS: Record<NotifyMode, string> = {
  all: "all messages",
  mentions: "only @mentions",
  mute: "nothing",
};

/**
 * A space's context menu, after Discord's server menu.
 *
 * Matrix keeps notification settings per room, not per space, so the space-wide
 * entries apply to every room in it you've joined, and read their current
 * state back from those rooms: a level shows when they all agree. A room you
 * join later starts on your default, as it would anywhere.
 */
function SpaceMenu({
  spaceId,
  at,
  onClose,
}: {
  spaceId: string;
  at: { x: number; y: number };
  onClose: () => void;
}) {
  const rooms = useStore((s) => s.rooms);
  const spaces = useStore((s) => s.spaces);
  const hideMuted = useStore((s) => s.settings.hideMutedInSpaces.includes(spaceId));
  const { updateSettings, openSpaceDialog, showBanner } = useStore(
    useShallow((s) => ({
      updateSettings: s.updateSettings,
      openSpaceDialog: s.openSpaceDialog,
      showBanner: s.showBanner,
    })),
  );

  const joined = roomsInSpace(rooms, spaces, spaceId);
  const ids = joined.map((r) => r.id);
  const anyUnread = joined.some((r) => r.hasUnread);
  const allMuted = joined.length > 0 && joined.every((r) => r.isMuted);
  // No explicit level is the account default — "all messages" for group rooms.
  const modes = new Set(joined.map((r) => r.notifyMode ?? "all"));
  const mode = modes.size === 1 ? [...modes][0] : null;

  function run(work: Promise<void>) {
    work.catch((e) => showBanner("error", ipc.asUwuError(e).message));
  }

  const items: MenuEntry[] = [
    {
      label: "mark as read",
      disabled: !anyUnread,
      onSelect: () => run(ipc.markRoomsRead(joined.filter((r) => r.hasUnread).map((r) => r.id))),
    },
    { kind: "separator" },
    { label: "invite to space", onSelect: () => openSpaceDialog("invite", spaceId) },
    {
      label: "let members into its rooms",
      hint: "opens invite-only rooms to everyone in the space",
      onSelect: () =>
        run(
          ipc.openSpaceRooms(spaceId).then(({ opened, notAllowed }) => {
            const rooms = (n: number) => (n === 1 ? "1 room" : `${n} rooms`);
            const message =
              opened === 0 && notAllowed === 0
                ? "everyone in the space can already get into its rooms"
                : opened === 0
                  ? `you can't change who gets into ${notAllowed === 1 ? "its invite-only room" : `its ${notAllowed} invite-only rooms`}`
                  : notAllowed === 0
                    ? `opened ${rooms(opened)} to the space`
                    : `opened ${rooms(opened)} to the space; ${rooms(notAllowed)} need someone with more power`;
            showBanner("info", message);
          }),
        ),
    },
    { kind: "separator" },
    {
      kind: "check",
      label: "mute space",
      checked: allMuted,
      onToggle: (next) => run(ipc.setRoomsNotifyMode(ids, next ? "mute" : null)),
    },
    {
      kind: "submenu",
      label: "notification settings",
      hint: mode ? NOTIFY_LABELS[mode] : joined.length ? "mixed" : undefined,
      items: (["all", "mentions", "mute"] as const).map((m) => ({
        kind: "radio" as const,
        label: NOTIFY_LABELS[m],
        checked: mode === m,
        onSelect: () => run(ipc.setRoomsNotifyMode(ids, m)),
      })),
    },
    {
      kind: "check",
      label: "hide muted rooms",
      checked: hideMuted,
      onToggle: (next) => {
        const current = useStore.getState().settings.hideMutedInSpaces;
        updateSettings({
          hideMutedInSpaces: next
            ? [...current, spaceId]
            : current.filter((id) => id !== spaceId),
        });
      },
    },
    { label: "browse rooms", onSelect: () => openSpaceDialog("browse", spaceId) },
    { kind: "separator" },
    {
      label: "leave space",
      danger: true,
      onSelect: () => openSpaceDialog("leave", spaceId),
    },
    { kind: "separator" },
    {
      label: "copy space id",
      icon: "identification-card",
      onSelect: () =>
        run(
          navigator.clipboard
            .writeText(spaceId)
            .then(() => showBanner("info", "space id copied")),
        ),
    },
  ];

  return <ContextMenu at={at} items={items} onClose={onClose} />;
}
