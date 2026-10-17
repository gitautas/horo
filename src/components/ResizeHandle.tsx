import { useEffect, useRef, useState } from "react";
import type React from "react";

import { useStore } from "../store";
import { DEFAULTS } from "../lib/settings";
import { dragOff } from "./ui";

type WidthKey = "roomListWidth" | "infoPanelWidth";

/** How much one arrow-key press moves the edge, for keyboard users. */
const KEY_STEP = 16;

/** The grabbable strip, centred on the border between two panes. */
const HIT_WIDTH = 8;

/**
 * A desktop sidebar whose width the user can drag, the way Discord's are.
 *
 * Returns the width to render at and the handle to put on the pane's edge.
 * While dragging, the width lives in local state and only reaches the store —
 * and `localStorage` — on release, so a drag is one write rather than one per
 * pointer move.
 *
 * `maxRoom` is how much the pane may take right now, read at the start of each
 * drag: the static ceiling in `range` doesn't know the window is narrow or that
 * the other sidebar is already wide, and the timeline needs what's left.
 */
export function useResizablePane({
  key,
  range,
  edge,
  maxRoom,
}: {
  key: WidthKey;
  range: { min: number; max: number };
  /** Which side of the pane the handle sits on. */
  edge: "left" | "right";
  maxRoom: () => number;
}): [number, React.ReactNode] {
  const saved = useStore((s) => s.settings[key]);
  const updateSettings = useStore((s) => s.updateSettings);
  const [live, setLive] = useState<number | null>(null);
  const drag = useRef<{ startX: number; startWidth: number; max: number } | null>(null);

  const clamp = (w: number, max = Math.min(range.max, maxRoom())) =>
    Math.round(Math.max(range.min, Math.min(Math.max(range.min, max), w)));

  const width = live ?? clamp(saved, range.max);
  const commit = (w: number) => updateSettings({ [key]: w });

  // Hold the resize cursor and stop text selection across the whole window
  // while dragging, not just over the strip the pointer has long since left.
  const dragging = live !== null;
  useEffect(() => {
    if (!dragging) return;
    const body = document.body.style;
    const before = { cursor: body.cursor, userSelect: body.userSelect };
    body.cursor = "col-resize";
    body.userSelect = "none";
    return () => {
      body.cursor = before.cursor;
      body.userSelect = before.userSelect;
    };
  }, [dragging]);

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = {
      startX: e.clientX,
      startWidth: width,
      max: Math.min(range.max, maxRoom()),
    };
    setLive(width);
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.startX;
    setLive(clamp(d.startWidth + (edge === "right" ? dx : -dx), d.max));
  };

  const onPointerUp = () => {
    if (!drag.current) return;
    drag.current = null;
    if (live !== null) commit(live);
    setLive(null);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const grow = edge === "right" ? "ArrowRight" : "ArrowLeft";
    const shrink = edge === "right" ? "ArrowLeft" : "ArrowRight";
    if (e.key === grow) commit(clamp(width + KEY_STEP));
    else if (e.key === shrink) commit(clamp(width - KEY_STEP));
    else if (e.key === "Home") commit(range.min);
    else if (e.key === "End") commit(clamp(range.max));
    else return;
    e.preventDefault();
  };

  const handle = (
    <div
      {...dragOff}
      role="separator"
      aria-orientation="vertical"
      aria-label="resize sidebar"
      aria-valuemin={range.min}
      aria-valuemax={range.max}
      aria-valuenow={width}
      tabIndex={0}
      title="drag to resize, double-click to reset"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={() => commit(clamp(DEFAULTS[key]))}
      onKeyDown={onKeyDown}
      className={`${dragOff.className} uwu-resize${dragging ? " is-dragging" : ""}`}
      style={{
        position: "absolute",
        top: 0,
        bottom: 0,
        [edge]: -HIT_WIDTH / 2,
        width: HIT_WIDTH,
        zIndex: 5,
        cursor: "col-resize",
        touchAction: "none",
      }}
    />
  );

  return [width, handle];
}
