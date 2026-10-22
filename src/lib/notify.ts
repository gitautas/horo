/**
 * Desktop notifications.
 *
 * The trigger is the room list rather than the timeline: `notificationCount`
 * comes from the homeserver, which has already run your push rules over the
 * event — so muted rooms, keyword rules and "mentions only" all work without
 * this file knowing anything about them. Watching timelines instead would only
 * ever notify for rooms that happen to be open, which is exactly backwards.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  isPermissionGranted,
  onAction,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type { PluginListener } from "@tauri-apps/api/core";

import { call } from "./call";
import { playCallSound, playMessageSound } from "./sounds";
import type { RoomSummary } from "./types";
import { useStore } from "../store";

/** What we knew about a room last time we looked. */
interface Seen {
  count: number;
  call: boolean;
  invited: boolean;
}

/** Longest body we'll put in a banner before trailing off. */
const MAX_BODY = 160;

/**
 * One room-list batch can reach the store as several updates, and ten messages
 * arriving at once shouldn't play ten overlapping chirps. So each kind of sound
 * has a floor on how often it may repeat.
 *
 * Kept per kind rather than globally: a call that starts just after a message
 * landed is the one thing you'd most want to hear.
 */
const SOUND_GAP_MS = 800;

const lastPlayed: Record<"message" | "call", number> = { message: 0, call: 0 };

function playOnce(kind: "message" | "call", play: () => void): void {
  const now = Date.now();
  if (now - lastPlayed[kind] < SOUND_GAP_MS) return;
  lastPlayed[kind] = now;
  play();
}

function truncate(text: string): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > MAX_BODY ? `${line.slice(0, MAX_BODY - 1)}…` : line;
}

/** Ask once, at startup. A refusal is final — we don't nag on every message. */
async function ensurePermission(): Promise<boolean> {
  try {
    if (await isPermissionGranted()) return true;
    return (await requestPermission()) === "granted";
  } catch {
    return false;
  }
}

let granted = false;

function notify(title: string, body: string, roomId: string): void {
  if (!granted) return;
  try {
    // `extra` comes back on the action event, which is how a click knows which
    // room to open.
    sendNotification({ title, body, extra: { roomId } });
  } catch {
    // A platform that won't show banners still gets the sound.
  }
}

/** Bring the window up and open the room whose banner was clicked. */
async function focusRoom(roomId: string): Promise<void> {
  try {
    const window = getCurrentWindow();
    await window.unminimize().catch(() => {});
    await window.show().catch(() => {});
    await window.setFocus();
  } catch {
    // Not fatal — the room still opens, the user just has to click the app.
  }
  void useStore.getState().selectRoom(roomId);
}

/**
 * True when the user is plainly already looking at this room, so a banner for
 * it would be telling them something they can see.
 */
function isBeingRead(roomId: string): boolean {
  const { activeRoomId, settings } = useStore.getState();
  if (settings.notifyWhenFocused) return false;
  return activeRoomId === roomId && document.hasFocus();
}

/** Who to say the message is from, in the room's own terms. */
function describe(room: RoomSummary): { title: string; body: string } {
  const latest = room.latest;
  const sender = latest?.senderName ?? latest?.sender ?? "someone";
  const body = latest ? truncate(latest.body) : "new message";

  // In a DM the room *is* the person, so naming them twice reads as a stutter.
  return room.isDirect
    ? { title: room.name, body }
    : { title: room.name, body: `${sender}: ${body}` };
}

function inspect(rooms: RoomSummary[], seen: Map<string, Seen>): void {
  const { settings, incomingCall, ringIncomingCall, stopIncomingCall } = useStore.getState();
  // The call we are *in*, straight from the controller that runs it. This used
  // to read a `callRoomId` in the store that nothing ever assigned, so the
  // guard below quietly did nothing and starting a call notified you about
  // your own call. Harmless as a stray banner; not harmless once it rings.
  const callRoomId = call.getState().roomId;
  let ring = false;
  let chirp = false;

  for (const room of rooms) {
    const previous = seen.get(room.id);
    seen.set(room.id, {
      count: room.notificationCount,
      call: room.hasActiveCall,
      invited: room.membership === "invited",
    });

    // A room we've only just heard of: nothing to compare against, and its
    // unread count is history rather than news.
    if (!previous) continue;

    if (settings.notifyMessages && !room.isMuted) {
      if (room.notificationCount > previous.count && !isBeingRead(room.id)) {
        const { title, body } = describe(room);
        notify(title, body, room.id);
        chirp = true;
      } else if (room.membership === "invited" && !previous.invited) {
        notify(room.name, "invited you~", room.id);
        chirp = true;
      }
    }

    // A call that has ended stops ringing, however it ended — the caller hung
    // up, or someone else in the room answered it. Checked before the start
    // below so a call that begins and ends inside one batch doesn't get stuck
    // ringing at nobody.
    if (!room.hasActiveCall && incomingCall?.roomId === room.id) {
      stopIncomingCall(room.id);
    }

    // Someone started a call in a room you're in — but not the call you're
    // already sitting in, which is where `hasActiveCall` also goes true.
    // A voice room's call is the room itself, and people drift in and out of
    // it all day — announcing that is a Discord channel pinging on every join.
    if (
      settings.notifyCalls &&
      !room.isMuted &&
      !room.isVideoRoom &&
      room.hasActiveCall &&
      !previous.call &&
      room.id !== callRoomId
    ) {
      notify(room.name, room.isDirect ? "calling~" : "call starting~", room.id);

      // A DM is one person calling *you*, so it rings until you deal with it.
      // A group call is an announcement — forty people cannot all be expected
      // to answer, and forty ringing phones is how you get a muted room.
      if (room.isDirect) {
        ringIncomingCall(room.id);
      } else {
        ring = true;
      }
    }
  }

  // A call is the louder event, so it wins if both happened at once.
  if (ring) {
    playOnce("call", () =>
      playCallSound(settings.callSound, settings.notificationVolume),
    );
  } else if (chirp) {
    playOnce("message", () =>
      playMessageSound(settings.messageSound, settings.notificationVolume),
    );
  }
}

/**
 * Start watching for things worth interrupting the user about.
 *
 * Returns an unsubscribe, so the shell can stop this on sign-out — the room
 * list is emptied by `reset`, and a fresh session should not be greeted with a
 * banner for every room it syncs.
 */
export function startNotifications(): () => void {
  let seen: Map<string, Seen> | null = null;

  // A new session starts quiet rather than inheriting the last one's throttle.
  lastPlayed.message = 0;
  lastPlayed.call = 0;

  void ensurePermission().then((ok) => {
    granted = ok;
  });

  let stopAction: PluginListener | undefined;
  void onAction((notification) => {
    const roomId = (notification.extra as { roomId?: string } | undefined)?.roomId;
    if (roomId) void focusRoom(roomId);
  })
    .then((listener) => {
      stopAction = listener;
    })
    .catch(() => {
      // Clicking the banner just won't jump to the room on this platform.
    });

  const unsubscribe = useStore.subscribe((state, previous) => {
    if (state.rooms === previous.rooms) return;

    // The first list we see is the baseline, not a pile of new arrivals.
    if (!seen) {
      seen = new Map(
        state.rooms.map((room) => [
          room.id,
          {
            count: room.notificationCount,
            call: room.hasActiveCall,
            invited: room.membership === "invited",
          },
        ]),
      );
      return;
    }

    inspect(state.rooms, seen);
  });

  return () => {
    unsubscribe();
    void stopAction?.unregister().catch(() => {});
  };
}
