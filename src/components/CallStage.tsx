import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { Track } from "livekit-client";

import { call, screenShareSupported, type CallParticipantView } from "../lib/call";
import { localpart } from "../lib/display";
import * as ipc from "../lib/ipc";
import { useStore } from "../store";
import { useCallState } from "./CallBar";
import { Avatar, Icon, Spinner } from "./ui";
import { corner, space, textSize } from "../lib/scale";

/**
 * The video stage: a grid of participant tiles above the timeline, in the shape
 * people already know from Discord and Meet.
 *
 * It deliberately doesn't take the whole window by default — chat stays
 * readable underneath, which is the point of having calls inside a chat client.
 * Fullscreen is a toggle for when the call *is* the thing you're doing.
 */
export function CallStage({ roomId }: { roomId: string }) {
  const state = useCallState();
  const showBanner = useStore((s) => s.showBanner);
  const [fullscreen, setFullscreen] = useState(false);

  // Escape is what everyone reaches for to get out of a fullscreen video.
  useEffect(() => {
    if (!fullscreen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFullscreen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fullscreen]);

  // Only draw for the room being looked at; a call elsewhere shows as the
  // compact bar instead.
  if (state.roomId !== roomId || state.status === "idle") return null;

  const connecting = state.status === "connecting";
  const reconnecting = state.status === "reconnecting";

  // A shared screen is the thing everyone is looking at, so it gets the stage
  // and the people become a filmstrip.
  const shared = state.participants.find((p) => p.screenTrack);

  async function run(action: () => Promise<void>) {
    try {
      await action();
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  const stage = (
    <div
      style={{
        position: fullscreen ? "fixed" : "relative",
        inset: fullscreen ? 0 : undefined,
        zIndex: fullscreen ? 90 : 1,
        display: "flex",
        flexDirection: "column",
        gap: space(10),
        margin: fullscreen ? 0 : "12px 22px 0",
        padding: space(fullscreen ? 24 : 12),
        borderRadius: corner(fullscreen ? 0 : 20),
        background: fullscreen ? "var(--surface-app)" : "var(--surface-card)",
        border: fullscreen ? "none" : "1px solid var(--border-subtle)",
        // Enough to be useful, not so much that chat disappears.
        height: fullscreen ? undefined : 320,
        flex: "none",
      }}
    >
      {connecting ? (
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            gap: space(12),
            color: "var(--text-tertiary)",
            fontSize: textSize(13),
          }}
        >
          <Spinner size={22} />
          connecting to the call…
        </div>
      ) : shared ? (
        <div style={{ flex: 1, display: "flex", gap: space(10), minHeight: 0 }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <Tile participant={shared} track={shared.screenTrack} label="screen" contain />
          </div>
          <div
            className="uwu-scroll"
            style={{
              width: 172,
              flex: "none",
              display: "flex",
              flexDirection: "column",
              gap: space(8),
            }}
          >
            {state.participants.map((p) => (
              <div key={p.identity} style={{ aspectRatio: "16 / 10", flex: "none" }}>
                <Tile participant={p} track={p.cameraTrack} />
              </div>
            ))}
          </div>
        </div>
      ) : (
        <div
          style={{
            flex: 1,
            minHeight: 0,
            display: "grid",
            gap: space(10),
            // Tiles get bigger as the call gets smaller, without a breakpoint
            // for every headcount.
            gridTemplateColumns: `repeat(auto-fit, minmax(${
              state.participants.length <= 2 ? 260 : 168
            }px, 1fr))`,
            // Rows divide the stage rather than being sized by the tiles. An
            // aspect-ratio here would let a tile grow taller than its row and
            // push its own name badge out of sight.
            gridAutoRows: "minmax(0, 1fr)",
            overflow: "hidden",
          }}
        >
          {state.participants.map((p) => (
            <div key={p.identity} style={{ minHeight: 0, minWidth: 0 }}>
              <Tile participant={p} track={p.cameraTrack} />
            </div>
          ))}
        </div>
      )}

      <div style={{ display: "flex", alignItems: "center", gap: space(8) }}>
        <span
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10.5),
            color: reconnecting ? "var(--status-warning)" : "var(--text-tertiary)",
            marginRight: "auto",
          }}
        >
          {reconnecting
            ? "reconnecting…"
            : `${state.participants.length} in the call`}
        </span>

        <Control
          icon={state.micEnabled ? "microphone" : "microphone-slash"}
          title={state.micEnabled ? "mute" : "unmute"}
          active={!state.micEnabled}
          onClick={() => void run(() => call.setMicEnabled(!state.micEnabled))}
        />
        <Control
          icon={state.cameraEnabled ? "video-camera" : "video-camera-slash"}
          title={state.cameraEnabled ? "turn camera off" : "turn camera on"}
          active={!state.cameraEnabled}
          onClick={() => void run(() => call.setCameraEnabled(!state.cameraEnabled))}
        />
        {screenShareSupported() && (
          <Control
            icon="monitor-arrow-up"
            title={state.screenShareEnabled ? "stop sharing" : "share your screen"}
            highlight={state.screenShareEnabled}
            onClick={() =>
              void run(() => call.setScreenShareEnabled(!state.screenShareEnabled))
            }
          />
        )}
        <Control
          icon={state.deafened ? "speaker-slash" : "speaker-high"}
          title={state.deafened ? "undeafen" : "deafen"}
          active={state.deafened}
          onClick={() => void run(() => call.setDeafened(!state.deafened))}
        />
        <Control
          icon={fullscreen ? "corners-in" : "corners-out"}
          title={fullscreen ? "exit fullscreen" : "fullscreen"}
          onClick={() => setFullscreen((v) => !v)}
        />
        <Control icon="phone-x" title="leave the call" danger onClick={() => void call.leave()} />
      </div>
    </div>
  );

  // Fullscreen has to leave the layout entirely, not just raise its z-index.
  // This lives inside ChatPane, which sets `position: relative` and a z-index —
  // that makes a stacking context, so a z-index here can only compete with
  // ChatPane's own children. The room info panel is a *sibling* of ChatPane and
  // comes after it in the DOM, so it would paint over the top however high we
  // set the number. A portal sidesteps the whole question.
  return fullscreen ? createPortal(stage, document.body) : stage;
}

