import { useEffect, useState } from "react";
import { createPortal } from "react-dom";

import { saveAttachment } from "../lib/download";
import { mediaUrl } from "../lib/ipc";
import { useStore } from "../store";
import { Icon, Spinner } from "./ui";
import { corner, space, textSize } from "../lib/scale";

/**
 * A picture, full size, over everything else.
 *
 * Mounted once from the shell; what it shows lives in the store, the same way
 * the profile card works. Deliberately one image at a time — an original-size
 * animated GIF decodes at original size, and a grid of those is how you make a
 * chat client stutter.
 */
export function Lightbox() {
  const picture = useStore((s) => s.lightbox);
  if (!picture) return null;
  return <Frame key={picture.mxc} mxc={picture.mxc} name={picture.name} />;
}

function Frame({ mxc, name }: { mxc: string; name: string }) {
  const close = useStore((s) => s.closeLightbox);
  const showBanner = useStore((s) => s.showBanner);
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);

  // No width or height: this is the one place that wants the original file
  // rather than a thumbnail, and a size here would also mint another entry in
  // the media cache for a picture we already hold at some other size.
  const src = mediaUrl(mxc);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close]);

  async function download() {
    setSaving(true);
    try {
      await saveAttachment(mxc, name);
    } finally {
      setSaving(false);
    }
  }

  return createPortal(
    <div
      onClick={close}
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 200,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: space(14),
        padding: space(40),
        background: "color-mix(in srgb, var(--ink-950) 86%, transparent)",
        cursor: "zoom-out",
      }}
    >
      <div
        style={{
          position: "absolute",
          top: 18,
          right: 18,
          display: "flex",
          gap: space(8),
          // The controls sit on the backdrop, which closes on click.
          cursor: "default",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <ControlButton
          icon={saving ? "hourglass" : "download-simple"}
          label="save this picture"
          onClick={() => void download()}
        />
        <ControlButton icon="x" label="close" onClick={close} />
      </div>

      {!loaded && <Spinner size={22} />}

      {src && (
        <img
          src={src}
          alt={name}
          onLoad={() => setLoaded(true)}
          onError={() => {
            showBanner("error", "couldn't load that one at full size");
            close();
          }}
          onClick={(e) => e.stopPropagation()}
          style={{
            maxWidth: "100%",
            maxHeight: "100%",
            objectFit: "contain",
            borderRadius: corner(16),
            border: "1px solid var(--border-subtle)",
            boxShadow: "var(--shadow-pop)",
            cursor: "default",
            display: loaded ? "block" : "none",
          }}
        />
      )}

      {loaded && (
        <div
          className="selectable"
          onClick={(e) => e.stopPropagation()}
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(11.5),
            color: "var(--text-tertiary)",
            cursor: "default",
            maxWidth: "80%",
            textAlign: "center",
            wordBreak: "break-all",
          }}
        >
          {name}
        </div>
      )}
    </div>,
    document.body,
  );
}

function ControlButton({
  icon,
  label,
  onClick,
}: {
  icon: string;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      title={label}
      aria-label={label}
      style={{
        width: 38,
        height: 38,
        borderRadius: 999,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
        background: "var(--surface-card-raised)",
        border: "1px solid var(--border-default)",
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.background = "var(--surface-card)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = "var(--surface-card-raised)";
      }}
    >
      <Icon name={icon} size={16} color="var(--text-secondary)" />
    </button>
  );
}
