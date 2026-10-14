import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { uploadPaths } from "../lib/upload";
import { selectActiveRoom, useStore } from "../store";
import { Icon } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

/**
 * The "drop it here" overlay, and the drop itself.
 *
 * The window has `dragDropEnabled`, which means the OS hands dragged files to
 * Tauri and the WebView never sees an HTML5 `dragover` — so this listens to the
 * webview's own drag events instead of the DOM's. The upside is that a drop
 * arrives as real filesystem *paths*, which Rust can read directly without
 * anything crossing the IPC bridge.
 */
export function DropZone() {
  const [dragging, setDragging] = useState(false);
  const { activeThreadRoot } = useStore(
    useShallow((s) => ({ activeThreadRoot: s.activeThreadRoot })),
  );
  const room = useStore(selectActiveRoom);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        switch (event.payload.type) {
          case "enter":
          case "over":
            setDragging(true);
            break;

          case "drop": {
            setDragging(false);
            // Read the room at drop time, not at subscribe time — this
            // subscription outlives any particular room being open.
            const { activeRoomId, activeThreadRoot: thread } = useStore.getState();
            if (!activeRoomId) return;
            void uploadPaths(event.payload.paths, activeRoomId, thread ?? undefined);
            break;
          }

          default:
            setDragging(false);
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (!dragging) return null;

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 300,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: space(28),
        background: "color-mix(in srgb, var(--ink-950) 78%, transparent)",
        // Purely decorative: the OS owns the drag, and a pointer-events target
        // here would only get in its way.
        pointerEvents: "none",
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: space(14),
          padding: space("44px 64px"),
          borderRadius: corner(28),
          border: `2px dashed ${room ? "var(--accent-primary)" : "var(--border-strong)"}`,
          background: "var(--surface-card-raised)",
          boxShadow: "var(--shadow-pop)",
          transform: "rotate(calc(-1.5deg * var(--tilt-scale)))",
          textAlign: "center",
        }}
      >
        <Icon
          name={room ? "upload-simple" : "warning-circle"}
          size={40}
          color={room ? "var(--accent-primary)" : "var(--text-tertiary)"}
        />
        <div style={{ fontFamily: "var(--font-heading)", fontWeight: 800, fontSize: headingSize(22) }}>
          {room ? "drop it here~" : "open a room first~"}
        </div>
        <div style={{ fontSize: textSize(13), color: "var(--text-secondary)", maxWidth: 320 }}>
          {room
            ? activeThreadRoot
              ? `it'll go to the thread you have open in ${room.name}`
              : `it'll go straight to ${room.name}`
            : "there's nowhere to put a file until you pick a room"}
        </div>
      </div>
    </div>
  );
}
