import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { RoomSummary } from "./types";

const sendNotification = vi.fn();
const playMessageSound = vi.fn();
const playCallSound = vi.fn();

vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: () => Promise.resolve(true),
  requestPermission: () => Promise.resolve("granted"),
  sendNotification: (options: unknown) => sendNotification(options),
  onAction: () => Promise.resolve({ unregister: () => Promise.resolve() }),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    unminimize: () => Promise.resolve(),
    show: () => Promise.resolve(),
    setFocus: () => Promise.resolve(),
  }),
}));

vi.mock("./sounds", () => ({
  playMessageSound: (id: string, volume: number) => playMessageSound(id, volume),
  playCallSound: (id: string, volume: number) => playCallSound(id, volume),
}));

/**
 * The call we are currently in. `notify` reads this from the controller rather
 * than from the store — the store field it used to read was never assigned by
 * anything, so this test passed while the real app notified you about your own
 * calls.
 */
const currentCall: { roomId: string | null; status: string } = {
  roomId: null,
  status: "idle",
};

vi.mock("./call", () => ({
  call: {
    getState: () => currentCall,
    subscribe: () => () => {},
  },
}));

const { startNotifications } = await import("./notify");
const { useStore } = await import("../store");

function room(patch: Partial<RoomSummary> & { id: string }): RoomSummary {
  return {
    name: patch.id,
    topic: null,
    canonicalAlias: null,
    avatarUrl: null,
    isDirect: false,
    dmUserId: null,
    isEncrypted: true,
    isSpace: false,
    isUtility: false,
    isMuted: false,
    notifyMode: null,
    membership: "joined",
    notificationCount: 0,
    highlightCount: 0,
    hasUnread: false,
    isMarkedUnread: false,
    memberCount: 2,
    latest: { sender: "@fa:veil.gg", senderName: "fa", body: "hi~", timestamp: 1 },
    recency: 1,
    parentSpaces: [],
    hasActiveCall: false,
    isVideoRoom: false,
    ...patch,
  };
}

/** Push a room list and let the watcher see it. */
function push(rooms: RoomSummary[]) {
  useStore.setState({ rooms });
}

describe("notifications", () => {
  let stop: () => void;

  beforeEach(() => {
    sendNotification.mockClear();
    playMessageSound.mockClear();
    playCallSound.mockClear();
    // Unfocused by default: the interesting cases are the ones where the user
    // isn't already looking at the room.
    vi.spyOn(document, "hasFocus").mockReturnValue(false);
    currentCall.roomId = null;
    currentCall.status = "idle";
    useStore.setState({ rooms: [], activeRoomId: null, incomingCall: null });
    stop = startNotifications();
  });

  afterEach(() => {
    stop();
    vi.restoreAllMocks();
  });

  it("treats the first room list as a baseline, not as news", () => {
    push([room({ id: "!a", notificationCount: 12 })]);
    expect(sendNotification).not.toHaveBeenCalled();
    expect(playMessageSound).not.toHaveBeenCalled();
  });

  it("notifies when a room's notification count goes up", () => {
    push([room({ id: "!a", name: "the pit", notificationCount: 0 })]);
    push([room({ id: "!a", name: "the pit", notificationCount: 1 })]);

    expect(sendNotification).toHaveBeenCalledTimes(1);
    expect(sendNotification.mock.calls[0][0]).toMatchObject({
      title: "the pit",
      body: "fa: hi~",
      extra: { roomId: "!a" },
    });
    expect(playMessageSound).toHaveBeenCalledOnce();
  });

  it("doesn't repeat the sender's name in a dm", () => {
    push([room({ id: "!a", name: "fa", isDirect: true })]);
    push([room({ id: "!a", name: "fa", isDirect: true, notificationCount: 1 })]);

    expect(sendNotification.mock.calls[0][0]).toMatchObject({ title: "fa", body: "hi~" });
  });

  it("stays quiet for the room you're reading", () => {
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    useStore.setState({ activeRoomId: "!a" });

    push([room({ id: "!a" })]);
    push([room({ id: "!a", notificationCount: 1 })]);

    expect(sendNotification).not.toHaveBeenCalled();
  });

  it("stays quiet for a muted room", () => {
    push([room({ id: "!a", isMuted: true })]);
    push([room({ id: "!a", isMuted: true, notificationCount: 3 })]);

    expect(sendNotification).not.toHaveBeenCalled();
  });

  it("rings when a call starts", () => {
    push([room({ id: "!a", name: "hangout" })]);
    push([room({ id: "!a", name: "hangout", hasActiveCall: true })]);

    expect(sendNotification.mock.calls[0][0]).toMatchObject({
      title: "hangout",
      body: "call starting~",
    });
    expect(playCallSound).toHaveBeenCalledOnce();
  });

  it("doesn't ring for the call you're already in", () => {
    currentCall.roomId = "!a";
    currentCall.status = "connected";
    push([room({ id: "!a" })]);
    push([room({ id: "!a", hasActiveCall: true })]);

    expect(sendNotification).not.toHaveBeenCalled();
    expect(playCallSound).not.toHaveBeenCalled();
  });

  it("rings a DM until it is answered, rather than chirping once", () => {
    push([room({ id: "!a", name: "kai", isDirect: true })]);
    push([room({ id: "!a", name: "kai", isDirect: true, hasActiveCall: true })]);

    expect(useStore.getState().incomingCall?.roomId).toBe("!a");
    // The sustained ring replaces the one-shot chirp; playing both would
    // double up on the first ring.
    expect(playCallSound).not.toHaveBeenCalled();
    expect(sendNotification.mock.calls[0][0]).toMatchObject({ body: "calling~" });
  });

  it("does not ring a group call", () => {
    push([room({ id: "!a", name: "hangout" })]);
    push([room({ id: "!a", name: "hangout", hasActiveCall: true })]);

    expect(useStore.getState().incomingCall).toBeNull();
    expect(playCallSound).toHaveBeenCalledOnce();
  });

  it("stops ringing when the caller gives up", () => {
    push([room({ id: "!a", isDirect: true })]);
    push([room({ id: "!a", isDirect: true, hasActiveCall: true })]);
    expect(useStore.getState().incomingCall?.roomId).toBe("!a");

    push([room({ id: "!a", isDirect: true, hasActiveCall: false })]);
    expect(useStore.getState().incomingCall).toBeNull();
  });

  it("does not ring for a DM call you started yourself", () => {
    currentCall.roomId = "!a";
    currentCall.status = "connected";
    push([room({ id: "!a", isDirect: true })]);
    push([room({ id: "!a", isDirect: true, hasActiveCall: true })]);

    expect(useStore.getState().incomingCall).toBeNull();
  });

  it("announces an invite once", () => {
    push([room({ id: "!a", name: "secret club", membership: "joined" })]);
    push([room({ id: "!a", name: "secret club", membership: "invited" })]);
    push([room({ id: "!a", name: "secret club", membership: "invited" })]);

    expect(sendNotification).toHaveBeenCalledTimes(1);
    expect(sendNotification.mock.calls[0][0]).toMatchObject({ body: "invited you~" });
  });

  it("honours the message and call toggles", () => {
    useStore.setState((s) => ({
      settings: { ...s.settings, notifyMessages: false, notifyCalls: false },
    }));

    push([room({ id: "!a" })]);
    push([room({ id: "!a", notificationCount: 4, hasActiveCall: true })]);

    expect(sendNotification).not.toHaveBeenCalled();
  });
});
