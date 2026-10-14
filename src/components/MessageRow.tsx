import { useMemo, useRef, useState } from "react";

import {
  accentFor,
  displayNameFor,
  formatBytes,
  formatDuration,
  formatTime,
  localpart,
} from "../lib/display";
import * as ipc from "../lib/ipc";
import { mediaUrl } from "../lib/ipc";
import { useMediaBlob } from "../lib/blobMedia";
import { useIsCutout } from "../lib/cutout";
import { saveAttachment } from "../lib/download";
import { imageLookup, reactionImage, reactionKeyFor } from "../lib/packs";
import { linkify, renderFormattedBody } from "../lib/richText";
import type { Content, EventItem, MediaInfo, PackImage } from "../lib/types";
import { useIsMobile, useLongPress } from "../lib/viewport";
import { useStore } from "../store";
import { EmojiPicker } from "./EmojiPicker";
import { AvatarButton, useProfileAnchor } from "./ProfileCard";
import { Icon, Spinner } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

export function MessageRow({
  item,
  rowId,
  roomId,
  threadRoot,
  showHeader,
  onReply,
  onOpenThread,
}: {
  item: EventItem;
  /** The timeline row's stable id, which the scroll anchor finds it by. */
  rowId: string;
  roomId: string;
  threadRoot?: string;
  /** False when this message continues a run from the same author. */
  showHeader: boolean;
  onReply: (eventId: string) => void;
  onOpenThread: (rootEventId: string) => void;
}) {
  const [hovered, setHovered] = useState(false);
  /** The rect the picker hangs off, or null when it's closed. */
  const [pickerAnchor, setPickerAnchor] = useState<DOMRect | null>(null);
  const pickerButton = useRef<HTMLButtonElement>(null);
  const setDraft = useStore((s) => s.setDraft);
  const showBanner = useStore((s) => s.showBanner);
  const recentReactions = useStore((s) => s.settings.recentReactions);
  const noteReactionUsed = useStore((s) => s.noteReactionUsed);
  const packs = useStore((s) => s.packs);
  // Every image, not just the ones usable as emotes: this is for reading what
  // somebody else sent, and they may have reacted with anything.
  const emotes = useMemo(() => imageLookup(packs), [packs]);
  const senderAnchor = useProfileAnchor(item.sender);
  const isMobile = useIsMobile();
  /** The touch action sheet, which stands in for the hover bar. */
  const [sheetOpen, setSheetOpen] = useState(false);
  const longPress = useLongPress(
    isMobile && item.eventId ? () => setSheetOpen(true) : undefined,
  );

  const author = displayNameFor(item.sender, item.senderName);
  const accent = accentFor(item.sender);
  const pending = item.sendState?.status === "notSentYet";
  const failed = item.sendState?.status === "failed";

  /**
   * Everything you can do to a message, in one list.
   *
   * The hover bar and the touch sheet are two presentations of the same set —
   * keeping them as one array is what stops a new action from being added to
   * the pointer surface and quietly missing from the touch one.
   */
  function messageActions(): RowAction[] {
    const list: RowAction[] = [];
    if (item.canReply) {
      list.push({
        icon: "arrow-bend-up-left",
        label: "reply",
        run: () => onReply(item.eventId!),
      });
    }
    list.push({
      icon: "chats-circle",
      label: "reply in thread",
      run: () => onOpenThread(item.threadRoot ?? item.eventId!),
    });
    list.push({
      icon: "copy",
      label: "copy text",
      // Desktop already has this: select the text and press ⌘C. Long-press on
      // a phone is where that affordance went, so the sheet has to offer it.
      touchOnly: true,
      run: () => {
        navigator.clipboard
          .writeText(plainBody(item.content))
          .catch(() => showBanner("error", "couldn't copy that"));
      },
    });
    if (item.isEditable) {
      list.push({
        icon: "pencil-simple",
        label: "edit",
        run: () => {
          const key = threadRoot ? `${roomId}|${threadRoot}` : roomId;
          setDraft(key, {
            editing: item.eventId,
            body: plainBody(item.content),
            replyTo: null,
          });
        },
      });
    }
    if (item.isOwn) {
      list.push({ icon: "trash", label: "delete", danger: true, run: () => void remove() });
    }
    return list;
  }

  async function react(key: string) {
    if (!item.eventId) return;
    setPickerAnchor(null);
    // Remembered even if the send fails: picking it is the signal about what
    // this person reaches for, and a failure is usually the network, not the
    // choice.
    noteReactionUsed(key);
    try {
      await ipc.toggleReaction(roomId, item.eventId, key, threadRoot);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  async function remove() {
    if (!item.eventId) return;
    try {
      await ipc.redactEvent(roomId, item.eventId, undefined, threadRoot);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  // State changes and membership events render as thin one-line notes rather
  // than full message rows, matching the design's system lines.
  if (isSystemEvent(item.content)) {
    return (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: space(10),
          padding: `${space("6px 0 6px")} calc(40px + ${space(12)})`,
          color: "var(--text-tertiary)",
          fontSize: textSize(12),
        }}
      >
        <Icon name={systemIcon(item.content)} size={12} color="var(--accent-primary)" />
        <span className="selectable">{systemText(item, author)}</span>
        <span style={{ fontFamily: "var(--font-mono)", fontSize: textSize(10.5) }}>
          {formatTime(item.timestamp)}
        </span>
      </div>
    );
  }

  return (
    <div
      className="uwu-msg"
      data-row-id={rowId}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      {...longPress}
      style={{
        position: "relative",
        display: "flex",
        gap: space(12),
        padding: space("5px 8px"),
        borderRadius: corner(16),
        transition: "background var(--dur-fast) var(--ease-out)",
        background: item.isHighlighted
          ? "color-mix(in srgb, var(--pink-500) 8%, transparent)"
          : hovered
            ? "rgba(255,255,255,.035)"
            : "transparent",
        opacity: pending ? 0.72 : 1,
      }}
    >
      <div
        style={{
          width: 40,
          flex: "none",
          display: "flex",
          justifyContent: "center",
          paddingTop: space(2),
        }}
      >
        {showHeader ? (
          <AvatarButton
            userId={item.sender}
            name={author}
            mxc={item.senderAvatar}
            size={38}
            radius={14}
          />
        ) : (
          hovered && (
            <span
              style={{
                fontFamily: "var(--font-mono)",
                fontSize: textSize(9.5),
                color: "var(--text-tertiary)",
                paddingTop: space(4),
              }}
            >
              {formatTime(item.timestamp)}
            </span>
          )
        )}
      </div>

      <div style={{ flex: 1, minWidth: 0 }}>
        {showHeader && (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: space(8),
              marginBottom: 3,
              flexWrap: "wrap",
            }}
          >
            <button
              {...senderAnchor}
              aria-label={`${author}'s profile`}
              style={{
                fontFamily: "var(--font-display)",
                fontWeight: 800,
                fontSize: textSize(14.5),
                color: accent,
                cursor: "pointer",
                padding: 0,
              }}
            >
              {author}
            </button>
            <span
              style={{
                fontFamily: "var(--font-mono)",
                fontSize: textSize(10.5),
                color: "var(--text-tertiary)",
              }}
            >
              {formatTime(item.timestamp)}
            </span>
            {item.shield && (
              <span
                title={item.shield.message}
                style={{ display: "inline-flex", alignItems: "center" }}
              >
                <Icon
                  name="warning"
                  size={12}
                  color={
                    item.shield.colour === "red"
                      ? "var(--status-danger)"
                      : "var(--text-tertiary)"
                  }
                />
              </span>
            )}
          </div>
        )}

        {item.reply && (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: space(8),
              marginBottom: 5,
              padding: space("5px 11px"),
              borderRadius: corner(12),
              background: "rgba(255,255,255,.04)",
              borderLeft: "2px solid var(--accent-tertiary)",
              maxWidth: "min(520px, 100%)",
            }}
          >
            <Icon name="arrow-bend-up-left" size={12} color="var(--accent-tertiary)" />
            <span
              className="uwu-ellipsis"
              // Without this the nowrap text reports its full width as the
              // row's minimum and pushes the message list off-screen.
              style={{ minWidth: 0, fontSize: textSize(12), color: "var(--text-secondary)" }}
            >
              {item.reply.senderName ??
                (item.reply.sender ? localpart(item.reply.sender) : "someone")}
              : {item.reply.body ?? "…"}
            </span>
          </div>
        )}

        <ContentBody content={item.content} emojiOnly={item.isEmojiOnly} />

        {item.isEdited && (
          <span
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: textSize(10),
              color: "var(--text-tertiary)",
              marginLeft: 6,
            }}
          >
            (edited)
          </span>
        )}

        {item.reactions.length > 0 && (
          <div style={{ display: "flex", flexWrap: "wrap", gap: space(6), marginTop: 7 }}>
            {item.reactions.map((reaction) => (
              <button
                key={reaction.key}
                onClick={() => void react(reaction.key)}
                title={reaction.senders.map(localpart).join(", ")}
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  gap: space(6),
                  padding: space("3px 10px"),
                  borderRadius: 999,
                  cursor: "pointer",
                  fontFamily: "var(--font-mono)",
                  fontSize: textSize(11.5),
                  opacity: reaction.pending ? 0.55 : 1,
                  transition: "all var(--dur-fast) var(--ease-bounce)",
                  ...(reaction.mine
                    ? {
                        background: "color-mix(in srgb, var(--acid-500) 16%, transparent)",
                        border: "1px solid var(--accent-primary)",
                        color: "var(--accent-primary)",
                      }
                    : {
                        background: "var(--surface-card-raised)",
                        border: "1px solid var(--border-subtle)",
                        color: "var(--text-secondary)",
                      }),
                }}
              >
                <ReactionKey label={reaction.key} emotes={emotes} size={13} />
                <span style={{ fontWeight: 700 }}>{reaction.count}</span>
              </button>
            ))}
          </div>
        )}

        {item.threadSummary && item.eventId && (
          <button
            onClick={() => onOpenThread(item.eventId!)}
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: space(8),
              marginTop: 8,
              padding: space("6px 12px 6px 8px"),
              borderRadius: 999,
              background: "var(--surface-card)",
              border: "1px solid var(--border-subtle)",
              cursor: "pointer",
            }}
          >
            <Icon name="chats-circle" size={14} color="var(--accent-quaternary)" />
            <span
              style={{
                fontFamily: "var(--font-display)",
                fontWeight: 700,
                fontSize: textSize(12.5),
                color: "var(--accent-quaternary)",
              }}
            >
              {item.threadSummary.numReplies}{" "}
              {item.threadSummary.numReplies === 1 ? "reply" : "replies"} in thread
            </span>
          </button>
        )}

        {failed && (
          <div
            style={{
              marginTop: 6,
              fontSize: textSize(11.5),
              color: "var(--status-danger)",
              display: "flex",
              alignItems: "center",
              gap: space(6),
            }}
          >
            <Icon name="warning-circle" size={12} color="var(--status-danger)" />
            couldn't send — it'll retry when you're back online
          </div>
        )}
      </div>

      {pending && (
        <span
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10),
            color: "var(--text-tertiary)",
            alignSelf: "flex-end",
            animation: "uwuPulse 1.2s infinite",
            flex: "none",
          }}
        >
          sending…
        </span>
      )}

      {/* The bar stays up while the picker is open, so the thing it's attached
          to doesn't vanish from under it when the pointer leaves the row.
          Never on touch: WKWebView synthesises `mouseenter` on tap, so this
          would flash up on every tap — and the long-press sheet is where these
          actions live there. */}
      {!isMobile && (hovered || pickerAnchor) && item.eventId && (
        <div
          style={{
            position: "absolute",
            top: -12,
            right: 12,
            display: "flex",
            alignItems: "center",
            gap: space(2),
            padding: space(3),
            borderRadius: corner(12),
            background: "var(--surface-card-raised)",
            border: "1px solid var(--border-default)",
            boxShadow: "var(--shadow-card)",
            zIndex: 2,
          }}
        >
          {recentReactions.map((key) => (
            <button
              key={key}
              onClick={() => void react(key)}
              title={`react with ${key}`}
              style={{
                width: 26,
                height: 26,
                borderRadius: corner(9),
                fontSize: textSize(15),
                lineHeight: 1,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                cursor: "pointer",
                transition: "transform var(--dur-fast) var(--ease-bounce)",
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = "rgba(255,255,255,.08)";
                e.currentTarget.style.transform = "scale(1.15)";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = "transparent";
                e.currentTarget.style.transform = "";
              }}
            >
              <ReactionKey label={key} emotes={emotes} size={15} />
            </button>
          ))}

          <span
            aria-hidden
            style={{
              width: 1,
              height: 16,
              margin: "0 2px",
              background: "var(--border-subtle)",
            }}
          />

          <ActionButton
            ref={pickerButton}
            icon="smiley"
            title="react"
            onClick={() =>
              setPickerAnchor((open) =>
                open ? null : (pickerButton.current?.getBoundingClientRect() ?? null),
              )
            }
          />
          {messageActions()
            .filter((action) => !action.touchOnly)
            .map((action) => (
              <ActionButton
                key={action.label}
                icon={action.icon}
                title={action.label}
                onClick={action.run}
                danger={action.danger}
              />
            ))}
        </div>
      )}

      {sheetOpen && item.eventId && (
        <MessageActionSheet
          actions={messageActions()}
          quick={recentReactions}
          emotes={emotes}
          onReact={(key) => void react(key)}
          onPickMore={(rect) => setPickerAnchor(rect)}
          onClose={() => setSheetOpen(false)}
        />
      )}

      {pickerAnchor && (
        <EmojiPicker
          anchor={pickerAnchor}
          packs={packs}
          onPick={(picked) => void react(reactionKeyFor(picked))}
          onClose={() => setPickerAnchor(null)}
        />
      )}
    </div>
  );
}

