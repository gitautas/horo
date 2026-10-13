/**
 * Blob URLs for `<video>` and `<audio>`.
 *
 * Images load happily from the `horo://` scheme, but WKWebView doesn't play
 * media elements through the WebView's networking at all — it hands the URL to
 * AVFoundation, which knows nothing about custom scheme handlers. The request
 * never reaches Rust, and the element reports "format not supported" with no
 * network activity to show for it.
 *
 * So media bytes come over IPC and get wrapped in a `blob:` URL, which
 * AVFoundation will load. The cost is that a video is fully in memory before it
 * plays; that's acceptable for chat attachments and is the same trade Element
 * makes for encrypted media.
 */

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

interface Entry {
  url: string;
  /** How many mounted components are using this URL. */
  refs: number;
}

const cache = new Map<string, Entry>();
const inFlight = new Map<string, Promise<string>>();

async function load(mxc: string, mimetype: string | null): Promise<string> {
  const cached = cache.get(mxc);
  if (cached) return cached.url;

  const existing = inFlight.get(mxc);
  if (existing) return existing;

  const request = (async () => {
    // Raw command responses arrive as an ArrayBuffer.
    const bytes = await invoke<ArrayBuffer>("get_media_bytes", { mxc });
    const blob = new Blob([bytes], { type: resolveType(bytes, mimetype) });
    const url = URL.createObjectURL(blob);
    cache.set(mxc, { url, refs: 0 });
    return url;
  })().finally(() => inFlight.delete(mxc));

  inFlight.set(mxc, request);
  return request;
}

/**
 * Decide what type to stamp on the blob.
 *
 * A blob typed `application/octet-stream` is not playable — WebKit won't guess,
 * it just reports "format not supported". Senders don't always fill in
 * `info.mimetype`, and for encrypted attachments it's often missing entirely,
 * so sniff the container from the bytes we already have and fall back to what
 * the event claimed.
 */
function resolveType(bytes: ArrayBuffer, declared: string | null): string {
  const sniffed = sniffContainer(new Uint8Array(bytes, 0, Math.min(16, bytes.byteLength)));
  if (sniffed) return sniffed;
  if (declared && declared !== "application/octet-stream") return declared;
  return "";
}

function sniffContainer(head: Uint8Array): string | null {
  const ascii = (offset: number, length: number) =>
    String.fromCharCode(...head.subarray(offset, offset + length));

  // ISO base media (mp4/m4a/mov): a `ftyp` box at offset 4.
  if (head.length >= 12 && ascii(4, 4) === "ftyp") {
    const brand = ascii(8, 4);
    return brand.startsWith("M4A") ? "audio/mp4" : "video/mp4";
  }
  // Matroska / WebM.
  if (head[0] === 0x1a && head[1] === 0x45 && head[2] === 0xdf && head[3] === 0xa3) {
    return "video/webm";
  }
  if (ascii(0, 4) === "OggS") return "audio/ogg";
  if (ascii(0, 4) === "RIFF" && ascii(8, 4) === "WAVE") return "audio/wav";
  if (ascii(0, 3) === "ID3" || (head[0] === 0xff && (head[1] & 0xe0) === 0xe0)) {
    return "audio/mpeg";
  }
  if (ascii(0, 4) === "fLaC") return "audio/flac";
  return null;
}

/**
 * Resolve `mxc` to a playable URL.
 *
 * Blob URLs pin their data in memory until revoked, so this ref-counts: the URL
 * survives re-renders and remounts, and is released once nothing is showing it.
 */
export function useMediaBlob(
  mxc: string | null | undefined,
  mimetype: string | null,
): { url: string | undefined; error: string | null } {
  const [url, setUrl] = useState<string | undefined>(() => cache.get(mxc ?? "")?.url);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!mxc) {
      setUrl(undefined);
      return;
    }

    let cancelled = false;
    setError(null);

    load(mxc, mimetype)
      .then((resolved) => {
        if (cancelled) return;
        const entry = cache.get(mxc);
        if (entry) entry.refs += 1;
        setUrl(resolved);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      });

    return () => {
      cancelled = true;
      const entry = cache.get(mxc);
      if (!entry) return;
      entry.refs -= 1;
      if (entry.refs <= 0) {
        URL.revokeObjectURL(entry.url);
        cache.delete(mxc);
      }
    };
  }, [mxc, mimetype]);

  return { url, error };
}
