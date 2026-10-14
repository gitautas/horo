/**
 * The emoji picker — custom packs, the Unicode set, search and skin tone.
 *
 * Rendered into a fixed-position layer and placed against an anchor rectangle,
 * so it can be opened from a message hover bar near the bottom of the timeline
 * without being clipped by whatever scrolls above it.
 *
 * Everything on offer is flattened into one list of sections: your packs first,
 * then the room's, then stickers if the caller can send them, then Unicode by
 * category. The rail across the top jumps between them and search runs over all
 * of them at once, so a shortcode and an emoji name are found the same way.
 */

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import {
  EMOJI_GROUPS,
  emojiName,
  searchEmoji,
  TONE_SAMPLES,
  withSkinTone,
  type Emoji,
  type SkinTone,
} from "../lib/emoji";
import { mediaUrl } from "../lib/ipc";
import {
  emoteLookup,
  imageLookup,
  matchEmotes,
  reactionImage,
  stickersOf,
  type Picked,
} from "../lib/packs";
import type { ImagePack, PackImage } from "../lib/types";
import { useStore } from "../store";
import { Icon } from "./ui";
import { corner, space, textSize } from "../lib/scale";

const WIDTH = 340;
const HEIGHT = 392;
/** Kept clear of the window edges so the picker never sits flush against them. */
const MARGIN = 10;
/** The size custom images are fetched and drawn at in the grid. */
const CELL_IMAGE = 26;

/** One scrollable block of the picker, and one button on the rail. */
type Section =
  | { key: string; title: string; icon: string; emojis: Emoji[] }
  | { key: string; title: string; icon: string; images: PackImage[]; sticker: boolean };

