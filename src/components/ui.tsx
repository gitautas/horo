/** The small shared pieces the design repeats everywhere. */

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import type { CSSProperties, ReactNode } from "react";

import { avatarFailed, cachedAvatar, loadAvatar } from "../lib/avatarCache";
import { accentFor, initialsFor } from "../lib/display";
import { mediaUrl } from "../lib/ipc";
import logoUrl from "../../src-tauri/icons/horo_logo.svg";
import { corner, headingSize, labelText, space, textSize } from "../lib/scale";

/**
 * The window has no title bar on macOS, so parts of the chrome double as its
 * drag handle.
 *
 * `-webkit-app-region` — what the `uwu-drag` class sets — is a Chromium
 * property, and WKWebView ignores it, so on macOS it drags nothing. Tauri
 * reads this data attribute on mousedown instead. `deep` means anywhere in the
 * subtree drags; Tauri's own handler still lets buttons, links and inputs
 * through, and `dragOff` opts the rest of a subtree back out.
 *
 * Desktop only: iOS synthesises a mousedown from a tap, and the handler calls
 * `preventDefault` on it, which swallows the tap that would have followed.
 */
export function dragRegion(desktop: boolean) {
  return desktop ? ({ className: "uwu-drag", "data-tauri-drag-region": "deep" } as const) : {};
}

/** Opts a subtree out of an enclosing {@link dragRegion}. */
export const dragOff = { className: "uwu-no-drag", "data-tauri-drag-region": "false" } as const;

export function Icon({
  name,
  size = 16,
  color = "var(--text-secondary)",
  style,
}: {
  name: string;
  size?: number;
  color?: string;
  style?: CSSProperties;
}) {
  return (
    <i
      className={`ph-fill ph-${name}`}
      aria-hidden
      style={{
        fontSize: size,
        color,
        lineHeight: 1,
        // An icon is a fixed-size glyph, never a thing to squeeze. As a flex
        // item it would otherwise shrink like any other inline content — so an
        // icon sitting beside a long label (a reply quote, the encryption
        // notice, a system event) would quietly compress to nothing while the
        // text beside it stayed. Callers can still override via `style`.
        flex: "none",
        ...style,
      }}
    />
  );
}

/**
 * The one size every avatar is fetched at, in CSS pixels before the retina
 * multiplier. Comfortably above the largest avatar the design draws (76px, in
 * the room header), so nothing is ever upscaled.
 */
const AVATAR_FETCH_SIZE = 88;

/**
 * The design's signature avatar: a chunky rounded tile with an ink border,
 * showing the real image when there is one and a neon monogram when there
 * isn't.
 */
