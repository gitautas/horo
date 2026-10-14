import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { displayNameFor } from "../lib/display";
import * as ipc from "../lib/ipc";
import { cachedProfile, loadProfile } from "../lib/profiles";
import type { Profile, SharedRoom, UserContext } from "../lib/types";
import { useStore } from "../store";
import { ProfileHeader } from "./ProfileHeader";
import { Avatar, Icon, RaveLabel, Spinner, Tag } from "./ui";
import { corner, space, textSize } from "../lib/scale";

const WIDTH = 320;
/** How far the card sits from the avatar that opened it, on every side. */
const GAP = 12;

/**
 * The popout behind every avatar in the app — cover, name, status, bio, and the
 * rooms you're both in.
 *
 * Two sources fill it: `get_profile` is the homeserver's half (display name,
 * avatar, the MSC4133 fields), `get_user_context` is ours (verification, shared
 * rooms, whether a DM already exists). They arrive separately and the card
 * draws whatever it has, so a slow profile fetch doesn't hold up the rest.
 *
 * Mounted once, from the shell; which user it shows lives in the store.
 */
export function ProfileCard() {
  const card = useStore((s) => s.profileCard);
  if (!card) return null;
  // Keyed so switching between two people rebuilds rather than showing one
  // person's bio under another's name while the fetch is in flight.
  return <Card key={card.userId} userId={card.userId} anchor={card.anchor} />;
}

/**
 * Props that turn any element into something that opens a person's card.
 *
 * The marker attribute matters: the open card closes itself on a click
 * anywhere outside, which would otherwise fire *before* this handler and turn a
 * second click on the same avatar into close-then-reopen instead of close.
 */
export function useProfileAnchor(userId: string) {
  const toggleProfile = useStore((s) => s.toggleProfile);

  return {
    "data-profile-anchor": "",
    onClick: (e: React.MouseEvent<HTMLElement>) =>
      toggleProfile(userId, e.currentTarget.getBoundingClientRect()),
  };
}

/**
 * An avatar that opens its owner's card. Every avatar of a *person* should be
 * one of these — in the timeline, the member list, the rail.
 */
export function AvatarButton({
  userId,
  name,
  mxc,
  size = 38,
  radius,
}: {
  userId: string;
  name: string;
  mxc?: string | null;
  size?: number;
  radius?: number;
}) {
  const anchor = useProfileAnchor(userId);

  return (
    <button
      {...anchor}
      aria-label={`${name}'s profile`}
      style={{
        display: "flex",
        padding: 0,
        cursor: "pointer",
        borderRadius: corner(radius ?? Math.round(size * 0.36)),
        transition: "filter var(--dur-fast) var(--ease-bounce)",
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.filter = "brightness(1.15)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.filter = "";
      }}
    >
      <Avatar id={userId} name={name} mxc={mxc} size={size} radius={radius} />
    </button>
  );
}