export function EmojiPicker({
  anchor,
  packs = [],
  stickers = false,
  onPick,
  onClose,
}: {
  /** Screen rect the picker should sit next to — usually the button that opened it. */
  anchor: DOMRect;
  /** Custom packs to offer above the Unicode set. */
  packs?: ImagePack[];
  /**
   * Whether stickers are offerable here.
   *
   * A reaction is a string, so a sticker can't be one: the hover bar asks for
   * emotes only, and the composer asks for everything.
   */
  stickers?: boolean;
  onPick: (picked: Picked) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [hovered, setHovered] = useState<Picked | null>(null);
  const skinTone = useStore((s) => s.settings.skinTone);
  const recent = useStore((s) => s.settings.recentReactions);
  const updateSettings = useStore((s) => s.updateSettings);

  const root = useRef<HTMLDivElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);

  useEffect(() => {
    search.current?.focus();
  }, []);

  // Escape closes, and so does a press anywhere else. Pointerdown rather than
  // click so a press that starts outside can't also land on something inside.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    const onDown = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) onClose();
    };

    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onDown, true);
    };
  }, [onClose]);

  // What you may pick, and what a remembered key might mean. The second is
  // wider: a shortcode remembered from a sticker-only pack should still draw.
  const lookup = useMemo(() => emoteLookup(packs), [packs]);
  const known = useMemo(() => imageLookup(packs), [packs]);

  const sections = useMemo<Section[]>(() => {
    const out: Section[] = [];

    for (const pack of packs) {
      const images = pack.images.filter((i) => i.isEmoticon);
      if (images.length > 0) {
        out.push({
          key: pack.id,
          title: pack.displayName,
          icon: pack.source === "user" ? "user-circle" : "users-three",
          images,
          sticker: false,
        });
      }
    }

    if (stickers) {
      const all = stickersOf(packs);
      if (all.length > 0) {
        out.push({ key: "stickers", title: "stickers", icon: "sticker", images: all, sticker: true });
      }
    }

    for (const group of EMOJI_GROUPS) {
      out.push({ key: group.name, title: group.name, icon: group.icon, emojis: group.emojis });
    }

    return out;
  }, [packs, stickers]);

  const results = useMemo(() => {
    if (query.trim().length === 0) return null;
    return {
      emotes: matchEmotes(lookup, query.trim(), 16),
      emojis: searchEmoji(query),
    };
  }, [query, lookup]);

  const position = usePlacement(anchor);

  function pickEmoji(emoji: Emoji) {
    onPick({
      kind: "unicode",
      emoji: emoji.skinToneSupport ? withSkinTone(emoji.emoji, skinTone) : emoji.emoji,
    });
  }

  function pickImage(image: PackImage, sticker: boolean) {
    onPick({ kind: sticker ? "sticker" : "emote", image });
  }

  /**
   * A remembered reaction: a character, a `:shortcode:`, or the `mxc://` key
   * some other client used for its own emote.
   *
   * Only a shortcode we still have a pack for becomes an emote pick; anything
   * else is sent back as the exact string it was, which is what the person
   * reacted with in the first place. That keeps a remembered reaction working
   * even when we can't say what it is.
   */
  function pickRecent(key: string) {
    const code = /^:([^\s:]+):$/.exec(key)?.[1];
    const image = code ? known.get(code) : undefined;
    onPick(image ? { kind: "emote", image } : { kind: "unicode", emoji: key });
  }

  function jumpTo(index: number) {
    scroller.current
      ?.querySelector(`[data-group="${index}"]`)
      ?.scrollIntoView({ block: "start" });
  }

  return (
    <div
      ref={root}
      role="dialog"
      aria-label="pick an emoji"
      style={{
        position: "fixed",
        ...position,
        width: WIDTH,
        height: HEIGHT,
        display: "flex",
        flexDirection: "column",
        borderRadius: corner(20),
        background: "var(--surface-card-raised)",
        border: "1px solid var(--border-default)",
        boxShadow: "var(--shadow-pop)",
        overflow: "hidden",
        zIndex: 60,
      }}
    >
      <div style={{ padding: space("10px 12px 8px") }}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: space(8),
            padding: space("6px 11px"),
            borderRadius: 999,
            background: "var(--surface-card)",
            border: "1px solid var(--border-subtle)",
          }}
        >
          <Icon name="magnifying-glass" size={13} color="var(--text-tertiary)" />
          <input
            ref={search}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="search emoji~"
            aria-label="search emoji"
            style={{
              flex: 1,
              minWidth: 0,
              background: "transparent",
              border: "none",
              outline: "none",
              color: "var(--text-primary)",
              fontSize: textSize(13),
            }}
          />
        </div>
      </div>

      {!results && (
        <div
          className="uwu-scroll"
          style={{
            display: "flex",
            gap: space(2),
            padding: space("0 10px 6px"),
            borderBottom: "1px solid var(--border-subtle)",
          }}
        >
          {sections.map((section, i) => (
            <button
              key={section.key}
              onClick={() => jumpTo(i)}
              title={section.title}
              aria-label={section.title}
              style={{
                flex: 1,
                minWidth: 22,
                height: 26,
                borderRadius: corner(8),
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                cursor: "pointer",
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = "rgba(255,255,255,.07)";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = "transparent";
              }}
            >
              <Icon name={section.icon} size={14} color="var(--text-tertiary)" />
            </button>
          ))}
        </div>
      )}

      <div ref={scroller} className="uwu-scroll" style={{ flex: 1, padding: space("4px 8px 8px") }}>
        {results ? (
          <SearchResults
            results={results}
            query={query.trim()}
            tone={skinTone}
            onPickEmoji={pickEmoji}
            onPickImage={(image) => pickImage(image, false)}
            onHover={setHovered}
          />
        ) : (
          <>
            {recent.length > 0 && (
              <Block title="recent">
                {/* Recents are stored exactly as they were sent — already
                    toned, and already a shortcode where one was used. */}
                <div style={GRID}>
                  {recent.map((key) => (
                    <RecentCell
                      key={key}
                      value={key}
                      lookup={known}
                      onClick={() => pickRecent(key)}
                      onHover={setHovered}
                    />
                  ))}
                </div>
              </Block>
            )}

            {sections.map((section, i) => (
              <Block key={section.key} title={section.title} index={i}>
                {"emojis" in section ? (
                  <EmojiGrid
                    emojis={section.emojis}
                    tone={skinTone}
                    onPick={pickEmoji}
                    onHover={setHovered}
                  />
                ) : (
                  <ImageGrid
                    images={section.images}
                    sticker={section.sticker}
                    onPick={(image) => pickImage(image, section.sticker)}
                    onHover={setHovered}
                  />
                )}
              </Block>
            ))}
          </>
        )}
      </div>

      <Footer
        hovered={hovered}
        skinTone={skinTone}
        onTone={(tone) => updateSettings({ skinTone: tone })}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// pieces
// ---------------------------------------------------------------------------

const GRID: React.CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(8, 1fr)",
  gap: space(1),
};

