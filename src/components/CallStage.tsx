import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { Track } from "livekit-client";

import { call, screenShareSupported, type CallParticipantView } from "../lib/call";
import { localpart } from "../lib/display";
import * as ipc from "../lib/ipc";
import { useStore } from "../store";
import { useCallState } from "./CallBar";
import { Avatar, Icon } from "./ui";
import { corner, space, textSize } from "../lib/scale";
import { useIsMobile } from "../lib/viewport";

/**
 * The video stage: a grid of cameras and shared screens above the timeline, in
 * the shape people already know from Discord and Meet.
 *
 * Only video gets a tile. A voice call with nobody on camera has nothing to
 * look at, so there's no stage at all — who's in it is listed under the room
 * in the sidebar, and the controls live in the sidebar's voice panel, as in
 * Discord.
 *
 * Clicking a tile focuses it — it takes the stage and everything else becomes a
 * filmstrip — and clicking the focused tile goes back to the grid.
 *
 * It deliberately doesn't take the whole window by default — chat stays
 * readable underneath, which is the point of having calls inside a chat client.
 * Fullscreen is a toggle for when the call *is* the thing you're doing.
 */
export function CallStage({ roomId }: { roomId: string }) {
  const state = useCallState();
  const showBanner = useStore((s) => s.showBanner);
  const [fullscreen, setFullscreen] = useState(false);
  const mobile = useIsMobile();
  // `undefined` is "not chosen yet", which lets a screen share take the stage
  // on its own; `null` is an explicit choice of the grid.
  const [picked, setPicked] = useState<string | null | undefined>(undefined);

  // Escape is what everyone reaches for to get out of a fullscreen video.
  useEffect(() => {
    if (!fullscreen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFullscreen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fullscreen]);

  const tiles = tilesOf(state.participants);

  // Only draw for the room being looked at, and only when there's video.
  // Leaving fullscreen as well means the next video doesn't open fullscreen.
  const showing = state.roomId === roomId && state.status !== "idle" && tiles.length > 0;
  useEffect(() => {
    if (!showing) setFullscreen(false);
  }, [showing]);
  if (!showing) return null;

  const reconnecting = state.status === "reconnecting";
  // A shared screen is the thing everyone is looking at, so until someone
  // picks otherwise it gets the stage. A pick that has left the call (they
  // hung up, or stopped sharing) falls back the same way.
  const firstScreen = tiles.find((t) => t.screen)?.key ?? null;
  const focusKey =
    picked === undefined || (picked !== null && !tiles.some((t) => t.key === picked))
      ? firstScreen
      : picked;
  const focused = tiles.find((t) => t.key === focusKey);

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
      {focused ? (
        <Focus
          focused={focused}
          others={tiles.filter((t) => t !== focused)}
          // Inline on a desktop the stage is wide and short, so the strip goes
          // down the side; fullscreen or on a phone there's height to spare.
          strip={fullscreen || mobile ? "bottom" : "side"}
          large={fullscreen}
          onPick={setPicked}
        />
      ) : (
        <Grid tiles={tiles} onPick={setPicked} />
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

        {/* Fullscreen covers the sidebar and its voice panel, so it brings
            the controls along. */}
        {fullscreen && (
          <>
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
            <Control icon="phone-x" title="leave the call" danger onClick={() => void call.leave()} />
          </>
        )}
        <Control
          icon={fullscreen ? "corners-in" : "corners-out"}
          title={fullscreen ? "exit fullscreen" : "fullscreen"}
          onClick={() => setFullscreen((v) => !v)}
        />
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

/** One thing on the stage: someone's camera, or a screen they're sharing. */
interface TileView {
  key: string;
  participant: CallParticipantView;
  track: Track | null;
  screen: boolean;
}

/**
 * A screen share is its own tile, the way Discord shows a stream next to the
 * person streaming it, rather than replacing their face. Shares come first —
 * they're what people joined to look at. Someone with their camera off has no
 * tile: an avatar in a box says nothing the sidebar doesn't.
 */
function tilesOf(participants: CallParticipantView[]): TileView[] {
  const screens = participants
    .filter((p) => p.screenTrack)
    .map((p) => ({ key: `${p.identity}:screen`, participant: p, track: p.screenTrack, screen: true }));
  const cameras = participants.filter((p) => p.cameraTrack).map((p) => ({
    key: `${p.identity}:camera`,
    participant: p,
    track: p.cameraTrack,
    screen: false,
  }));
  return [...screens, ...cameras];
}

const ASPECT = 16 / 9;
const GAP = 10;

/**
 * The column count that makes the biggest 16:9 tiles fit the box.
 *
 * Stretching tiles to fill rows instead crops faces into slivers in a wide,
 * short stage; keeping the shape and letterboxing the grid is what Discord
 * and Meet do.
 */
function fit(count: number, width: number, height: number) {
  let best = { columns: 1, width: 0, height: 0 };
  for (let columns = 1; columns <= count; columns++) {
    const rows = Math.ceil(count / columns);
    const w = Math.min(
      (width - GAP * (columns - 1)) / columns,
      ((height - GAP * (rows - 1)) / rows) * ASPECT,
    );
    if (w > best.width) best = { columns, width: Math.floor(w), height: Math.floor(w / ASPECT) };
  }
  return best;
}

function useSize(element: HTMLElement | null) {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    if (!element) return;
    // Measure now: the observer's first report waits for a frame, which would
    // paint one empty grid first.
    const rect = element.getBoundingClientRect();
    setSize({ width: rect.width, height: rect.height });
    const observer = new ResizeObserver(([entry]) => {
      setSize({ width: entry.contentRect.width, height: entry.contentRect.height });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [element]);
  return size;
}

function Grid({ tiles, onPick }: { tiles: TileView[]; onPick: (key: string) => void }) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const size = useSize(box);
  const { columns, width, height } = fit(tiles.length, size.width, size.height);

  return (
    <div
      ref={setBox}
      style={{
        flex: 1,
        minHeight: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        overflow: "hidden",
      }}
    >
      {width > 0 && (
        <div
          style={{
            display: "flex",
            flexWrap: "wrap",
            // A short last row sits in the middle, not hard left.
            justifyContent: "center",
            gap: GAP,
            maxWidth: columns * width + (columns - 1) * GAP,
          }}
        >
          {tiles.map((t) => (
            <div key={t.key} style={{ width, height, flex: "none" }}>
              <Tile tile={t} onClick={() => onPick(t.key)} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Focus({
  focused,
  others,
  strip,
  large,
  onPick,
}: {
  focused: TileView;
  others: TileView[];
  strip: "side" | "bottom";
  large: boolean;
  onPick: (key: string | null) => void;
}) {
  const side = strip === "side";
  const thumbHeight = large ? 110 : 64;

  return (
    <div
      style={{
        flex: 1,
        minHeight: 0,
        display: "flex",
        flexDirection: side ? "row" : "column",
        gap: space(GAP),
      }}
    >
      <div style={{ flex: 1, minWidth: 0, minHeight: 0 }}>
        <Tile tile={focused} focused onClick={() => onPick(null)} />
      </div>
      {others.length > 0 && (
        <div
          className={side ? "uwu-scroll" : undefined}
          style={{
            flex: "none",
            display: "flex",
            flexDirection: side ? "column" : "row",
            gap: space(8),
            ...(side
              ? { width: 172 }
              : {
                  height: thumbHeight,
                  overflowX: "auto",
                  overflowY: "hidden",
                  // `safe` keeps the first thumbnail reachable once the strip
                  // overflows; plain centring would push it off the left edge.
                  justifyContent: "safe center",
                }),
          }}
        >
          {others.map((t) => (
            <div
              key={t.key}
              style={
                side
                  ? { aspectRatio: "16 / 10", flex: "none" }
                  : { height: "100%", aspectRatio: "16 / 9", flex: "none" }
              }
            >
              <Tile tile={t} small={!large} onClick={() => onPick(t.key)} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * One participant's camera, or their shared screen.
 *
 * The video element is fed by attaching the LiveKit track directly — frames
 * can't travel through React state, so the track object comes down and the
 * element is wired to it here.
 */
function Tile({
  tile,
  focused,
  small,
  onClick,
}: {
  tile: TileView;
  /** On the stage: shown whole rather than cropped, and a click unfocuses. */
  focused?: boolean;
  /** A filmstrip thumbnail, too small for a full-size avatar. */
  small?: boolean;
  onClick: () => void;
}) {
  const { participant, track, screen } = tile;
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
      className="uwu-call-tile"
      role="button"
      tabIndex={0}
      title={focused ? "back to the grid" : "focus"}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick();
        }
      }}
      style={{
        position: "relative",
        width: "100%",
        height: "100%",
        borderRadius: corner(small ? 12 : 16),
        overflow: "hidden",
        background: "var(--ink-900)",
        border: "1px solid var(--border-subtle)",
        outline: participant.isSpeaking && !screen
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
            // Screens are letterboxed, and so is a focused face — it's on the
            // stage to be seen, not cropped. Grid faces are cropped to fill.
            objectFit: screen || focused ? "contain" : "cover",
            transform: participant.isLocal && !screen ? "scaleX(-1)" : undefined,
          }}
        />
      ) : (
        <Avatar
          id={participant.userId}
          name={name}
          size={small ? 30 : 56}
          radius={small ? 11 : 20}
        />
      )}

      <span
        className="uwu-call-tile-hint"
        style={{
          position: "absolute",
          top: 8,
          right: 8,
          width: 26,
          height: 26,
          borderRadius: corner(9),
          background: "color-mix(in srgb, var(--ink-950) 72%, transparent)",
          display: small ? "none" : "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <Icon
          name={focused ? "arrows-in" : "arrows-out"}
          size={13}
          color="var(--text-primary)"
        />
      </span>

      <div
        style={{
          position: "absolute",
          left: small ? 5 : 8,
          bottom: small ? 5 : 8,
          right: small ? 5 : 8,
          display: "flex",
          alignItems: "center",
          gap: space(6),
        }}
      >
        <span
          className="uwu-ellipsis"
          style={{
            padding: space(small ? "2px 7px" : "3px 9px"),
            borderRadius: 999,
            background: "color-mix(in srgb, var(--ink-950) 72%, transparent)",
            fontFamily: "var(--font-display)",
            fontWeight: 700,
            fontSize: textSize(small ? 10.5 : 12),
            color: "var(--text-primary)",
          }}
        >
          {participant.isLocal ? "you" : name}
          {screen ? " · screen" : ""}
        </span>
        {participant.isMuted && !screen && (
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