/** One thing you can do to a message, drawn by both the hover bar and the sheet. */
type RowAction = {
  icon: string;
  label: string;
  run: () => void;
  danger?: boolean;
  /** Only offered on touch, where the pointer equivalent is unreachable. */
  touchOnly?: boolean;
};

/**
 * The touch equivalent of the hover bar: a sheet up from the bottom edge.
 *
 * Bottom-anchored rather than drawn at the message, because the message may be
 * anywhere on screen including under a thumb, and because the bottom of the
 * screen is the part of a phone a thumb actually reaches.
 */
function MessageActionSheet({
  actions,
  quick,
  emotes,
  onReact,
  onPickMore,
  onClose,
}: {
  actions: RowAction[];
  quick: string[];
  emotes: ReturnType<typeof imageLookup>;
  onReact: (key: string) => void;
  onPickMore: (anchor: DOMRect) => void;
  onClose: () => void;
}) {
  return (
    <div
      onClick={onClose}
      role="presentation"
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 140,
        display: "flex",
        flexDirection: "column",
        justifyContent: "flex-end",
        background: "rgba(0,0,0,.5)",
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          background: "var(--surface-card-raised)",
          borderTop: "1px solid var(--border-default)",
          borderTopLeftRadius: 22,
          borderTopRightRadius: 22,
          padding: space("10px 10px calc(var(--safe-bottom) + 10px)"),
          boxShadow: "var(--shadow-pop)",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: space(4),
            padding: space("6px 4px 10px"),
            borderBottom: "1px solid var(--border-subtle)",
            marginBottom: 6,
          }}
        >
          {quick.map((key) => (
            <button
              key={key}
              onClick={() => {
                onReact(key);
                onClose();
              }}
              aria-label={`react with ${key}`}
              style={{
                flex: 1,
                height: 44,
                borderRadius: corner(12),
                fontSize: textSize(22),
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                cursor: "pointer",
              }}
            >
              <ReactionKey label={key} emotes={emotes} size={22} />
            </button>
          ))}
          <button
            onClick={(e) => {
              onPickMore(e.currentTarget.getBoundingClientRect());
              onClose();
            }}
            aria-label="more reactions"
            style={{
              flex: "none",
              width: 44,
              height: 44,
              borderRadius: corner(12),
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              cursor: "pointer",
              background: "var(--surface-card)",
            }}
          >
            <Icon name="smiley" size={20} color="var(--text-secondary)" />
          </button>
        </div>

        {actions.map((action) => (
          <button
            key={action.label}
            onClick={() => {
              action.run();
              onClose();
            }}
            style={{
              display: "flex",
              alignItems: "center",
              gap: space(12),
              width: "100%",
              height: 48,
              padding: space("0 12px"),
              borderRadius: corner(14),
              cursor: "pointer",
              textAlign: "left",
              fontFamily: "var(--font-display)",
              fontWeight: 700,
              fontSize: textSize(15),
              color: action.danger ? "var(--status-danger)" : "var(--text-primary)",
            }}
          >
            <Icon
              name={action.icon}
              size={18}
              color={action.danger ? "var(--status-danger)" : "var(--text-secondary)"}
            />
            {action.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/**
 * A reaction as it should be drawn.
 *
 * A custom emote arrives either as `:shortcode:`, which needs the pack, or as
 * the image's own `mxc://` address, which doesn't — see `reactionImage`. Both
 * are drawn; anything else is the text it is.
 */
function ReactionKey({
  label,
  emotes,
  size,
}: {
  label: string;
  emotes: Map<string, PackImage>;
  size: number;
}) {
  const image = reactionImage(label, emotes);
  const src = image ? mediaUrl(image.url, { width: size * 3, height: size * 3 }) : null;

  if (!image || !src) return <span>{label}</span>;

  return (
    <img
      src={src}
      alt={image.label}
      title={image.label}
      draggable={false}
      style={{ height: size, width: "auto", maxWidth: size * 3, objectFit: "contain" }}
    />
  );
}

function ActionButton({
  icon,
  title,
  onClick,
  danger,
  ref,
}: {
  icon: string;
  title: string;
  onClick: () => void;
  danger?: boolean;
  /** Needed by the one button that anchors a popover to itself. */
  ref?: React.Ref<HTMLButtonElement>;
}) {
  return (
    <button
      ref={ref}
      onClick={onClick}
      title={title}
      style={{
        width: 28,
        height: 26,
        borderRadius: corner(9),
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.background = "rgba(255,255,255,.08)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = "transparent";
      }}
    >
      <Icon
        name={icon}
        size={14}
        color={danger ? "var(--status-danger)" : "var(--text-secondary)"}
      />
    </button>
  );
}

// ---------------------------------------------------------------------------
// content
// ---------------------------------------------------------------------------

function ContentBody({ content, emojiOnly }: { content: Content; emojiOnly: boolean }) {
  const textStyle = {
    // Emoji-only messages keep their ratio to the text around them.
    fontSize: emojiOnly ? "calc(var(--text-message) * 34 / 14.5)" : "var(--text-message)",
    lineHeight: emojiOnly ? 1.2 : 1.55,
    color: "var(--text-primary)",
    maxWidth: "min(640px, 100%)",
    textWrap: "pretty" as const,
    whiteSpace: "pre-wrap" as const,
    // `anywhere`, not `break-word`. Both wrap a long URL once a width is
    // settled, but only `anywhere` also shrinks the element's *min-content*
    // width. With `break-word` the row still reports the whole unbroken URL as
    // its minimum, every flex ancestor sizes to that, and the message list ends
    // up wider than the screen — which on a phone is a horizontal scrollbar and
    // a timeline shifted off its left edge.
    overflowWrap: "anywhere" as const,
  };

  switch (content.kind) {
    case "text":
      return (
        <div className="selectable" style={textStyle}>
          <Body body={content.body} formatted={content.formatted} />
        </div>
      );

    case "emote":
      return (
        <div className="selectable" style={{ ...textStyle, fontStyle: "italic" }}>
          <Body body={content.body} formatted={content.formatted} />
        </div>
      );

    case "notice":
      return (
        <div
          className="selectable"
          style={{
            ...textStyle,
            color: "var(--text-secondary)",
            fontFamily: "var(--font-mono)",
            fontSize: textSize(13),
          }}
        >
          <Body body={content.body} formatted={content.formatted} />
        </div>
      );

    case "image":
    case "sticker":
      return <ImageBody body={content.body} media={content.media} sticker={content.kind === "sticker"} />;

    case "video":
      return <VideoBody body={content.body} media={content.media} />;

    case "audio":
      return <AudioBody body={content.body} media={content.media} />;

    case "file":
      return <FileBody body={content.body} media={content.media} />;

    case "location":
      return (
        <a
          href={content.geoUri}
          style={{ display: "inline-flex", alignItems: "center", gap: space(8), fontSize: textSize(14) }}
        >
          <Icon name="map-pin" size={16} color="var(--accent-quaternary)" />
          {content.body || "shared a location"}
        </a>
      );

    case "poll":
      return (
        <div
          style={{
            padding: space(14),
            borderRadius: corner(16),
            background: "var(--surface-card)",
            border: "1px solid var(--border-subtle)",
            maxWidth: 420,
          }}
        >
          <div style={{ fontFamily: "var(--font-heading)", fontWeight: 700, fontSize: headingSize(14.5) }}>
            {content.question}
          </div>
          <div style={{ marginTop: 8, display: "flex", flexDirection: "column", gap: space(6) }}>
            {content.answers.map((answer, i) => (
              <div
                key={i}
                style={{
                  padding: space("7px 12px"),
                  borderRadius: corner(12),
                  background: "var(--surface-card-raised)",
                  fontSize: textSize(13),
                  color: "var(--text-secondary)",
                }}
              >
                {answer}
              </div>
            ))}
          </div>
          {content.ended && (
            <div style={{ marginTop: 8, fontSize: textSize(11), color: "var(--text-tertiary)" }}>
              this poll has ended
            </div>
          )}
        </div>
      );

    case "redacted":
      return (
        <div style={{ fontSize: textSize(13.5), color: "var(--text-tertiary)", fontStyle: "italic" }}>
          message deleted
        </div>
      );

    case "unableToDecrypt":
      return (
        <div
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: space(8),
            padding: space("7px 13px"),
            borderRadius: corner(14),
            background: "color-mix(in srgb, var(--violet-500) 8%, transparent)",
            border: "1px dashed color-mix(in srgb, var(--violet-500) 40%, transparent)",
            fontSize: textSize(13),
            color: "var(--text-secondary)",
          }}
          title={content.reason}
        >
          <Icon name="lock-key-open" size={14} color="var(--accent-tertiary)" />
          can't decrypt this one — restore your key backup in settings → security
        </div>
      );

    case "callInvite":
    case "rtcNotification":
      return (
        <div
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: space(8),
            fontSize: textSize(13.5),
            color: "var(--text-secondary)",
          }}
        >
          <Icon name="phone-call" size={15} color="var(--accent-primary)" />
          started a call
        </div>
      );

    default:
      return (
        <div style={{ fontSize: textSize(13), color: "var(--text-tertiary)", fontStyle: "italic" }}>
          unsupported message
        </div>
      );
  }
}

