/**
 * The microphone half of voice settings: a live level meter, and the noise
 * gate you tune against it. The gate is always there; all the way left it lets
 * everything through, which is how it ships.
 *
 * Laid out the way Discord does input sensitivity — the threshold is a marker
 * on the meter itself, and the bar changes colour depending on which side of
 * it you are — because that's the one layout where setting a threshold
 * doesn't take guesswork.
 */

import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { call } from "../lib/call";
import {
  gateSupported,
  gateThreshold,
  METER_FLOOR_DB,
  MicTest,
  type GateReading,
} from "../lib/noiseGate";
import { useStore } from "../store";
import { useCallState } from "./CallBar";
import { Field } from "./settingsUi";
import { space, textSize } from "../lib/scale";

/** Where a level lands on the meter, 0 to 100. */
function percent(db: number): number {
  const clamped = Math.min(0, Math.max(METER_FLOOR_DB, db));
  return ((clamped - METER_FLOOR_DB) / -METER_FLOOR_DB) * 100;
}

export function MicSettings() {
  const { settings, updateSettings } = useStore(
    useShallow((s) => ({ settings: s.settings, updateSettings: s.updateSettings })),
  );
  const callState = useCallState();
  const inCall = callState.status === "connected" || callState.status === "reconnecting";

  const threshold = gateThreshold(settings);
  const thresholdRef = useRef(threshold);
  thresholdRef.current = threshold;

  const fill = useRef<HTMLDivElement>(null);
  const testRef = useRef<MicTest | null>(null);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Mid-call, read the call's own gate so the meter shows what's being sent;
  // otherwise open a test capture of our own. Readings go straight to the DOM:
  // thirty React renders a second for a bar is a waste.
  useEffect(() => {
    let cancelled = false;
    let unsubscribe: (() => void) | null = null;
    let test: MicTest | null = null;

    const paint = ({ level, open }: GateReading) => {
      if (fill.current) fill.current.style.width = `${percent(level)}%`;
      setOpen(open);
    };

    setError(null);
    const fromCall = inCall ? call.onMicReading(paint) : null;
    if (fromCall) {
      unsubscribe = fromCall;
    } else if (gateSupported()) {
      MicTest.open(settings.audioInput, thresholdRef.current)
        .then((opened) => {
          if (cancelled) {
            void opened.close();
            return;
          }
          test = opened;
          testRef.current = opened;
          unsubscribe = opened.onReading(paint);
        })
        .catch(() => {
          if (!cancelled) setError("couldn't open the microphone to test it.");
        });
    } else {
      setError("this webview can't measure the microphone, so there's no meter here.");
    }

    return () => {
      cancelled = true;
      unsubscribe?.();
      void test?.close();
      testRef.current = null;
      if (fill.current) fill.current.style.width = "0%";
    };
  }, [settings.audioInput, inCall, callState.gateAttached]);

  useEffect(() => {
    testRef.current?.setThreshold(threshold);
  }, [threshold]);

  function setSensitivity(inputSensitivity: number) {
    updateSettings({ inputSensitivity });
    call.setNoiseGate(gateThreshold({ ...settings, inputSensitivity }));
  }

  const gated = threshold !== null;
  const muted = inCall && !callState.micEnabled;
  const colour = !gated || open ? "var(--accent-primary)" : "var(--status-warning)";

  return (
    <>
      <Field
        label={`input sensitivity — ${gated ? `${settings.inputSensitivity} db` : "−∞ db"}`}
        hint={
          error ??
          (muted
            ? "you're muted in the call, so there's nothing to show."
            : gated
              ? "anything quieter than the marker is cut. talk normally — the bar lights up when you'd be heard. drag the marker all the way left to let everything through."
              : "everything gets through. to cut out background noise, stay quiet and drag the marker just past where the bar stops.")
        }
      >
        <div style={{ display: "flex", alignItems: "center", gap: space(10) }}>
          <div
            style={{
              position: "relative",
              flex: 1,
              height: 18,
              borderRadius: 999,
              background: "var(--surface-inset)",
              border: "1px solid var(--border-subtle)",
            }}
          >
            <div
              style={{
                position: "absolute",
                inset: 0,
                borderRadius: 999,
                overflow: "hidden",
              }}
            >
              <div
                ref={fill}
                style={{
                  height: "100%",
                  width: "0%",
                  background: colour,
                  opacity: gated && !open ? 0.7 : 1,
                  transition:
                    "width 60ms linear, background-color var(--dur-fast) var(--ease-out)",
                }}
              />
            </div>
            <input
              type="range"
              className="uwu-gate-slider"
              min={METER_FLOOR_DB}
              max={0}
              step={1}
              value={Math.max(METER_FLOOR_DB, settings.inputSensitivity)}
              onChange={(e) => setSensitivity(Number(e.target.value))}
              aria-label="input sensitivity"
            />
          </div>

          {gated && (
            <span
              style={{
                width: 58,
                flex: "none",
                textAlign: "right",
                fontFamily: "var(--font-mono)",
                fontSize: textSize(10.5),
                color: open ? "var(--accent-primary)" : "var(--text-tertiary)",
              }}
            >
              {open ? "sending" : "gated"}
            </span>
          )}
        </div>
      </Field>
    </>
  );
}