function Card({ userId, anchor }: { userId: string; anchor: DOMRect }) {
  const [profile, setProfile] = useState<Profile | null>(
    () => cachedProfile(userId) ?? null,
  );
  const [context, setContext] = useState<UserContext | null>(null);
  const [opening, setOpening] = useState(false);

  const closeProfile = useStore((s) => s.closeProfile);
  const openSettings = useStore((s) => s.openSettings);
  const selectRoom = useStore((s) => s.selectRoom);
  const showBanner = useStore((s) => s.showBanner);

  const card = useRef<HTMLDivElement>(null);
  const position = usePosition(anchor, card);

  useEffect(() => {
    let cancelled = false;

    loadProfile(userId)
      .then((p) => !cancelled && setProfile(p))
      .catch(() => {});
    ipc
      .getUserContext(userId)
      .then((c) => !cancelled && setContext(c))
      .catch(() => {});

    return () => {
      cancelled = true;
    };
  }, [userId]);

  // Click-away and Escape, the two ways out of any popover.
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      if (card.current?.contains(target)) return;
      // Avatars close the card through their own toggle, not through this.
      if (target.closest?.("[data-profile-anchor]")) return;
      closeProfile();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeProfile();
    };
    // Deferred: the click that opened this would otherwise close it again.
    const timer = window.setTimeout(() => {
      window.addEventListener("mousedown", onDown);
    }, 0);
    window.addEventListener("keydown", onKey);
    // The anchor is a snapshot of where the avatar was; a resize moves it and
    // there's nothing sensible left to point at.
    window.addEventListener("resize", closeProfile);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", closeProfile);
    };
  }, [closeProfile]);

  const name = displayNameFor(userId, profile?.displayName ?? null);
  const verified = context?.verification === "verified";
  const isMe = context?.isMe ?? false;
  const shared = context?.sharedRooms ?? [];

  function jumpTo(roomId: string) {
    closeProfile();
    void selectRoom(roomId);
  }

  async function message() {
    setOpening(true);
    try {
      const roomId = await ipc.openDm(userId);
      jumpTo(roomId);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    } finally {
      setOpening(false);
    }
  }

  return createPortal(
    <div
      ref={card}
      className="uwu-scroll"
      style={{
        position: "fixed",
        left: position.left,
        top: position.top,
        // Hidden for the one frame between mount and measuring, so the card
        // never appears in the wrong place and then hops.
        visibility: position.measured ? "visible" : "hidden",
        zIndex: 130,
        width: WIDTH,
        maxHeight: `calc(100vh - ${GAP * 2}px)`,
        overflowY: "auto",
        borderRadius: corner(20),
        background: "var(--surface-card-raised)",
        border: "1px solid var(--border-default)",
        boxShadow: "var(--shadow-pop)",
      }}
    >
      <ProfileHeader
        userId={userId}
        profile={profile}
        name={name}
        width={WIDTH}
        badge={verified ? <Tag icon="seal-check">verified</Tag> : null}
      />

      <div style={{ padding: space("0 16px 16px") }}>
        {!profile && !context && (
          <div style={{ display: "flex", justifyContent: "center", padding: space(16) }}>
            <Spinner size={14} />
          </div>
        )}

        {profile?.bio && (
          <>
            <Divider />
            <RaveLabel style={{ marginBottom: 6 }}>about</RaveLabel>
            <div
              className="selectable"
              style={{
                fontSize: textSize(13),
                lineHeight: 1.55,
                color: "var(--text-secondary)",
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
            >
              {profile.bio}
            </div>
          </>
        )}

        {shared.length > 0 && (
          <>
            <Divider />
            <SharedRooms rooms={shared} onPick={jumpTo} />
          </>
        )}

        {/* Held back until the context lands, so nobody sees "message" on
            their own card for a frame. */}
        <div style={{ display: "flex", gap: space(8), marginTop: 14 }}>
          {!context ? null : isMe ? (
            <CardButton
              icon="pencil-simple"
              label="edit your profile"
              onClick={() => {
                closeProfile();
                openSettings();
              }}
            />
          ) : (
            <>
              <CardButton
                icon={opening ? "hourglass" : "chat-teardrop-dots"}
                label={context?.dmRoomId ? "open dm" : "message"}
                primary
                disabled={opening}
                onClick={() => void message()}
              />
              {!verified && (
                <CardButton
                  icon="seal-question"
                  label="verify"
                  onClick={() => {
                    closeProfile();
                    void ipc
                      .requestVerification(userId)
                      .catch((e) => showBanner("error", ipc.asUwuError(e).message));
                  }}
                />
              )}
            </>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}

/**
 * Hang the card off the anchor: to its right if there's room, flipped to its
 * left if not, and never off the top or bottom of the window.
 *
 * The height isn't known at mount and doesn't stay still — the profile and the
 * shared rooms land separately, and a long bio makes a much taller card — so
 * this watches the element rather than measuring once.
 */
function usePosition(anchor: DOMRect, card: React.RefObject<HTMLDivElement | null>) {
  const [height, setHeight] = useState(0);

  useLayoutEffect(() => {
    const element = card.current;
    if (!element) return;

    setHeight(element.offsetHeight);
    const observer = new ResizeObserver(() => setHeight(element.offsetHeight));
    observer.observe(element);
    return () => observer.disconnect();
  }, [card]);

  const spaceRight = window.innerWidth - anchor.right - GAP;
  const left =
    spaceRight >= WIDTH + GAP
      ? anchor.right + GAP
      : Math.max(GAP, anchor.left - WIDTH - GAP);

  // Prefer the card's top level with the avatar's, then push it back inside the
  // window rather than letting it run off the bottom.
  const top = Math.max(GAP, Math.min(anchor.top - 8, window.innerHeight - height - GAP));

  return { left, top, measured: height > 0 };
}

function Divider() {
  return (
    <div
      style={{
        height: 1,
        margin: "13px 0 11px",
        background: "var(--border-subtle)",
      }}
    />
  );
}

/** The rooms you're both in — Discord's "mutual servers", one row of tiles. */
function SharedRooms({
  rooms,
  onPick,
}: {
  rooms: SharedRoom[];
  onPick: (roomId: string) => void;
}) {
  const shown = rooms.slice(0, 7);
  const rest = rooms.length - shown.length;

  return (
    <>
      <RaveLabel style={{ marginBottom: 8 }}>
        shared with you · {rooms.length}
      </RaveLabel>
      <div style={{ display: "flex", flexWrap: "wrap", gap: space(6) }}>
        {shown.map((room) => (
          <button
            key={room.id}
            onClick={() => onPick(room.id)}
            title={room.name}
            style={{ cursor: "pointer", display: "flex", padding: 0 }}
          >
            <Avatar
              id={room.id}
              name={room.name}
              mxc={room.avatarUrl}
              size={32}
              radius={11}
            />
          </button>
        ))}
        {rest > 0 && (
          <div
            style={{
              width: 32,
              height: 32,
              borderRadius: corner(11),
              border: "1px solid var(--border-subtle)",
              background: "var(--surface-card)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontFamily: "var(--font-mono)",
              fontSize: textSize(10.5),
              color: "var(--text-tertiary)",
            }}
            title={rooms
              .slice(shown.length)
              .map((room) => room.name)
              .join(", ")}
          >
            +{rest}
          </div>
        )}
      </div>
    </>
  );
}

function CardButton({
  icon,
  label,
  onClick,
  primary,
  disabled,
}: {
  icon: string;
  label: string;
  onClick: () => void;
  primary?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      style={{
        flex: 1,
        padding: space("9px 0"),
        borderRadius: 999,
        cursor: disabled ? "default" : "pointer",
        opacity: disabled ? 0.55 : 1,
        fontFamily: "var(--font-display)",
        fontWeight: 700,
        fontSize: textSize(13),
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: space(7),
        transition: "transform var(--dur-fast) var(--ease-bounce)",
        ...(primary
          ? {
              background: "var(--accent-primary)",
              color: "var(--text-on-accent)",
              border: "2px solid var(--ink-950)",
              boxShadow: "var(--shadow-sticker-ink)",
            }
          : {
              background: "var(--surface-card)",
              color: "var(--text-secondary)",
              border: "1px solid var(--border-default)",
            }),
      }}
      onMouseDown={(e) => {
        if (!disabled) e.currentTarget.style.transform = "translateY(1px)";
      }}
      onMouseUp={(e) => {
        e.currentTarget.style.transform = "";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.transform = "";
      }}
    >
      <Icon
        name={icon}
        size={13}
        color={primary ? "var(--text-on-accent)" : "var(--text-secondary)"}
      />
      {label}
    </button>
  );
}
