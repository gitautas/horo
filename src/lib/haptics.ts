/**
 * A tap you can feel, on a phone.
 *
 * The plugin is only compiled into the Android and iOS builds (see Cargo.toml),
 * so on desktop the call rejects with "plugin not found". Feedback is never
 * worth an error banner, so every failure is swallowed — callers fire and
 * forget.
 */
import { impactFeedback, type ImpactFeedbackStyle } from "@tauri-apps/plugin-haptics";

export function haptic(style: ImpactFeedbackStyle = "light"): void {
  impactFeedback(style).catch(() => {});
}