export function Avatar({
  id,
  name,
  mxc,
  size = 38,
  radius,
  fontSize,
  style,
}: {
  id: string;
  name: string;
  mxc?: string | null;
  size?: number;
  radius?: number;
  fontSize?: number;
  style?: CSSProperties;
}) {
  // Every avatar in the app asks for one size, whatever it's drawn at.
  //
  // The size is part of the URL and therefore the cache key, so a 38px timeline
  // avatar and a 66px card avatar would otherwise be two fetches of the same
  // picture — and on a bad day the second one is a ten-second federation stall
  // that ends in a monogram while the first renders fine. One bucket, scaled by
  // CSS, means an avatar is fetched once and then always instant.
  const src = mediaUrl(mxc, { width: AVATAR_FETCH_SIZE, height: AVATAR_FETCH_SIZE });

  // The monogram is not just the no-avatar case: it's also what's shown while
  // the image loads and if it never arrives. Remote avatars can take seconds or
  // fail outright (see ARCHITECTURE.md on thumbnails), and the alternative is
  // the WebView's broken-image glyph, which looks like a bug in us.
  //
  // A picture already in the avatar cache is drawn on the first frame, with no
  // monogram and no fade — that's every avatar after the first time it's seen.
  // `fetched` remembers which src a load finished for, so a src that changes
  // underneath an in-flight load never shows the old picture.
  const [fetched, setFetched] = useState<{ src: string; url: string }>();
  const url = src
    ? (cachedAvatar(src) ?? (fetched?.src === src ? fetched.url : undefined))
    : undefined;
  const fadeIn = !!url && fetched?.src === src;

  useEffect(() => {
    if (!src || cachedAvatar(src) || avatarFailed(src)) return;
    let cancelled = false;
    loadAvatar(src)
      .then((loaded) => !cancelled && setFetched({ src, url: loaded }))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [src]);

  return (
    <div
      style={{
        position: "relative",
        overflow: "hidden",
        width: size,
        height: size,
        flex: "none",
        borderRadius: corner(radius ?? Math.round(size * 0.36)),
        border: "2px solid var(--ink-950)",
        background: url ? "var(--ink-900)" : accentFor(id),
        color: "var(--text-on-accent)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        fontFamily: "var(--font-display)",
        fontWeight: 800,
        fontSize: fontSize ?? Math.max(10, Math.round(size * 0.37)),
        userSelect: "none",
        ...style,
      }}
    >
      {url ? (
        <img
          src={url}
          alt=""
          draggable={false}
          style={{
            position: "absolute",
            inset: 0,
            width: "100%",
            height: "100%",
            objectFit: "cover",
            animation: fadeIn ? "uwuFadeIn var(--dur-fast) var(--ease-out)" : undefined,
          }}
        />
      ) : (
        initialsFor(name)
      )}
    </div>
  );
}

/**
 * A small corner badge marking what kind of room this is: `#` for a text
 * channel, a camera for a video room. DMs get nothing — a person isn't a
 * channel, and the avatar already says who they are.
 */
export function ChannelBadge({
  kind,
  size = 16,
  ring = "var(--ink-900)",
}: {
  kind: "text" | "video";
  size?: number;
  ring?: string;
}) {
  const video = kind === "video";
  return (
    <div
      title={video ? "video room" : "text room"}
      style={{
        position: "absolute",
        left: -4,
        bottom: -4,
        width: size,
        height: size,
        borderRadius: "50%",
        background: video ? "var(--accent-tertiary)" : "var(--ink-700)",
        border: `2px solid ${ring}`,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        pointerEvents: "none",
      }}
    >
      {video ? (
        <Icon name="video-camera" size={size * 0.5} color="var(--ink-950)" />
      ) : (
        <span
          style={{
            fontFamily: "var(--font-display)",
            fontWeight: 800,
            fontSize: size * 0.62,
            lineHeight: 1,
            color: "var(--text-secondary)",
          }}
        >
          #
        </span>
      )}
    </div>
  );
}

export function PresenceDot({
  colour = "var(--status-online)",
  size = 12,
  ring = "var(--ink-900)",
}: {
  colour?: string;
  size?: number;
  ring?: string;
}) {
  return (
    <div
      style={{
        position: "absolute",
        right: -3,
        bottom: -3,
        width: size,
        height: size,
        borderRadius: "50%",
        background: colour,
        border: `2px solid ${ring}`,
      }}
    />
  );
}

/**
 * A switch. Lives here rather than in a view because there used to be two of
 * these, drawn differently, in two files.
 */
export function Toggle({
  on,
  onToggle,
  label,
}: {
  on: boolean;
  onToggle: (next: boolean) => void;
  label?: string;
}) {
  return (
    <button
      type="button"
      onClick={() => onToggle(!on)}
      role="switch"
      aria-checked={on}
      aria-label={label}
      style={{
        width: 38,
        height: 22,
        flex: "none",
        borderRadius: 999,
        background: on ? "var(--accent-primary)" : "var(--surface-inset)",
        border: "1px solid var(--border-subtle)",
        position: "relative",
        cursor: "pointer",
      }}
    >
      <div
        style={{
          position: "absolute",
          top: 2,
          left: on ? 18 : 2,
          width: 16,
          height: 16,
          borderRadius: "50%",
          background: on ? "var(--ink-950)" : "var(--text-tertiary)",
          transition: "left var(--dur-fast) var(--ease-bounce)",
        }}
      />
    </button>
  );
}

/** The section label used throughout the design — uppercase and letter-spaced under classic. */
export function RaveLabel({
  children,
  style,
}: {
  children: ReactNode;
  style?: CSSProperties;
}) {
  return (
    <div
      style={{
        ...labelText(9.5),
        color: "var(--text-tertiary)",
        ...style,
      }}
    >
      {children}
    </div>
  );
}

export function Button({
  children,
  onClick,
  variant = "primary",
  size = "md",
  disabled,
  type = "button",
  style,
}: {
  children: ReactNode;
  onClick?: () => void;
  variant?: "primary" | "ghost" | "danger";
  size?: "sm" | "md";
  disabled?: boolean;
  type?: "button" | "submit";
  style?: CSSProperties;
}) {
  const palette = {
    primary: {
      background: "var(--accent-primary)",
      color: "var(--text-on-accent)",
      border: "2px solid var(--ink-950)",
      boxShadow: "var(--shadow-sticker-ink)",
    },
    ghost: {
      background: "transparent",
      color: "var(--text-secondary)",
      border: "1px solid var(--border-default)",
    },
    danger: {
      background: "var(--status-danger)",
      color: "var(--ink-950)",
      border: "2px solid var(--ink-950)",
    },
  }[variant];

  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled}
      style={{
        padding: space(size === "sm" ? "7px 15px" : "10px 20px"),
        borderRadius: 999,
        fontFamily: "var(--font-display)",
        fontWeight: 700,
        fontSize: textSize(size === "sm" ? 13 : 14.5),
        cursor: disabled ? "not-allowed" : "pointer",
        opacity: disabled ? 0.45 : 1,
        transition: "transform var(--dur-fast) var(--ease-bounce)",
        whiteSpace: "nowrap",
        ...palette,
        ...style,
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
      {children}
    </button>
  );
}

/** A small pill used for the e2e badge, bridge tags and role labels. */
export function Tag({
  children,
  colour = "var(--accent-primary)",
  icon,
}: {
  children: ReactNode;
  colour?: string;
  icon?: string;
}) {
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: space(5),
        padding: space("3px 9px"),
        borderRadius: 999,
        border: `1px solid color-mix(in srgb, ${colour} 40%, transparent)`,
        background: `color-mix(in srgb, ${colour} 12%, transparent)`,
        color: colour,
        fontFamily: "var(--font-rave)",
        fontSize: textSize(9),
        fontWeight: 800,
        letterSpacing: "0.1em",
        textTransform: "uppercase",
        whiteSpace: "nowrap",
      }}
    >
      {icon && <Icon name={icon} size={11} color={colour} />}
      {children}
    </span>
  );
}

