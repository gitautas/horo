/**
 * Telling a cut-out apart from a photo.
 *
 * A transparent PNG pasted into a room is nearly always meant as a sticker — a
 * character, a reaction, something cropped out of its background — and drawing
 * it on a card with a border puts back the very box it was cut out of. So an
 * image that's mostly see-through is shown frameless, the way a sticker is.
 *
 * Only the pixels can say. The format can't (a PNG screenshot is opaque), and
 * neither can an alpha channel (plenty of opaque PNGs carry one anyway). And
 * since an `<img>` from `horo://` is cross-origin and would taint a canvas, the
 * bytes come over IPC — the same thumbnail the timeline shows, so it's a
 * cache hit rather than a second download.
 */

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { thumbnailSize } from "./ipc";

/** Formats that can be transparent at all; anything else is never asked about. */
const ALPHA_FORMATS = new Set(["image/png", "image/apng", "image/webp", "image/gif", "image/avif"]);

/**
 * The share of see-through pixels that makes an image a cut-out.
 *
 * Low enough to catch a character on an empty background, high enough that a
 * screenshot with rounded corners stays a picture.
 */
const CUTOUT_SHARE = 0.04;

/** Below this alpha a pixel counts as see-through. */
const SEE_THROUGH = 128;

/** How small the image is drawn before counting — plenty to judge by. */
const SAMPLE = 48;

/**
 * Whether these RGBA pixels are a cut-out.
 *
 * Separate from the decoding so it can be tested without a canvas.
 */
export function isCutout(rgba: Uint8ClampedArray): boolean {
  const pixels = rgba.length / 4;
  if (pixels === 0) return false;

  let clear = 0;
  for (let i = 3; i < rgba.length; i += 4) {
    if (rgba[i] < SEE_THROUGH) clear += 1;
  }
  return clear / pixels >= CUTOUT_SHARE;
}

// Answers are remembered across launches: they never change for a given mxc,
// and without this every transparent image would flash its frame once per
// session before losing it.
// Still under the pre-rename name, so existing caches carry over.
const STORAGE_KEY = "uwum:cutouts";
const REMEMBERED = 500;

const known = loadKnown();
const inFlight = new Map<string, Promise<boolean>>();

function loadKnown(): Map<string, boolean> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return new Map(JSON.parse(raw) as [string, boolean][]);
  } catch {
    // Unreadable or blocked storage just means asking again.
  }
  return new Map();
}

function remember(mxc: string, cutout: boolean) {
  known.set(mxc, cutout);
  // Oldest first, so dropping from the front keeps the recent ones.
  while (known.size > REMEMBERED) known.delete(known.keys().next().value as string);
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...known]));
  } catch {
    // Not worth failing over; it's only a cache.
  }
}

async function inspect(
  mxc: string,
  mimetype: string,
  size: { width: number; height: number },
): Promise<boolean> {
  const { width, height } = thumbnailSize(size);
  const bytes = await invoke<ArrayBuffer>("get_media_bytes", { mxc, width, height });
  const bitmap = await createImageBitmap(new Blob([bytes], { type: mimetype }));

  try {
    const canvas = document.createElement("canvas");
    canvas.width = SAMPLE;
    canvas.height = SAMPLE;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) return false;
    context.drawImage(bitmap, 0, 0, SAMPLE, SAMPLE);
    return isCutout(context.getImageData(0, 0, SAMPLE, SAMPLE).data);
  } finally {
    bitmap.close();
  }
}

/**
 * Whether the image at `mxc` is a cut-out, once that's known.
 *
 * `false` until then, and for good if it can't be checked — showing a sticker
 * with a frame is a much smaller mistake than losing one around a photo.
 */
export function useIsCutout(
  mxc: string | null | undefined,
  mimetype: string | null,
  size: { width: number; height: number },
): boolean {
  const candidate = mxc && mimetype && ALPHA_FORMATS.has(mimetype) ? mxc : null;
  const [cutout, setCutout] = useState(() => (candidate ? (known.get(candidate) ?? false) : false));

  const { width, height } = size;
  useEffect(() => {
    if (!candidate || !mimetype) {
      setCutout(false);
      return;
    }

    const remembered = known.get(candidate);
    if (remembered !== undefined) {
      setCutout(remembered);
      return;
    }

    let cancelled = false;
    let request = inFlight.get(candidate);
    if (!request) {
      // A failure isn't remembered: it's an answer for this session, not
      // for the image, and the next launch may well be online.
      request = inspect(candidate, mimetype, { width, height })
        .then(
          (answer) => {
            remember(candidate, answer);
            return answer;
          },
          () => false,
        )
        .finally(() => inFlight.delete(candidate));
      inFlight.set(candidate, request);
    }

    void request.then((answer) => {
      if (!cancelled) setCutout(answer);
    });
    return () => {
      cancelled = true;
    };
  }, [candidate, mimetype, width, height]);

  return cutout;
}
