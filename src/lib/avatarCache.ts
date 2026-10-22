/**
 * Avatars, decoded once and kept in memory for the session.
 *
 * WebKit doesn't put `horo://` responses in its HTTP cache: a custom scheme
 * handler is asked again for every `<img>` that mounts, however immutable the
 * response says it is. So each avatar that scrolled back into view, or sat in
 * a row the list re-rendered, went back to Rust for its bytes, and showed its
 * monogram until they arrived — which is the popping in and out.
 *
 * Here each picture is fetched once, wrapped in a `blob:` URL and decoded, and
 * an `Avatar` that mounts later finds it ready and paints it on its first
 * frame. Failures are remembered briefly too, so a picture the server can't
 * produce stays a monogram instead of flickering between attempts.
 */

/** About 30 KB a picture at the size avatars are fetched, so a few MB at most. */
const MAX_ENTRIES = 400;

/** Matches the Rust side's memory of failed media, after which both retry. */
const FAILURE_MEMORY_MS = 60_000;

const ready = new Map<string, string>();
const failed = new Map<string, number>();
const inFlight = new Map<string, Promise<string>>();

/** The blob URL for an avatar that's already loaded, without waiting. */
export function cachedAvatar(src: string): string | undefined {
  const url = ready.get(src);
  if (url) {
    // Most recently used goes to the back, so eviction takes the stalest.
    ready.delete(src);
    ready.set(src, url);
  }
  return url;
}

/** Whether this avatar failed recently enough that asking again is pointless. */
export function avatarFailed(src: string): boolean {
  const at = failed.get(src);
  if (at === undefined) return false;
  if (Date.now() - at < FAILURE_MEMORY_MS) return true;
  failed.delete(src);
  return false;
}

/**
 * Load an avatar into the cache. Concurrent callers share one request: a room
 * full of messages from one person asks for their picture once.
 */
export function loadAvatar(src: string): Promise<string> {
  const cached = cachedAvatar(src);
  if (cached) return Promise.resolve(cached);

  const existing = inFlight.get(src);
  if (existing) return existing;

  const request = (async () => {
    try {
      const response = await fetch(src);
      if (!response.ok) throw new Error(`avatar request failed: ${response.status}`);
      const url = URL.createObjectURL(await response.blob());

      // Decoded before anyone is told, so the first paint has pixels in it.
      const image = new Image();
      image.src = url;
      try {
        await image.decode();
      } catch (e) {
        URL.revokeObjectURL(url);
        throw e;
      }

      remember(src, url);
      return url;
    } catch (e) {
      failed.set(src, Date.now());
      throw e;
    }
  })().finally(() => inFlight.delete(src));

  inFlight.set(src, request);
  return request;
}

function remember(src: string, url: string) {
  ready.set(src, url);
  while (ready.size > MAX_ENTRIES) {
    const [oldest, oldUrl] = ready.entries().next().value!;
    ready.delete(oldest);
    // Safe while it's on screen: an image already decoded from a blob URL
    // keeps its pixels after the URL is revoked. A later mount refetches.
    URL.revokeObjectURL(oldUrl);
  }
}