/**
 * A centred dialog over a dimmed app: backdrop click and Escape both close it.
 *
 * Here rather than in a view because there are several of these now, and the
 * third copy of a pattern is where DESIGN.md says it stops being a view's
 * business.
 */
export function Modal({
  title,
  onClose,
  width = 420,
  children,
}: {
  title: string;
  onClose: () => void;
  width?: number;
  children: ReactNode;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return createPortal(
    <div
      onClick={onClose}
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 180,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: space(24),
        background: "color-mix(in srgb, var(--ink-950) 70%, transparent)",
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="uwu-scroll"
        style={{
          width,
          maxWidth: "100%",
          maxHeight: "calc(100vh - 48px)",
          overflowY: "auto",
          padding: space(24),
          borderRadius: corner(24),
          background: "var(--surface-card-raised)",
          border: "1px solid var(--border-default)",
          boxShadow: "var(--shadow-pop)",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: space(10), marginBottom: 18 }}>
          <div
            style={{
              fontFamily: "var(--font-heading)",
              fontWeight: 800,
              fontSize: headingSize(20),
              flex: 1,
            }}
          >
            {title}
          </div>
          <button
            onClick={onClose}
            aria-label="close"
            style={{ cursor: "pointer", display: "flex" }}
          >
            <Icon name="x" size={15} color="var(--text-tertiary)" />
          </button>
        </div>

        {children}
      </div>
    </div>,
    document.body,
  );
}

/** The full-bleed ray pattern behind the whole app. */
/**
 * The horo wordmark, sized by height. It keeps its own cream and gold rather
 * than following the accent, since it is the brand mark.
 */
export function Logo({ height, style }: { height: number; style?: CSSProperties }) {
  return (
    <img
      src={logoUrl}
      alt="horo"
      draggable={false}
      style={{ display: "block", height, width: "auto", ...style }}
    />
  );
}

export function BackdropPattern() {
  return (
    <div className="uwu-pattern">
      <div className="uwu-rays" />
    </div>
  );
}

export function Spinner({ size = 16, color = "var(--accent-primary)" }) {
  return (
    <div
      style={{
        width: size,
        height: size,
        borderRadius: "50%",
        border: `2px solid color-mix(in srgb, ${color} 25%, transparent)`,
        borderTopColor: color,
        animation: "uwuSpin 0.7s linear infinite",
      }}
    />
  );
}

/** A row that dims and highlights on hover, used by list items. */
export function HoverRow({
  children,
  onClick,
  active,
  style,
  title,
}: {
  children: ReactNode;
  onClick?: () => void;
  active?: boolean;
  style?: CSSProperties;
  title?: string;
}) {
  return (
    <div
      onClick={onClick}
      title={title}
      style={{
        cursor: onClick ? "pointer" : "default",
        background: active ? "rgba(255,255,255,.07)" : "transparent",
        transition: "background var(--dur-fast) var(--ease-out)",
        ...style,
      }}
      onMouseEnter={(e) => {
        if (!active) e.currentTarget.style.background = "rgba(255,255,255,.05)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = active ? "rgba(255,255,255,.07)" : "transparent";
      }}
    >
      {children}
    </div>
  );
}
