/**
 * Picking something to send, on a phone.
 *
 * The system file picker is one tap away from a document browser, which is the
 * long way round to the photo you took a minute ago. This puts the recent
 * library in front of you instead: tap a tile and it sends.
 *
 * The grid needs native help — see `photos.rs` — so everything here degrades to
 * the system picker when there is no library to read, which is every desktop
 * platform and iOS before permission is granted.
 */
import { useEffect, useRef, useState } from "react";

import * as ipc from "../lib/ipc";
import type { Photo } from "../lib/types";
import { uploadPaths } from "../lib/upload";
import { useStore } from "../store";
import { Icon, Spinner } from "./ui";

/** How many tiles to ask for. Two screens' worth on a phone. */
const RECENT_LIMIT = 36;

export function AttachSheet({
  roomId,
  threadRoot,
  onClose,
  onCamera,
  onBrowse,
}: {
  roomId: string;
  threadRoot?: string;
  onClose: () => void;
  /** Straight to the camera, skipping every intermediate sheet. */
  onCamera: () => void;
  /** The system picker, for everything that isn't a recent photo. */
  onBrowse: () => void;
}) {
  const showBanner = useStore((s) => s.showBanner);
  const [photos, setPhotos] = useState<Photo[] | null>(null);
  const [denied, setDenied] = useState(false);
  const [limited, setLimited] = useState(false);
  /** The tile being exported, so it can show it's working. */
  const [sending, setSending] = useState<string | null>(null);

  /**
   * The callbacks, kept where the fetch can reach them without depending on
   * them.
   *
   * They arrive as inline arrows, so they are new objects on every render of
   * the composer — and the composer re-renders on every keystroke. Listing them
   * as effect dependencies re-ran the fetch each time, and each run asks the
   * photo library to encode `RECENT_LIMIT` JPEG thumbnails. The fetch belongs
   * to *mounting the sheet*, not to whatever the composer is doing.
   */
  const latest = useRef({ onBrowse, onClose });
  latest.current = { onBrowse, onClose };

  useEffect(() => {
    let live = true;
    ipc
      .photosRecent(RECENT_LIMIT)
      .then((result) => {
        if (!live) return;
        // Nothing to show and nothing to ask for: let the system picker be the
        // whole answer rather than drawing an empty grid above it.
        if (!result.supported) {
          latest.current.onBrowse();
          latest.current.onClose();
          return;
        }
        setDenied(result.denied);
        setLimited(result.limited);
        setPhotos(result.photos);
      })
      .catch((e) => {
        if (!live) return;
        showBanner("error", ipc.asUwuError(e).message);
        setPhotos([]);
      });
    return () => {
      live = false;
    };
    // Once, when the sheet opens. See `latest` above.
  }, [showBanner]);

  async function send(photo: Photo) {
    setSending(photo.id);
    try {
      // Export first, then hand the path to the same uploader the file picker
      // and drag-and-drop use — one upload path, one set of limits and errors.
      const path = await ipc.photosExport(photo.id);
      await uploadPaths([path], roomId, threadRoot);
      onClose();
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
      setSending(null);
    }
  }

  return (
    <div
      onClick={onClose}
      role="presentation"
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 150,
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
          padding: "10px 10px calc(var(--safe-bottom) + 10px)",
          boxShadow: "var(--shadow-pop)",
        }}
      >
        <div
          className="uwu-scroll"
          style={{
            maxHeight: "42vh",
            display: "grid",
            gridTemplateColumns: "repeat(4, 1fr)",
            gap: 4,
            marginBottom: 8,
          }}
        >
          {photos === null && (
            <div style={{ gridColumn: "1 / -1", display: "flex", justifyContent: "center", padding: 24 }}>
              <Spinner size={18} />
            </div>
          )}

          {denied && (
            <div
              style={{
                gridColumn: "1 / -1",
                padding: "14px 6px",
                fontSize: 13,
                lineHeight: 1.5,
                color: "var(--text-secondary)",
              }}
            >
              horo can't see your photos. you can turn that on in Settings →
              Privacy &amp; Security → Photos, or pick a file below instead.
            </div>
          )}

          {photos?.map((photo) => (
            <button
              key={photo.id}
              onClick={() => void send(photo)}
              disabled={sending !== null}
              aria-label={photo.video ? "send this video" : "send this photo"}
              style={{
                position: "relative",
                aspectRatio: "1 / 1",
                padding: 0,
                borderRadius: 10,
                overflow: "hidden",
                cursor: "pointer",
                background: "var(--surface-card)",
                opacity: sending && sending !== photo.id ? 0.4 : 1,
              }}
            >
              <img
                src={photo.thumb}
                alt=""
                style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }}
              />
              {photo.video && (
                <span
                  style={{
                    position: "absolute",
                    right: 4,
                    bottom: 3,
                    fontFamily: "var(--font-mono)",
                    fontSize: 10,
                    color: "var(--text-primary)",
                    textShadow: "0 1px 3px rgba(0,0,0,.9)",
                  }}
                >
                  {formatDuration(photo.seconds)}
                </span>
              )}
              {sending === photo.id && (
                <span
                  style={{
                    position: "absolute",
                    inset: 0,
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    background: "rgba(0,0,0,.45)",
                  }}
                >
                  <Spinner size={16} />
                </span>
              )}
            </button>
          ))}
        </div>

        {limited && (
          <div
            style={{
              padding: "0 6px 8px",
              fontSize: 11.5,
              lineHeight: 1.45,
              color: "var(--text-tertiary)",
            }}
          >
            showing the photos you chose to share. Settings → Privacy &amp; Security →
            Photos lets horo see more.
          </div>
        )}

        <SheetRow icon="camera" label="take a photo or video" onClick={onCamera} />
        <SheetRow icon="folder-open" label="choose a file" onClick={onBrowse} />
      </div>
    </div>
  );
}

function SheetRow({
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
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        width: "100%",
        height: 48,
        padding: "0 12px",
        borderRadius: 14,
        cursor: "pointer",
        textAlign: "left",
        fontFamily: "var(--font-display)",
        fontWeight: 700,
        fontSize: 15,
        color: "var(--text-primary)",
      }}
    >
      <Icon name={icon} size={18} color="var(--text-secondary)" />
      {label}
    </button>
  );
}

/** `m:ss`, which is all a phone video ever needs. */
function formatDuration(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}
