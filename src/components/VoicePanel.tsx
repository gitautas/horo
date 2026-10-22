import { call, screenShareSupported } from "../lib/call";
import * as ipc from "../lib/ipc";
import { useStore } from "../store";
import { useCallState } from "./CallBar";
import { Icon } from "./ui";
import { corner, space, textSize } from "../lib/scale";

/**
 * The "voice connected" panel at the foot of the sidebar, where Discord keeps
 * it.
 *
 * It's the one home for the call's controls, so a call with nobody on camera
 * needs nothing in the room itself — the chat keeps the whole pane — and the
 * call stays in reach from whichever room is open.
 */
export function VoicePanel() {
  const state = useCallState();
  const room = useStore((s) => s.rooms.find((r) => r.id === state.roomId));
  const selectRoom = useStore((s) => s.selectRoom);
  const showBanner = useStore((s) => s.showBanner);

  if (state.status === "idle") return null;

  const connected = state.status === "connected";
  const failed = state.status === "failed";
  const status = connected
    ? "voice connected"
    : failed
      ? "couldn't connect"
      : state.status === "reconnecting"
        ? "reconnecting…"
        : "connecting…";
  const colour = connected
    ? "var(--status-online)"
    : failed
      ? "var(--status-danger)"
      : "var(--status-warning)";

  async function run(action: () => Promise<void>) {
    try {
      await action();
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  return (
    <div
      style={{
        flex: "none",
        margin: space("0 10px calc(var(--safe-bottom) + 10px)"),
        padding: space("10px 10px 10px 12px"),
        borderRadius: corner(16),
        background: "var(--surface-card)",
        border: "1px solid var(--border-subtle)",
        display: "flex",
        flexDirection: "column",
        gap: space(10),
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: space(10) }}>
        <Icon name="cell-signal-full" size={16} color={colour} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              fontFamily: "var(--font-display)",
              fontWeight: 700,
              fontSize: textSize(13.5),
              color: colour,
            }}
          >
            {status}
          </div>
          <button
            onClick={() => state.roomId && void selectRoom(state.roomId)}
            title="go to the room"
            className="uwu-ellipsis"
            style={{
              display: "block",
              maxWidth: "100%",
              padding: 0,
              textAlign: "left",
              cursor: "pointer",
              fontSize: textSize(12),
              color: "var(--text-tertiary)",
            }}
          >
            {room?.name ?? "voice"}
          </button>
        </div>
        <PanelButton
          icon="phone-x"
          title="disconnect"
          danger
          onClick={() => void call.leave()}
        />
      </div>

      <div style={{ display: "flex", gap: space(6) }}>
        <PanelButton
          wide
          icon={state.micEnabled ? "microphone" : "microphone-slash"}
          title={state.micEnabled ? "mute" : "unmute"}
          off={!state.micEnabled}
          onClick={() => void run(() => call.setMicEnabled(!state.micEnabled))}
        />
        <PanelButton
          wide
          icon={state.deafened ? "speaker-slash" : "speaker-high"}
          title={state.deafened ? "undeafen" : "deafen"}
          off={state.deafened}
          onClick={() => void run(() => call.setDeafened(!state.deafened))}
        />
        <PanelButton
          wide
          icon={state.cameraEnabled ? "video-camera" : "video-camera-slash"}
          title={state.cameraEnabled ? "turn camera off" : "turn camera on"}
          on={state.cameraEnabled}
          onClick={() => void run(() => call.setCameraEnabled(!state.cameraEnabled))}
        />
        {screenShareSupported() && (
          <PanelButton
            wide
            icon="monitor-arrow-up"
            title={state.screenShareEnabled ? "stop sharing" : "share your screen"}
            on={state.screenShareEnabled}
            onClick={() =>
              void run(() => call.setScreenShareEnabled(!state.screenShareEnabled))
            }
          />
        )}
      </div>
    </div>
  );
}

function PanelButton({
  icon,
  title,
  onClick,
  wide,
  on,
  off,
  danger,
}: {
  icon: string;
  title: string;
  onClick: () => void;
  /** Shares the row with its neighbours instead of sitting at its own size. */
  wide?: boolean;
  /** Sending something — camera or screen — so lit up. */
  on?: boolean;
  /** Muted or deafened: the state worth noticing. */
  off?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      aria-label={title}
      aria-pressed={on || off || undefined}
      style={{
        flex: wide ? 1 : "none",
        width: wide ? undefined : 32,
        height: 32,
        borderRadius: corner(10),
        background: on ? "var(--accent-primary)" : "var(--surface-inset)",
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
          on
            ? "var(--text-on-accent)"
            : off || danger
              ? "var(--status-danger)"
              : "var(--text-secondary)"
        }
      />
    </button>
  );
}
