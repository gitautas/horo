import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/** A fresh copy of the module, so one test's cache can't leak into the next. */
async function fresh() {
  vi.resetModules();
  return import("./avatarCache");
}

let urls = 0;

beforeEach(() => {
  urls = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (src: string) =>
      src.includes("broken")
        ? new Response(null, { status: 502 })
        : new Response(new Blob(["png"])),
    ),
  );
  vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:${++urls}`);
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
  // happy-dom doesn't decode images.
  vi.spyOn(HTMLImageElement.prototype, "decode").mockResolvedValue(undefined);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("avatar cache", () => {
  it("knows nothing about a picture before it's loaded", async () => {
    const { cachedAvatar } = await fresh();
    expect(cachedAvatar("horo://media/a")).toBeUndefined();
  });

  it("hands a loaded picture back without asking again", async () => {
    const { cachedAvatar, loadAvatar } = await fresh();
    const url = await loadAvatar("horo://media/a");

    expect(cachedAvatar("horo://media/a")).toBe(url);
    expect(await loadAvatar("horo://media/a")).toBe(url);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("asks once for a picture many avatars want at the same moment", async () => {
    const { loadAvatar } = await fresh();
    const [a, b, c] = await Promise.all([
      loadAvatar("horo://media/a"),
      loadAvatar("horo://media/a"),
      loadAvatar("horo://media/a"),
    ]);

    expect(new Set([a, b, c]).size).toBe(1);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("remembers a failure for a minute, then tries again", async () => {
    vi.useFakeTimers();
    try {
      const { avatarFailed, loadAvatar } = await fresh();
      await expect(loadAvatar("horo://media/broken")).rejects.toThrow();

      expect(avatarFailed("horo://media/broken")).toBe(true);
      vi.advanceTimersByTime(61_000);
      expect(avatarFailed("horo://media/broken")).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("lets go of the least recently used pictures when it's full", async () => {
    const { cachedAvatar, loadAvatar } = await fresh();
    for (let i = 0; i < 400; i++) await loadAvatar(`horo://media/${i}`);
    // Seen again, so it's no longer the stalest.
    cachedAvatar("horo://media/0");

    await loadAvatar("horo://media/new");

    expect(cachedAvatar("horo://media/0")).toBeDefined();
    expect(cachedAvatar("horo://media/1")).toBeUndefined();
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(1);
  });
});
