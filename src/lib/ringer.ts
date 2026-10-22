/**
 * The ringtone behind an incoming DM call.
 *
 * The store decides *whether* a call is ringing; this decides what that sounds
 * like. Keeping the two apart means the modal and the audio can never disagree
 * about what is happening — there is one piece of state, `incomingCall`, and
 * the sound is a consequence of it rather than a second thing to keep in step.
 */

import { call, RING_TIMEOUT_MS } from "./call";
import { startRinging } from "./sounds";
import { useStore } from "../store";

export function startRinger(): () => void {
  let stop: (() => void) | null = null;
  let timer: number | null = null;
  let ringingFor: string | null = null;

  function silence(): void {
    stop?.();
    stop = null;
    if (timer !== null) window.clearTimeout(timer);
    timer = null;
    ringingFor = null;
  }

  const unsubscribe = useStore.subscribe((state) => {
    const roomId = state.incomingCall?.roomId ?? null;
    if (roomId === ringingFor) return;

    // Any change of room means the previous ring is over, answered or not.
    silence();
    if (!roomId) return;

    const { settings } = state;
    ringingFor = roomId;
    stop = startRinging(settings.callSound, settings.notificationVolume);
    timer = window.setTimeout(() => {
      useStore.getState().stopIncomingCall(roomId);
    }, RING_TIMEOUT_MS);
  });

  // Answering is not the only way into a call: the room's own call button gets
  // there too, and a phone that keeps ringing after you have joined is the
  // kind of bug people describe as "it rang at me while I was talking".
  const unsubscribeCall = call.subscribe((state) => {
    if (state.roomId && state.status !== "idle") {
      useStore.getState().stopIncomingCall(state.roomId);
    }
  });

  return () => {
    unsubscribe();
    unsubscribeCall();
    silence();
  };
}
