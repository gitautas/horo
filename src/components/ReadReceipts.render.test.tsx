import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { EventItem, TimelineItem } from "../lib/types";

const getProfile = vi.fn();

vi.mock("../lib/ipc", () => ({
  getProfile: (userId: string) => getProfile(userId),
  // The avatar asks for a thumbnail; there's no protocol handler in a test, so
  // the mxc is echoed back and asserted on directly.
  mediaUrl: (mxc: string | null | undefined) => mxc ?? null,
  asUwuError: (e: unknown) => ({ kind: "other", message: String(e) }),
}));

// Nothing to fetch the picture from either, so the cache hands the src back as
// its own URL — which picture was chosen is what these tests are about.
vi.mock("../lib/avatarCache", () => ({
  cachedAvatar: () => undefined,
  avatarFailed: () => false,
  loadAvatar: async (src: string) => src,
}));

const { ReadReceipts } = await import("./TimelineView");
const { invalidateProfile } = await import("../lib/profiles");

const READER = "@fa:veil.gg";
const AUTHOR = "@gintas:veil.gg";

function event(item: Partial<EventItem>): TimelineItem {
  return {
    id: item.eventId ?? "e",
    kind: "event",
    event: {
      eventId: "e",
      transactionId: null,
      sender: AUTHOR,
      senderName: "Gintas",
      senderAvatar: null,
      timestamp: 0,
      isOwn: false,
      isEditable: false,
      canReply: true,
      isHighlighted: false,
      isEdited: false,
      isEmojiOnly: false,
      sendState: null,
      shield: null,
      content: { type: "text", body: "meow", formatted: null },
      reactions: [],
      reply: null,
      threadRoot: null,
      threadSummary: null,
      readReceipts: [],
      ...item,
    } as EventItem,
  };
}

function profile(avatarUrl: string | null) {
  return {
    userId: READER,
    displayName: "fa",
    avatarUrl,
    bio: null,
    status: null,
    coverUrl: null,
  };
}

describe("the read receipt facepile", () => {
  beforeEach(() => {
    getProfile.mockReset();
    invalidateProfile(READER);
    invalidateProfile(AUTHOR);
  });
  afterEach(cleanup);

  it("draws the reader's avatar, not just their initials", async () => {
    getProfile.mockResolvedValue(profile("mxc://veil.gg/fa"));

    render(<ReadReceipts items={[event({ readReceipts: [READER] })]} />);

    // The bug: this used to be a monogram with no image behind it at all.
    const avatar = await waitFor(() => {
      const img = document.querySelector("img");
      expect(img).not.toBeNull();
      return img!;
    });
    expect(avatar.getAttribute("src")).toBe("mxc://veil.gg/fa");
    // Faces only: the count is a label, not text on screen.
    expect(screen.getByRole("group", { name: "seen by 1" })).toBeTruthy();
    expect(screen.queryByText(/seen by/)).toBeNull();
  });

  it("uses the picture already on screen rather than asking again", async () => {
    render(
      <ReadReceipts
        items={[
          event({
            sender: READER,
            senderName: "fa",
            senderAvatar: "mxc://veil.gg/from-timeline",
            readReceipts: [READER],
          }),
        ]}
      />,
    );

    await waitFor(() => expect(document.querySelector("img")).not.toBeNull());
    expect(document.querySelector("img")!.getAttribute("src")).toBe(
      "mxc://veil.gg/from-timeline",
    );
    expect(getProfile).not.toHaveBeenCalled();
  });

  it("falls back to a monogram when the profile can't be loaded", async () => {
    getProfile.mockRejectedValue(new Error("offline"));

    render(<ReadReceipts items={[event({ readReceipts: [READER] })]} />);

    await waitFor(() => expect(getProfile).toHaveBeenCalled());
    expect(document.querySelector("img")).toBeNull();
    // `initialsFor` on the localpart of `@fa:veil.gg`.
    expect(screen.getByText("fa")).toBeTruthy();
  });

  it("says nothing when nobody has read the last message", () => {
    const { container } = render(<ReadReceipts items={[event({ readReceipts: [] })]} />);
    expect(container.textContent).toBe("");
  });
});