/**
 * Prefer the sender's formatted body, fall back to plain text.
 *
 * `formatted_body` is HTML from an untrusted source, so `renderFormattedBody`
 * rebuilds it as React elements from an allowlist rather than injecting markup.
 */
function Body({ body, formatted }: { body: string; formatted: string | null }) {
  // Plain text is the common case, and a URL in it is still a URL — nobody
  // types `<a href>` into a chat box.
  if (!formatted) return <>{linkify(body)}</>;

  const rendered = renderFormattedBody(formatted);
  return <>{rendered ?? linkify(body)}</>;
}

function ImageBody({
  body,
  media,
  sticker,
}: {
  body: string;
  media: MediaInfo;
  sticker: boolean;
}) {
  const openLightbox = useStore((s) => s.openLightbox);
  const maxWidth = sticker ? 160 : 400;
  const maxHeight = sticker ? 160 : 320;

  // Preserve the aspect ratio from the event so the timeline doesn't jump as
  // images load.
  const ratio = media.width && media.height ? media.width / media.height : 4 / 3;
  const width = Math.min(maxWidth, maxHeight * ratio);
  const height = width / ratio;

  const src = mediaUrl(media.thumbnailMxc ?? media.mxc, {
    width: Math.round(width),
    height: Math.round(height),
  });

  // A transparent image is drawn the way a sticker is: on the timeline itself,
  // not on a card. It keeps the size it would have had as a picture, so
  // finding out doesn't shift the timeline under the reader.
  const cutout = useIsCutout(sticker ? null : media.mxc, media.mimetype, { width, height });
  const frameless = sticker || cutout;

  if (!src) return <div style={{ fontSize: textSize(13), color: "var(--text-tertiary)" }}>{body}</div>;

  return (
    <img
      src={src}
      alt={body}
      loading="lazy"
      draggable={false}
      // The lightbox gets the *original* URI, not the thumbnail being shown.
      onClick={() => media.mxc && openLightbox(media.mxc, body || "picture")}
      style={{
        cursor: media.mxc ? "zoom-in" : "default",
        marginTop: 4,
        // `width` is the size we *want*; on a narrow screen the column beside
        // the avatar is smaller than that, and a fixed pixel width there makes
        // the whole message list wider than the phone. Cap it, and let
        // `aspect-ratio` recompute the height so the no-jump property survives
        // — a fixed `height` with a shrinking width would just crop instead.
        width,
        maxWidth: "100%",
        height: "auto",
        aspectRatio: `${Math.round(width)} / ${Math.round(height)}`,
        // `contain` for a cut-out: cropping one would slice off the edges of
        // whatever was cut out.
        objectFit: cutout ? "contain" : "cover",
        borderRadius: corner(frameless ? 8 : 16),
        border: frameless ? "none" : "1px solid var(--border-subtle)",
        background: cutout ? "transparent" : "var(--surface-card)",
      }}
    />
  );
}

