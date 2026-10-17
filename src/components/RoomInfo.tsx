import { useEffect, useState } from "react";

import { accentFor, displayNameFor } from "../lib/display";
import * as ipc from "../lib/ipc";
import { cachedProfile, loadProfile } from "../lib/profiles";
import type { Profile, RoomMember, RoomSummary } from "../lib/types";
import { useStore } from "../store";
import { PresenceDot } from "./Presence";
import { AvatarButton, useProfileAnchor } from "./ProfileCard";
import { ProfileHeader } from "./ProfileHeader";
import { RoomSettingsDialog } from "./RoomSettings";
import { Avatar, Icon, RaveLabel, Spinner } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";
import { DEFAULTS, INFO_PANEL_WIDTH_RANGE } from "../lib/settings";

/**
 * What a full-bleed cover is asked for at: the widest the panel can be dragged,
 * so it stays sharp at any width without refetching on every pixel of a drag.
 */
const COVER_WIDTH = INFO_PANEL_WIDTH_RANGE.max;

/** `onBack` is passed only by the mobile shell, where this is a pushed view
    rather than a side panel. See `ChatPane` for why the prop doubles as the
    layout switch. */
export function RoomInfo({
  room,
  onBack,
  width = DEFAULTS.infoPanelWidth,
}: {
  room: RoomSummary;
  onBack?: () => void;
  width?: number;
}) {
  const [members, setMembers] = useState<RoomMember[] | null>(null);
  const showBanner = useStore((s) => s.showBanner);
  const openLightbox = useStore((s) => s.openLightbox);
  const ownUserId = useStore((s) => s.session?.userId);
  // Local rather than in the store: this dialog only exists while the panel
  // that owns the room is on screen.
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setMembers(null);

    ipc
      .getMembers(room.id)
      .then((list) => {
        if (!cancelled) setMembers(list);
      })
      .catch((e) => {
        if (!cancelled) {
          setMembers([]);
          showBanner("error", ipc.asUwuError(e).message);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [room.id, showBanner]);

  // `get_members` already returns only active (joined + invited) members, so
  // this list is shown as-is rather than filtered again.
  const joined = members ?? [];
  const unverified = joined.filter((m) => m.verification !== "verified").length;

  // The mute toggle used to swallow its errors, so one that failed server-side
  // looked exactly like a button that did nothing.
  const toggleMuted = () =>
    void ipc
      .setRoomMuted(room.id, !room.isMuted)
      .catch((e) => showBanner("error", ipc.asUwuError(e).message));

  // A DM is a person, so the top of the panel should be them rather than a
  // room avatar and a room ID. Not every direct room has exactly one other
  // member — you can be alone in one, or still be flagged direct after others
  // joined — so this falls back to the room header whenever it isn't obvious
  // who the DM is with.
  const others = joined.filter((m) => m.userId !== ownUserId);
  const partner = room.isDirect && others.length === 1 ? others[0] : undefined;

  return (
    <div
      className="uwu-scroll"
      style={{
        position: "relative",
        zIndex: 1,
        width: onBack ? "100%" : width,
        flex: onBack ? 1 : "none",
        // The border separates this from the timeline beside it. As a pushed
        // view there is nothing to its left to separate it from.
        borderLeft: onBack ? undefined : "1px solid var(--border-subtle)",
        background: "color-mix(in srgb, var(--ink-900) 72%, transparent)",
        padding: space(onBack
          ? "calc(var(--safe-top) + 56px) 16px calc(var(--safe-bottom) + 24px)"
          : "18px 16px 24px"),
      }}
    >
      {onBack && (
        <button
          onClick={onBack}
          aria-label="back to messages"
          style={{
            position: "absolute",
            top: "calc(var(--safe-top) + 6px)",
            left: 4,
            width: 44,
            height: 44,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            cursor: "pointer",
            zIndex: 2,
          }}
        >
          <Icon name="caret-left" size={18} color="var(--text-secondary)" />
        </button>
      )}

      {/* Tucked into the corner: things you set once and then forget about. */}
      <div
        style={{
          position: "absolute",
          top: onBack ? "calc(var(--safe-top) + 14px)" : 14,
          right: 14,
          display: "flex",
          gap: space(6),
          zIndex: 2,
        }}
      >
        <CornerButton
          icon={room.isMuted ? "bell-slash" : "bell"}
          label={room.isMuted ? "unmute notifications" : "mute notifications"}
          colour={room.isMuted ? "var(--status-warning)" : undefined}
          pressed={room.isMuted}
          onClick={toggleMuted}
        />
        <CornerButton
          icon="pencil-simple"
          label="room settings"
          onClick={() => setSettingsOpen(true)}
        />
      </div>

      {partner ? (
        <DmHeader partner={partner} />
      ) : (
        <div style={{ textAlign: "center", padding: space("8px 0 16px") }}>
          <button
            onClick={() => room.avatarUrl && openLightbox(room.avatarUrl, room.name)}
            aria-label={`${room.name}'s picture, full size`}
            style={{
              display: "inline-block",
              padding: 0,
              transform: "rotate(calc(-3deg * var(--tilt-scale)))",
              cursor: room.avatarUrl ? "zoom-in" : "default",
            }}
          >
            <Avatar
              id={room.id}
              name={room.name}
              mxc={room.avatarUrl}
              size={76}
              radius={26}
              fontSize={26}
            />
          </button>
          <div
            style={{
              fontFamily: "var(--font-heading)",
              fontWeight: 800,
              fontSize: headingSize(19),
              marginTop: 12,
            }}
          >
            {room.name}
          </div>
          <div
            className="selectable uwu-ellipsis"
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: textSize(11),
              color: "var(--text-tertiary)",
            }}
          >
            {room.canonicalAlias ?? room.id}
          </div>
        </div>
      )}

      {!partner && room.topic && (
        <div
          className="selectable"
          style={{
            fontSize: textSize(12.5),
            color: "var(--text-secondary)",
            lineHeight: 1.55,
            padding: space("0 4px 16px"),
          }}
        >
          {room.topic}
        </div>
      )}

      <EncryptionCard room={room} unverified={unverified} total={joined.length} />

      <RaveLabel style={{ padding: space("20px 4px 8px") }}>
        members · {members ? joined.length : "…"}
      </RaveLabel>

      {!members && (
        <div style={{ display: "flex", justifyContent: "center", padding: space(20) }}>
          <Spinner />
        </div>
      )}

      {joined.map((member) => (
        <MemberRow key={member.userId} member={member} />
      ))}

      {settingsOpen && (
        <RoomSettingsDialog room={room} onClose={() => setSettingsOpen(false)} />
      )}
    </div>
  );
}