/**
 * One participant.
 *
 * The video element is fed by attaching the LiveKit track directly — frames
 * can't travel through React state, so the track object comes down and the
 * element is wired to it here.
 */
function Tile({
  participant,
  track,
  label,
  contain,
}: {
  participant: CallParticipantView;
  track: Track | null;
  label?: string;
  /** Screen shares are letterboxed; faces are cropped to fill. */
  contain?: boolean;
}) {
  const video = useRef<HTMLVideoElement>(null);
  const name = localpart(participant.userId);

  useEffect(() => {
    const element = video.current;
    if (!track || !element) return;
    track.attach(element);
    return () => {
      track.detach(element);
    };
  }, [track]);

  return (
    <div
      style={{
        position: "relative",
        width: "100%",
        height: "100%",
        borderRadius: corner(16),
        overflow: "hidden",
        background: "var(--ink-900)",
        border: "1px solid var(--border-subtle)",
        outline: participant.isSpeaking
          ? "2px solid var(--accent-primary)"
          : "2px solid transparent",
        outlineOffset: -1,
        transition: "outline-color var(--dur-fast) var(--ease-out)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      {track ? (
        <video
          ref={video}
          autoPlay
          playsInline
          // Your own camera is mirrored, the way every other client does it —
          // an unmirrored self-view reads as wrong even though it's accurate.
          muted={participant.isLocal}
          style={{
            width: "100%",
            height: "100%",
            objectFit: contain ? "contain" : "cover",
            transform: participant.isLocal && !label ? "scaleX(-1)" : undefined,
          }}
        />
      ) : (
        <Avatar id={participant.userId} name={name} size={56} radius={20} />
      )}

      <div
        style={{
          position: "absolute",
          left: 8,
          bottom: 8,
          right: 8,
          display: "flex",
          alignItems: "center",
          gap: space(6),
        }}
      >
        <span
          className="uwu-ellipsis"
          style={{
            padding: space("3px 9px"),
            borderRadius: 999,
            background: "color-mix(in srgb, var(--ink-950) 72%, transparent)",
            fontFamily: "var(--font-display)",
            fontWeight: 700,
            fontSize: textSize(12),
            color: "var(--text-primary)",
          }}
        >
          {participant.isLocal ? "you" : name}
          {label ? ` · ${label}` : ""}
        </span>
        {participant.isMuted && !label && (
          <span
            style={{
              width: 20,
              height: 20,
              flex: "none",
              borderRadius: "50%",
              background: "color-mix(in srgb, var(--ink-950) 72%, transparent)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            <Icon name="microphone-slash" size={11} color="var(--status-danger)" />
          </span>
        )}
      </div>
    </div>
  );
}

function Control({
  icon,
  title,
  onClick,
  active,
  highlight,
  danger,
}: {
  icon: string;
  title: string;
  onClick: () => void;
  /** Muted/off — the state worth noticing. */
  active?: boolean;
  highlight?: boolean;
  danger?: boolean;
}) {
  const background = danger
    ? "var(--status-danger)"
    : highlight
      ? "var(--accent-primary)"
      : active
        ? "var(--surface-inset)"
        : "var(--surface-card-raised)";

  const colour = danger
    ? "var(--ink-950)"
    : highlight
      ? "var(--text-on-accent)"
      : active
        ? "var(--status-warning)"
        : "var(--text-secondary)";

  return (
    <button
      onClick={onClick}
      title={title}
      style={{
        width: 36,
        height: 36,
        borderRadius: corner(12),
        background,
        border: "1px solid var(--border-subtle)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
      }}
    >
      <Icon name={icon} size={16} color={colour} />
    </button>
  );
}
