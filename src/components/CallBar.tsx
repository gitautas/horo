import { useEffect, useState } from "react";

import { call, IDLE_CALL, type CallState } from "../lib/call";
import { useStore } from "../store";
import { Icon } from "./ui";
import { corner, space, textSize } from "../lib/scale";

export function useCallState(): CallState {
  const [state, setState] = useState<CallState>(IDLE_CALL);
  useEffect(() => call.subscribe(setState), []);
  return state;
}

/** The strip above the composer while a call is running, on a phone. */
export function CallBar() {
  const state = useCallState();
  const rooms = useStore((s) => s.rooms);
  const selectRoom = useStore((s) => s.selectRoom);

  if (state.status === "idle") return null;

  const room = rooms.find((r) => r.id === state.roomId);
  const connecting = state.status === "connecting";
  const reconnecting = state.status === "reconnecting";

  return (
    <div
      style={{
        margin: "0 22px 10px",
        display: "flex",
        alignItems: "center",
        gap: space(14),
        padding: space("11px 14px"),
        borderRadius: corner(20),
        background: "var(--surface-card-raised)",
        border: `1px solid ${reconnecting ? "color-mix(in srgb, var(--amber-500) 40%, transparent)" : "color-mix(in srgb, var(--acid-500) 35%, transparent)"}`,
        boxShadow: reconnecting ? "none" : "0 0 24px color-mix(in srgb, var(--acid-500) 12%, transparent)",
      }}
    >
      <div
        style={{
          width: 34,
          height: 34,
          flex: "none",
          borderRadius: corner(13),
          background: reconnecting ? "var(--status-warning)" : "var(--accent-primary)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          animation: connecting ? "uwuPulse 1.2s infinite" : undefined,
        }}
      >
        <Icon name="phone-call" size={16} color="var(--ink-950)" />
      </div>

      <div style={{ minWidth: 0, flex: 1 }}>
        <button
          onClick={() => state.roomId && void selectRoom(state.roomId)}
          className="uwu-ellipsis"
          style={{
            fontFamily: "var(--font-display)",
            fontWeight: 700,
            fontSize: textSize(14),
            cursor: "pointer",
            display: "block",
            textAlign: "left",
            maxWidth: "100%",
          }}
        >
          {room?.name ?? "voice"}
        </button>
        <div
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10.5),
            color: "var(--text-tertiary)",
          }}
        >
          {connecting
            ? "connecting…"
            : reconnecting
              ? "reconnecting…"
              : `${state.participants.length} in the call`}
        </div>
      </div>

      <div style={{ display: "flex", gap: space(6) }}>
        <CallButton
          icon={state.micEnabled ? "microphone" : "microphone-slash"}
          active={!state.micEnabled}
          title={state.micEnabled ? "mute" : "unmute"}
          onClick={() => void call.setMicEnabled(!state.micEnabled)}
        />
        <CallButton
          icon={state.deafened ? "speaker-slash" : "speaker-high"}
          active={state.deafened}
          title={state.deafened ? "undeafen" : "deafen"}
          onClick={() => void call.setDeafened(!state.deafened)}
        />
        <CallButton
          icon={state.cameraEnabled ? "video-camera" : "video-camera-slash"}
          active={!state.cameraEnabled}
          title={state.cameraEnabled ? "turn camera off" : "turn camera on"}
          onClick={() => void call.setCameraEnabled(!state.cameraEnabled)}
        />
        <CallButton
          icon="phone-x"
          danger
          title="leave the call"
          onClick={() => void call.leave()}
        />
      </div>
    </div>
  );
}

function CallButton({
  icon,
  onClick,
  title,
  active,
  danger,
}: {
  icon: string;
  onClick: () => void;
  title: string;
  active?: boolean;
  danger?: boolean;
}) {
  const background = danger
    ? "var(--status-danger)"
    : active
      ? "var(--surface-inset)"
      : "var(--surface-card)";

  return (
    <button
      onClick={onClick}
      title={title}
      style={{
        width: 34,
        height: 34,
        borderRadius: corner(12),
        background,
        border: "1px solid var(--border-subtle)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
      }}
    >
      <Icon
        name={icon}
        size={15}
        color={
          danger
            ? "var(--ink-950)"
            : active
              ? "var(--status-warning)"
              : "var(--text-secondary)"
        }
      />
    </button>
  );
}
