/**
 * The two faces of the updater: a banner that appears on its own when a new
 * version turns up, and a block in settings → about for people who'd rather
 * ask than be told.
 *
 * Both render the same state machine (`lib/update.ts`), so there is exactly one
 * place where "downloading" or "restart to finish" is described, and the two
 * can't drift apart. The banner adds only its own dismissal; the settings block
 * adds only the button that starts a check.
 */

import { useShallow } from "zustand/react/shallow";

import { useUpdate } from "../lib/update";
import { Card } from "./settingsUi";
import { Button, Icon, Spinner } from "./ui";
import { corner, space, textSize } from "../lib/scale";

/** Shared by both faces: what this phase says, and what the button does. */
function useUpdateCopy() {
  const state = useUpdate(
    useShallow((s) => ({
      phase: s.phase,
      mode: s.mode,
      version: s.version,
      progress: s.progress,
      message: s.message,
      install: s.install,
      relaunch: s.relaunch,
    })),
  );

  const { phase, mode, version, progress } = state;

  const percent = progress === null ? null : Math.round(progress * 100);

  const line =
    phase === "checking"
      ? "looking for a new version…"
      : phase === "available"
        ? mode === "manual"
          ? `horo ${version} is out — this build updates from where you installed it`
          : `horo ${version} is ready to install`
        : phase === "downloading"
          ? percent === null
            ? "downloading the update…"
            : `downloading the update… ${percent}%`
          : phase === "ready"
            ? "update installed — restart to finish"
            : state.message;

  // One action per phase, or none. `available` is the only one that branches,
  // because on a manual build "install" means "open the release page".
  const action =
    phase === "available"
      ? { label: mode === "manual" ? "open release" : "install", run: state.install }
      : phase === "ready"
        ? { label: "restart", run: state.relaunch }
        : null;

  return { ...state, line, action, percent };
}

/**
 * The banner. Top-centre rather than bottom, because the app's own error
 * banner already owns the bottom and the two can be on screen together.
 *
 * Only ever shows for a version the user hasn't waved away, and never for the
 * quiet phases — a background check that finds nothing says nothing.
 */
export function UpdateBanner() {
  const { phase, version, dismissed, dismiss } = useUpdate(
    useShallow((s) => ({
      phase: s.phase,
      version: s.version,
      dismissed: s.dismissed,
      dismiss: s.dismiss,
    })),
  );
  const { line, action, progress } = useUpdateCopy();

  const worth = phase === "available" || phase === "downloading" || phase === "ready";
  if (!worth || (version !== null && version === dismissed)) return null;

  return (
    <div
      style={{
        position: "fixed",
        top: 14,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: 200,
        display: "flex",
        alignItems: "center",
        gap: space(12),
        padding: space("10px 16px"),
        borderRadius: corner(18),
        maxWidth: 520,
        overflow: "hidden",
        background: "var(--surface-card-raised)",
        border: "1px solid var(--accent-primary)",
        boxShadow: "var(--shadow-pop)",
      }}
    >
      {phase === "downloading" ? (
        <Spinner size={14} />
      ) : (
        <Icon name="sparkle" size={15} color="var(--accent-primary)" />
      )}

      <span style={{ fontSize: textSize(13), color: "var(--text-secondary)", lineHeight: 1.45 }}>
        {line}
      </span>

      {action && (
        <Button size="sm" onClick={() => void action.run()}>
          {action.label}
        </Button>
      )}

      {/* Dismissing a finished download only hides the nudge; the update is
          already on disk and applies on the next launch either way. */}
      <button onClick={dismiss} style={{ cursor: "pointer", display: "flex" }}>
        <Icon name="x" size={13} color="var(--text-tertiary)" />
      </button>

      {phase === "downloading" && progress !== null && (
        <div
          style={{
            position: "absolute",
            left: 0,
            bottom: 0,
            height: 2,
            width: `${progress * 100}%`,
            background: "var(--accent-primary)",
            transition: "width var(--dur-fast) linear",
          }}
        />
      )}
    </div>
  );
}

/** The settings → about block: the same state, plus a way to start a check. */
export function UpdateSettings() {
  const { phase, mode, line, action, check } = {
    ...useUpdateCopy(),
    check: useUpdate((s) => s.check),
  };

  const busy = phase === "checking" || phase === "downloading";

  return (
    <Card tone={phase === "error" ? "warning" : undefined}>
      <div style={{ display: "flex", alignItems: "center", gap: space(12) }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: textSize(13.5), color: "var(--text-primary)" }}>updates</div>
          <div
            style={{
              marginTop: 4,
              fontSize: textSize(12),
              color: phase === "error" ? "var(--status-warning)" : "var(--text-tertiary)",
              lineHeight: 1.5,
            }}
          >
            {line ||
              (mode === "manual"
                ? "this build is installed by your package manager or app store"
                : "checked automatically every few hours")}
          </div>
        </div>

        {action ? (
          <Button size="sm" onClick={() => void action.run()}>
            {action.label}
          </Button>
        ) : (
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => void check(true)}>
            {busy ? "checking…" : "check now"}
          </Button>
        )}
      </div>
    </Card>
  );
}
