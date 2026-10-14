/**
 * The green dot, and the "last seen" line under a name.
 *
 * Both take a user ID and do their own subscribing, so anywhere that draws a
 * person can add one without threading presence through its props. Both draw
 * *nothing* when there's no answer — a homeserver with presence switched off,
 * or the moment before the first poll lands — because a grey "offline" dot for
 * everyone on the server is a confident lie, and the absence of a dot isn't.
 */

import { useEffect, useState, type CSSProperties } from "react";

import { formatLastSeen } from "../lib/display";
import { usePresence, usePresenceSupported } from "../lib/presence";
import type { Availability, Presence } from "../lib/types";
import { space, textSize } from "../lib/scale";

/** What gets a dot. `unknown` is a server answer we can't read, and gets none. */
type Drawable = Exclude<Availability, "unknown">;

const COLOUR: Record<Drawable, string> = {
  online: "var(--status-online)",
  unavailable: "var(--status-idle)",
  offline: "var(--status-offline)",
};

const WORD: Record<Drawable, string> = {
  online: "online",
  unavailable: "away",
  offline: "offline",
};

/** `undefined` for anything we shouldn't draw — see the file comment. */
function drawable(
  presence: Presence | undefined,
  supported: boolean,
): Drawable | undefined {
  if (!supported || !presence) return undefined;
  return presence.presence === "unknown" ? undefined : presence.presence;
}

/**
 * The status dot, cut into the bottom-right corner of an avatar.
 *
 * Expects a `position: relative` parent — same arrangement as the channel badge
 * and the call badge it sits alongside.
 */
export function PresenceDot({
  userId,
  size = 12,
  ring = "var(--ink-900)",
}: {
  userId: string;
  size?: number;
  /** The colour it's cut out of, matching whatever sits behind the avatar. */
  ring?: string;
}) {
  const presence = usePresence(userId);
  const supported = usePresenceSupported();
  const state = drawable(presence, supported);
  if (!state) return null;

  return (
    <div
      title={presenceTitle(presence)}
      style={{
        position: "absolute",
        right: -2,
        bottom: -2,
        width: size,
        height: size,
        borderRadius: "50%",
        background: COLOUR[state],
        border: `2px solid ${ring}`,
        // Offline sits back a little, and online glows: the two live states
        // should catch the eye first in a list where most people aren't.
        opacity: state === "offline" ? 0.9 : 1,
        boxShadow: state === "online" ? "0 0 6px color-mix(in srgb, var(--green-500) 50%, transparent)" : undefined,
      }}
    />
  );
}

/**
 * `● online` / `● away · last seen 12m ago`, for a profile card or the top of
 * a DM's sidebar.
 *
 * Re-renders itself on a slow timer: the label is relative to now, and the
 * backend only pushes when the *server's* answer changes — so without this, "2m
 * ago" would sit there saying 2m an hour later.
 */
export function PresenceLine({
  userId,
  style,
}: {
  userId: string;
  style?: CSSProperties;
}) {
  const presence = usePresence(userId);
  const supported = usePresenceSupported();
  useTicker(60_000);

  const state = drawable(presence, supported);
  if (!state || !presence) return null;

  const seen =
    state === "online" ? null : formatLastSeen(presence.lastActive);

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(6),
        marginTop: 7,
        fontFamily: "var(--font-mono)",
        fontSize: textSize(10.5),
        color: "var(--text-tertiary)",
        ...style,
      }}
    >
      <span
        style={{
          width: 7,
          height: 7,
          borderRadius: "50%",
          flex: "none",
          background: COLOUR[state],
        }}
      />
      <span>{WORD[state]}</span>
      {seen && (
        <>
          <span>·</span>
          <span>last seen {seen}</span>
        </>
      )}
      {presence.statusMsg && (
        <span className="uwu-ellipsis" title={presence.statusMsg}>
          · {presence.statusMsg}
        </span>
      )}
    </div>
  );
}

/** What hovering a dot says, spelled out. */
export function presenceTitle(presence: Presence | undefined): string | undefined {
  if (!presence || presence.presence === "unknown") return undefined;
  if (presence.presence === "online") return "online";

  const seen = formatLastSeen(presence.lastActive);
  const word = WORD[presence.presence];
  return seen ? `${word} · last seen ${seen}` : word;
}

/** Re-render on an interval, for labels that are relative to the clock. */
function useTicker(everyMs: number): void {
  const [, setTick] = useState(0);
  useEffect(() => {
    const timer = window.setInterval(() => setTick((t) => t + 1), everyMs);
    return () => window.clearInterval(timer);
  }, [everyMs]);
}