function VideoBody({ body, media }: { body: string; media: MediaInfo }) {
  const { url, error } = useMediaBlob(media.mxc, media.mimetype);

  const frame: React.CSSProperties = {
    marginTop: 4,
    maxWidth: "min(400px, 100%)",
    maxHeight: 320,
    borderRadius: corner(16),
    border: "1px solid var(--border-subtle)",
    background: "var(--ink-950)",
  };

  if (error) {
    return (
      <div
        style={{ ...frame, padding: space("12px 16px"), fontSize: textSize(13), color: "var(--status-danger)" }}
      >
        couldn't load {body}
      </div>
    );
  }

  if (!url) {
    return (
      <div
        style={{
          ...frame,
          width: "min(320px, 100%)",
          height: 180,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <Spinner />
      </div>
    );
  }

  return <video src={url} controls preload="metadata" style={frame} aria-label={body} />;
}

function AudioBody({ body, media }: { body: string; media: MediaInfo }) {
  const { url } = useMediaBlob(media.mxc, media.mimetype);

  return (
    <div
      style={{
        marginTop: 4,
        display: "flex",
        alignItems: "center",
        gap: space(10),
        padding: space("9px 14px"),
        borderRadius: corner(16),
        background: "var(--surface-card)",
        border: "1px solid var(--border-subtle)",
        maxWidth: "min(340px, 100%)",
      }}
    >
      <Icon
        name={media.isVoice ? "microphone" : "music-notes"}
        size={16}
        color="var(--accent-secondary)"
      />
      {url ? (
        <audio src={url} controls style={{ height: 30, flex: 1 }} aria-label={body} />
      ) : (
        <div style={{ flex: 1, display: "flex", justifyContent: "center" }}>
          <Spinner size={14} />
        </div>
      )}
      {media.durationMs != null && (
        <span
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10.5),
            color: "var(--text-tertiary)",
          }}
        >
          {formatDuration(media.durationMs)}
        </span>
      )}
    </div>
  );
}