function SearchResults({
  results,
  query,
  tone,
  onPickEmoji,
  onPickImage,
  onHover,
}: {
  results: { emotes: PackImage[]; emojis: Emoji[] };
  query: string;
  tone: SkinTone;
  onPickEmoji: (emoji: Emoji) => void;
  onPickImage: (image: PackImage) => void;
  onHover: (picked: Picked) => void;
}) {
  const { emotes, emojis } = results;

  if (emotes.length === 0 && emojis.length === 0) {
    return (
      <div
        style={{
          padding: space("28px 12px"),
          textAlign: "center",
          fontSize: textSize(12.5),
          color: "var(--text-tertiary)",
        }}
      >
        nothing matches "{query}"
      </div>
    );
  }

  return (
    <>
      {emotes.length > 0 && (
        <Block title="your packs">
          <ImageGrid images={emotes} sticker={false} onPick={onPickImage} onHover={onHover} />
        </Block>
      )}
      {emojis.length > 0 && (
        <Block title={`${emojis.length} emoji`}>
          <EmojiGrid emojis={emojis} tone={tone} onPick={onPickEmoji} onHover={onHover} />
        </Block>
      )}
    </>
  );
}

function Block({
  title,
  index,
  children,
}: {
  title: string;
  /** Marks the scroll target for the category rail. */
  index?: number;
  children: React.ReactNode;
}) {
  return (
    <div data-group={index}>
      <div
        className="uwu-ellipsis"
        style={{
          position: "sticky",
          top: 0,
          zIndex: 1,
          padding: space("6px 4px 4px"),
          fontFamily: "var(--font-mono)",
          fontSize: textSize(10),
          letterSpacing: ".08em",
          textTransform: "uppercase",
          color: "var(--text-tertiary)",
          background: "var(--surface-card-raised)",
        }}
      >
        {title}
      </div>
      {children}
    </div>
  );
}

function EmojiGrid({
  emojis,
  tone,
  onPick,
  onHover,
}: {
  emojis: Emoji[];
  tone: SkinTone;
  onPick: (emoji: Emoji) => void;
  onHover: (picked: Picked) => void;
}) {
  return (
    <div style={GRID}>
      {emojis.map((emoji) => (
        <Cell
          key={emoji.emoji}
          label={emoji.name}
          onClick={() => onPick(emoji)}
          // The preview names the base emoji, so hovering a toned hand still
          // says "waving hand" rather than falling back to the character.
          onHover={() => onHover({ kind: "unicode", emoji: emoji.emoji })}
        >
          {emoji.skinToneSupport ? withSkinTone(emoji.emoji, tone) : emoji.emoji}
        </Cell>
      ))}
    </div>
  );
}

function ImageGrid({
  images,
  sticker,
  onPick,
  onHover,
}: {
  images: PackImage[];
  sticker: boolean;
  onPick: (image: PackImage) => void;
  onHover: (picked: Picked) => void;
}) {
  return (
    <div style={GRID}>
      {images.map((image) => (
        <Cell
          key={`${image.shortcode}-${image.url}`}
          label={image.shortcode}
          onClick={() => onPick(image)}
          onHover={() => onHover({ kind: sticker ? "sticker" : "emote", image })}
        >
          <PackImg image={image} />
        </Cell>
      ))}
    </div>
  );
}

/** One pack image, or its shortcode if the media can't be addressed. */
function PackImg({ image, size = CELL_IMAGE }: { image: PackImage; size?: number }) {
  return <RemoteImg url={image.url} label={image.shortcode} size={size} />;
}

function RemoteImg({
  url,
  label,
  size = CELL_IMAGE,
}: {
  url: string;
  label: string;
  size?: number;
}) {
  const src = mediaUrl(url, { width: size * 2, height: size * 2 });
  if (!src) return <span style={{ fontSize: textSize(9) }}>{label}</span>;

  return (
    <img
      src={src}
      alt={label}
      loading="lazy"
      draggable={false}
      style={{ width: size, height: size, objectFit: "contain" }}
    />
  );
}

/**
 * One remembered reaction.
 *
 * Drawn as a picture whenever it stands for one — including an `mxc://` key
 * from a client that reacts that way, which has no shortcode to look up but is
 * still perfectly showable.
 */