/** A small round button for the panel's top-right corner. */
function CornerButton({
  icon,
  label,
  colour,
  pressed,
  onClick,
}: {
  icon: string;
  label: string;
  colour?: string;
  /** Set for toggles, so screen readers hear the state and not just the name. */
  pressed?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      title={label}
      aria-label={label}
      aria-pressed={pressed}
      style={{
        width: 28,
        height: 28,
        borderRadius: 999,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
        background: "var(--surface-card)",
        border: `1px solid ${colour ?? "var(--border-subtle)"}`,
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.background = "var(--surface-card-raised)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = "var(--surface-card)";
      }}
    >
      <Icon name={icon} size={13} color={colour ?? "var(--text-secondary)"} />
    </button>
  );
}

/**
 * The person on the other end of a DM, drawn the way their profile card is.
 *
 * The profile is a separate fetch from the room — cover, bio and status are
 * MSC4133 fields, not room state — but it's cached, so opening a DM you've
 * already looked at costs nothing.
 */
function DmHeader({ partner }: { partner: RoomMember }) {
  const name = displayNameFor(partner.userId, partner.displayName);
  const [profile, setProfile] = useState<Profile | null>(
    () => cachedProfile(partner.userId) ?? null,
  );

  useEffect(() => {
    let cancelled = false;
    loadProfile(partner.userId)
      .then((p) => !cancelled && setProfile(p))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [partner.userId]);

  return (
    <>
      <ProfileHeader
        userId={partner.userId}
        profile={profile}
        name={name}
        width={COVER_WIDTH}
        avatarSize={72}
        ring="var(--ink-900)"
        // Out to the panel's edges, over the padding the rest of it sits in.
        style={{ margin: "-18px -16px 0", overflow: "hidden" }}
      />

      {profile?.bio && (
        <div
          className="selectable"
          style={{
            fontSize: textSize(12.5),
            color: "var(--text-secondary)",
            lineHeight: 1.55,
            padding: space("12px 4px 4px"),
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          {profile.bio}
        </div>
      )}

      <div style={{ height: 16 }} />
    </>
  );
}

function EncryptionCard({
  room,
  unverified,
  total,
}: {
  room: RoomSummary;
  unverified: number;
  total: number;
}) {
  if (!room.isEncrypted) {
    return (
      <div
        style={{
          background: "var(--surface-card)",
          border: "1px solid color-mix(in srgb, var(--amber-500) 30%, transparent)",
          borderRadius: corner(20),
          padding: space(14),
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: space(8) }}>
          <Icon name="lock-key-open" size={16} color="var(--status-warning)" />
          <span style={{ fontFamily: "var(--font-heading)", fontWeight: 700, fontSize: headingSize(14) }}>
            not encrypted
          </span>
        </div>
        <div
          style={{
            fontSize: textSize(12.5),
            color: "var(--text-secondary)",
            marginTop: 5,
            lineHeight: 1.5,
          }}
        >
          messages here are readable by the homeserver.
        </div>
      </div>
    );
  }

  const allVerified = total > 0 && unverified === 0;

  return (
    <div
      style={{
        background: "var(--surface-card)",
        border: "1px solid var(--border-subtle)",
        borderRadius: corner(20),
        padding: space(14),
        boxShadow: "var(--shadow-card)",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: space(8) }}>
        <Icon
          name={allVerified ? "shield-check" : "shield-warning"}
          size={16}
          color={allVerified ? "var(--accent-primary)" : "var(--status-warning)"}
        />
        <span style={{ fontFamily: "var(--font-heading)", fontWeight: 700, fontSize: headingSize(14) }}>
          {allVerified ? "everyone verified~" : "some devices unverified"}
        </span>
      </div>
      <div
        style={{
          fontSize: textSize(12.5),
          color: "var(--text-secondary)",
          marginTop: 5,
          lineHeight: 1.5,
        }}
      >
        {allVerified
          ? `all ${total} members' devices are cross-signed. nothing to worry about.`
          : `${unverified} of ${total} members haven't been verified yet.`}
      </div>
      <div
        style={{
          display: "flex",
          gap: space(6),
          marginTop: 10,
          fontFamily: "var(--font-mono)",
          fontSize: textSize(10.5),
          color: "var(--text-tertiary)",
        }}
      >
        <span>megolm · v1</span>
        <span>·</span>
        <span>end-to-end</span>
      </div>
    </div>
  );
}

function MemberRow({ member }: { member: RoomMember }) {
  const name = displayNameFor(member.userId, member.displayName);
  const showBanner = useStore((s) => s.showBanner);
  const anchor = useProfileAnchor(member.userId);

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(10),
        padding: space("8px 6px"),
        borderRadius: corner(14),
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.background = "rgba(255,255,255,.05)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = "transparent";
      }}
    >
      <div style={{ position: "relative", flex: "none" }}>
        <AvatarButton
          userId={member.userId}
          name={name}
          mxc={member.avatarUrl}
          size={34}
          radius={12}
        />
        <PresenceDot userId={member.userId} size={11} />
      </div>
      <button
        {...anchor}
        aria-label={`${name}'s profile`}
        style={{ flex: 1, minWidth: 0, textAlign: "left", cursor: "pointer", padding: 0 }}
      >
        <div
          className="uwu-ellipsis"
          style={{
            fontFamily: "var(--font-display)",
            fontWeight: 700,
            fontSize: textSize(13.5),
            color: accentFor(member.userId),
          }}
        >
          {name}
        </div>
        <div
          className="uwu-ellipsis selectable"
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10.5),
            color: "var(--text-tertiary)",
          }}
        >
          {member.userId}
        </div>
      </button>

      {member.verification === "verified" ? (
        <Icon name="seal-check" size={14} color="var(--accent-primary)" />
      ) : (
        <button
          title="verify this person"
          onClick={() =>
            void ipc
              .requestVerification(member.userId)
              .catch((e) => showBanner("error", ipc.asUwuError(e).message))
          }
          style={{ cursor: "pointer", display: "flex" }}
        >
          <Icon name="seal-question" size={14} color="var(--text-tertiary)" />
        </button>
      )}
    </div>
  );
}