function FileBody({ body, media }: { body: string; media: MediaInfo }) {
  const [saving, setSaving] = useState(false);
  const mxc = media.mxc;

  async function download() {
    if (!mxc || saving) return;
    setSaving(true);
    try {
      await saveAttachment(mxc, body);
    } finally {
      setSaving(false);
    }
  }

  return (
    <button
      type="button"
      onClick={() => void download()}
      disabled={!mxc}
      title={mxc ? `save ${body}` : body}
      style={{
        marginTop: 4,
        display: "inline-flex",
        alignItems: "center",
        gap: space(11),
        padding: space("11px 14px"),
        borderRadius: corner(16),
        background: "var(--surface-card)",
        border: "1px solid var(--border-subtle)",
        maxWidth: "min(360px, 100%)",
        textAlign: "left",
        cursor: mxc ? "pointer" : "default",
      }}
      onMouseEnter={(e) => {
        if (mxc) e.currentTarget.style.background = "var(--surface-card-raised)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = "var(--surface-card)";
      }}
    >
      {saving ? (
        <Spinner size={16} />
      ) : (
        <Icon name="file-arrow-down" size={18} color="var(--accent-quaternary)" />
      )}
      <div style={{ minWidth: 0 }}>
        <div
          className="uwu-ellipsis"
          style={{ fontFamily: "var(--font-display)", fontWeight: 700, fontSize: textSize(13) }}
        >
          {body}
        </div>
        <div
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10.5),
            color: "var(--text-tertiary)",
          }}
        >
          {formatBytes(media.size)}
        </div>
      </div>
    </button>
  );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function isSystemEvent(content: Content): boolean {
  return (
    content.kind === "membership" ||
    content.kind === "profileChange" ||
    content.kind === "state"
  );
}