function RecentCell({
  value,
  lookup,
  onClick,
  onHover,
}: {
  value: string;
  lookup: Map<string, PackImage>;
  onClick: () => void;
  onHover: (picked: Picked) => void;
}) {
  const code = /^:([^\s:]+):$/.exec(value)?.[1];
  const known = code ? lookup.get(code) : undefined;
  const shown = reactionImage(value, lookup);

  return (
    <Cell
      // Named like the same emote in its pack below, so a search for it and a
      // hover over it agree.
      label={known ? known.shortcode : (shown?.label ?? value)}
      onClick={onClick}
      onHover={() =>
        onHover(known ? { kind: "emote", image: known } : { kind: "unicode", emoji: value })
      }
    >
      {shown ? <RemoteImg url={shown.url} label={shown.label} /> : value}
    </Cell>
  );
}

function Cell({
  label,
  onClick,
  onHover,
  children,
}: {
  label: string;
  onClick: () => void;
  onHover: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      onMouseEnter={(e) => {
        onHover();
        e.currentTarget.style.background = "rgba(255,255,255,.08)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = "transparent";
      }}
      title={label}
      aria-label={label}
      style={{
        height: 34,
        borderRadius: corner(8),
        fontSize: textSize(20),
        lineHeight: 1,
        cursor: "pointer",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        // An image that fails to load falls back to its alt text, which is
        // wider than the cell; clip it rather than letting it push the grid out.
        overflow: "hidden",
      }}
    >
      {children}
    </button>
  );
}

function Footer({
  hovered,
  skinTone,
  onTone,
}: {
  hovered: Picked | null;
  skinTone: SkinTone;
  onTone: (tone: SkinTone) => void;
}) {
  const label =
    hovered === null
      ? "pick something~"
      : hovered.kind === "unicode"
        ? (emojiName(hovered.emoji) ?? hovered.emoji)
        : `:${hovered.image.shortcode}:`;

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(8),
        padding: space("7px 12px"),
        borderTop: "1px solid var(--border-subtle)",
        background: "var(--surface-card)",
      }}
    >
      <span
        style={{
          width: 20,
          fontSize: textSize(17),
          lineHeight: 1,
          display: "flex",
          justifyContent: "center",
        }}
      >
        {hovered === null ? (
          "🫧"
        ) : hovered.kind === "unicode" ? (
          hovered.emoji
        ) : (
          <PackImg image={hovered.image} size={19} />
        )}
      </span>
      <span
        className="uwu-ellipsis"
        style={{ flex: 1, fontSize: textSize(11.5), color: "var(--text-secondary)" }}
      >
        {label}
      </span>

      <div style={{ display: "flex", gap: space(1) }}>
        {TONE_SAMPLES.map((sample, tone) => (
          <button
            key={tone}
            onClick={() => onTone(tone as SkinTone)}
            title={tone === 0 ? "default skin tone" : `skin tone ${tone}`}
            // The swatch is a hand, so without a label the button announces
            // itself as "waving hand" rather than as the tone it sets.
            aria-label={tone === 0 ? "default skin tone" : `skin tone ${tone}`}
            aria-pressed={skinTone === tone}
            style={{
              width: 20,
              height: 20,
              borderRadius: corner(6),
              fontSize: textSize(12),
              lineHeight: 1,
              cursor: "pointer",
              background:
                skinTone === tone
                  ? "color-mix(in srgb, var(--accent-primary) 22%, transparent)"
                  : "transparent",
            }}
          >
            {sample}
          </button>
        ))}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// placement
// ---------------------------------------------------------------------------

/**
 * Place the picker beside its anchor, flipping and sliding to stay on screen.
 *
 * Measured in a layout effect so the first paint is already in the right place
 * — a popover that visibly jumps after opening reads as a bug.
 */
function usePlacement(anchor: DOMRect) {
  const [placement, setPlacement] = useState<{ left: number; top: number }>(() =>
    place(anchor),
  );

  useLayoutEffect(() => {
    setPlacement(place(anchor));
  }, [anchor]);

  return placement;
}

function place(anchor: DOMRect): { left: number; top: number } {
  // Prefer below-right of the anchor, which is where the hover bar's button is.
  const left = clamp(anchor.right - WIDTH, MARGIN, window.innerWidth - WIDTH - MARGIN);

  const below = anchor.bottom + 6;
  const top =
    below + HEIGHT + MARGIN <= window.innerHeight
      ? below
      : Math.max(MARGIN, anchor.top - HEIGHT - 6);

  return { left, top };
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), Math.max(min, max));
}
