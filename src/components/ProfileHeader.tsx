import type { CSSProperties, ReactNode } from "react";

import { accentFor } from "../lib/display";
import { mediaUrl } from "../lib/ipc";
import type { Profile } from "../lib/types";
import { useStore } from "../store";
import { PresenceLine } from "./Presence";
import { Avatar } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

/**
 * The top of a person: cover, avatar over it, name, handle, status.
 *
 * Shared by the profile card and the sidebar of a DM, which are the same thing
 * seen from two directions — a DM *is* a person, and drawing it as a room with
 * a room avatar and a room ID says nothing you wanted to know.
 *
 * The cover is a `mxc://` we may not have, so the fallback is the same accent
 * the avatar's monogram would use: the card is never a grey box.
 */
export function ProfileHeader({
  userId,
  profile,
  name,
  width,
  coverHeight = 88,
  avatarSize = 66,
  ring = "var(--surface-card-raised)",
  style,
  badge,
}: {
  userId: string;
  /** Null while it's still being fetched — everything degrades to the ID. */
  profile: Profile | null;
  name: string;
  /** What to ask the server for; the cover is drawn edge to edge. */
  width: number;
  coverHeight?: number;
  avatarSize?: number;
  /** The colour the avatar is cut out of, matching whatever sits behind. */
  ring?: string;
  style?: CSSProperties;
  /** Anything to sit opposite the avatar — the verified tag, on a card. */
  badge?: ReactNode;
}) {
  const openLightbox = useStore((s) => s.openLightbox);
  const cover = mediaUrl(profile?.coverUrl, { width, height: coverHeight });

  return (
    <div style={style}>
      <div
        onClick={() =>
          profile?.coverUrl && openLightbox(profile.coverUrl, `${name}'s cover`)
        }
        style={{
          height: coverHeight,
          cursor: profile?.coverUrl ? "zoom-in" : "default",
          background: cover
            ? `center/cover no-repeat url("${cover}")`
            : accentFor(userId),
        }}
      />

      <div style={{ padding: space("0 16px") }}>
        <div
          style={{
            marginTop: -Math.round(avatarSize * 0.45),
            marginBottom: 10,
            display: "flex",
            alignItems: "flex-end",
            justifyContent: "space-between",
          }}
        >
          <button
            onClick={() => profile?.avatarUrl && openLightbox(profile.avatarUrl, name)}
            aria-label={`${name}'s picture, full size`}
            style={{
              display: "flex",
              padding: 0,
              cursor: profile?.avatarUrl ? "zoom-in" : "default",
            }}
          >
            <Avatar
              id={userId}
              name={name}
              mxc={profile?.avatarUrl}
              size={avatarSize}
              radius={Math.round(avatarSize / 3)}
              style={{ border: `3px solid ${ring}` }}
            />
          </button>
          {badge}
        </div>

        <div
          style={{
            fontFamily: "var(--font-heading)",
            fontWeight: 800,
            fontSize: headingSize(19),
            lineHeight: 1.2,
          }}
        >
          {name}
        </div>
        <div
          className="selectable"
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(11),
            color: "var(--text-tertiary)",
            wordBreak: "break-all",
          }}
        >
          {userId}
        </div>

        <PresenceLine userId={userId} />

        {profile?.status && (
          <div
            className="selectable"
            style={{
              marginTop: 11,
              padding: space("7px 11px"),
              borderRadius: corner(12),
              background: "var(--surface-inset)",
              fontSize: textSize(13),
              color: "var(--text-secondary)",
              wordBreak: "break-word",
            }}
          >
            {profile.status}
          </div>
        )}
      </div>
    </div>
  );
}