function systemIcon(content: Content): string {
  if (content.kind === "membership") return "user-circle";
  if (content.kind === "state") return "lock-key";
  return "user-switch";
}

function systemText(item: EventItem, author: string): string {
  const { content } = item;
  if (content.kind === "profileChange") return content.summary;

  if (content.kind === "membership") {
    const who = content.displayName ?? localpart(content.userId);
    switch (content.change) {
      case "joined":
        return `${who} joined`;
      case "left":
        return `${who} left`;
      case "invited":
        return `${author} invited ${who}`;
      case "banned":
        return `${author} banned ${who}`;
      case "kicked":
        return `${author} removed ${who}`;
      default:
        return `${who}'s membership changed`;
    }
  }

  if (content.kind === "state") {
    if (content.eventType === "m.room.encryption") {
      return `${author} turned on end-to-end encryption`;
    }
    if (content.eventType === "m.room.topic") return `${author} changed the topic`;
    if (content.eventType === "m.room.name") return `${author} renamed the room`;
    if (content.eventType === "m.room.avatar") return `${author} changed the room avatar`;
    return `${author} updated ${content.eventType.replace(/^m\.room\./, "")}`;
  }

  return "";
}

/** The text to prefill when editing a message. */
function plainBody(content: Content): string {
  return "body" in content ? content.body : "";
}
