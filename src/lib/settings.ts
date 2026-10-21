/**
 * Local preferences — things that belong to this machine rather than the
 * account, so they live in `localStorage` and never touch the homeserver.
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";

import type { SkinTone } from "./emoji";

export type Accent = "acid" | "pink" | "violet" | "cyan";

/**
 * A stylesheet of token overrides, keyed by `data-theme` on `<html>`: `classic`
 * is `tokens/` as written, `horo` is `themes/horo.css` on top.
 */
export type Theme = "horo" | "classic";

/** How far the theme's surfaces are pushed toward black. */
export type Shade = "dim" | "dark" | "deep" | "black";

/** How strongly quiet text and hairline borders stand out. */
export type Contrast = "soft" | "normal" | "high";

export interface Settings {
  theme: Theme;
  accent: Accent;
  shade: Shade;
  contrast: Contrast;
  /**
   * The CSS font family for body text, or "" for the design's own. Anything
   * installed on the machine works; a name that isn't falls back to the default.
   */
  fontFamily: string;
  /** Size of message text, in px. The rest of the UI keeps its own sizes. */
  messageFontSize: number;
  /** Page zoom for the whole window, as a factor: 1 is the design's own size. */
  zoom: number;
  /** `deviceId` of the microphone, or "" for the system default. */
  audioInput: string;
  /** `deviceId` of the speaker, or "" for the system default. */
  audioOutput: string;
  /** `deviceId` of the camera, or "" for the system default. */
  videoInput: string;
  /**
   * The noise gate's threshold, in dBFS: the microphone is silenced while it's
   * quieter than this. At `GATE_OFF_DB` or below, nothing is ever cut.
   */
  inputSensitivity: number;
  /** Show the room info panel beside the timeline. */
  showInfoPanel: boolean;
  /** Width of the room list, in px, within `ROOM_LIST_WIDTH_RANGE`. */
  roomListWidth: number;
  /** Width of the room info panel, in px, within `INFO_PANEL_WIDTH_RANGE`. */
  infoPanelWidth: number;
  /** Which skin tone the emoji picker offers. 0 is the default yellow. */
  skinTone: SkinTone;
  /** Show a desktop notification when a message arrives you'd be pinged for. */
  notifyMessages: boolean;
  /** Show a desktop notification when a call starts in a room you're in. */
  notifyCalls: boolean;
  /**
   * Notify even while you're looking at the app.
   *
   * Off by default: a banner for the room you're already reading is noise.
   */
  notifyWhenFocused: boolean;
  /** Id from `sounds.MESSAGE_SOUNDS`, or `"none"`. */
  messageSound: string;
  /** Id from `sounds.CALL_SOUNDS`, or `"none"`. */
  callSound: string;
  /** How loud the notification sounds are, 0 to 1. */
  notificationVolume: number;
  /**
   * Reactions you've picked lately, most recent first.
   *
   * Kept here rather than on the account because it's a habit of this machine,
   * and because writing account data on every reaction would be a lot of
   * traffic for something nobody else can see.
   */
  recentReactions: string[];
  /** Spaces whose room list leaves muted rooms out — Discord's "hide muted channels". */
  hideMutedInSpaces: string[];
}

/**
 * The bottom of the sensitivity slider, which stands in for −∞: a gate here
 * lets everything through. A number rather than `-Infinity` because settings
 * go through JSON, which would turn it into `null`.
 */
export const GATE_OFF_DB = -80;

/** How many recent reactions to remember — one row of the hover bar. */
export const MAX_RECENT_REACTIONS = 6;

export const DEFAULTS: Settings = {
  theme: "horo",
  accent: "acid",
  shade: "dark",
  contrast: "normal",
  fontFamily: "",
  messageFontSize: 14.5,
  zoom: 1,
  audioInput: "",
  audioOutput: "",
  videoInput: "",
  // Wide open until someone drags it up: a threshold set wrong cuts people off
  // mid-word, which is worse than the background noise it was meant to hide.
  inputSensitivity: GATE_OFF_DB,
  showInfoPanel: true,
  roomListWidth: 298,
  infoPanelWidth: 296,
  skinTone: 0,
  notifyMessages: true,
  notifyCalls: true,
  notifyWhenFocused: false,
  messageSound: "blip",
  callSound: "ring",
  notificationVolume: 0.7,
  // A first-run row that isn't empty, in the house voice. These are replaced by
  // real usage as soon as the user reacts to anything.
  recentReactions: ["💜", "😹", "🥺", "✨", "👀", "🔥"],
  hideMutedInSpaces: [],
};

/** The range the message text size slider covers, in px. */
export const MESSAGE_FONT_SIZE_RANGE = { min: 12, max: 22, step: 0.5 } as const;

/**
 * How far the desktop sidebars can be dragged, in px.
 *
 * The floor is where the room list's header row and the info panel's member
 * rows start to truncate; the ceiling keeps either from eating the timeline.
 */
export const ROOM_LIST_WIDTH_RANGE = { min: 240, max: 420 } as const;
export const INFO_PANEL_WIDTH_RANGE = { min: 240, max: 420 } as const;

/** The range the zoom slider and shortcuts cover, as a factor. */
export const ZOOM_RANGE = { min: 0.5, max: 2, step: 0.1 } as const;

/**
 * The zoom one shortcut press away from `zoom`, clamped to `ZOOM_RANGE`.
 *
 * Rounded to the step so repeated presses don't accumulate float error and
 * land on 1.0000000002 instead of back on 1.
 */
export function stepZoom(zoom: number, direction: 1 | -1): number {
  const { min, max, step } = ZOOM_RANGE;
  const next = Math.round((zoom + direction * step) / step) * step;
  return Math.min(max, Math.max(min, Number(next.toFixed(2))));
}

/** One-click choices for the font picker; anything else is typed in by name. */
export const FONT_PRESETS: { label: string; family: string }[] = [
  { label: "default", family: "" },
  { label: "system", family: "system-ui" },
  { label: "baloo", family: "'Baloo 2'" },
  { label: "mono", family: "'JetBrains Mono Variable'" },
];

// Still under the pre-rename name, so existing settings carry over.
const STORAGE_KEY = "uwum:settings";

/**
 * The design's four neon accents, as the palette tokens they come from.
 *
 * Each accent is the 500/400/600 step of one ramp — base, hover, pressed —
 * which is exactly how the design system defines them. Naming the tokens rather
 * than repeating their hex keeps `tokens/colors.css` the only place a colour is
 * written down.
 */
const ACCENTS: Record<Accent, [string, string, string]> = {
  acid: ["--acid-500", "--acid-400", "--acid-600"],
  pink: ["--pink-500", "--pink-400", "--pink-600"],
  violet: ["--violet-500", "--violet-400", "--violet-600"],
  cyan: ["--cyan-500", "--cyan-400", "--cyan-600"],
};

export const ACCENT_SWATCHES = Object.entries(ACCENTS).map(([name, [base]]) => ({
  name: name as Accent,
  colour: `var(${base})`,
}));

export const THEMES: { name: Theme; label: string; hint: string }[] = [
  { name: "horo", label: "horo", hint: "grounded and compact, in the logo's colours" },
  { name: "classic", label: "classic", hint: "the original neon rave flyer" },
];

/**
 * The options for shade and contrast, with what each mixes in: `--shade` is how
 * much black goes into the surfaces, `--contrast` how many points of opacity
 * quiet text gains (borders get half). `dim` and `soft` are the themes exactly
 * as drawn; the defaults sit a step past them.
 */
export const SHADES: { name: Shade; label: string; mix: string }[] = [
  { name: "dim", label: "dim", mix: "0%" },
  { name: "dark", label: "dark", mix: "18%" },
  { name: "deep", label: "deep", mix: "38%" },
  { name: "black", label: "near black", mix: "70%" },
];

export const CONTRASTS: { name: Contrast; label: string; lift: string }[] = [
  { name: "soft", label: "soft", lift: "0%" },
  { name: "normal", label: "normal", lift: "8%" },
  { name: "high", label: "high", lift: "20%" },
];

/**
 * What each accent is called under each theme. The ramps keep their classic
 * names in code, but horo fills them with different colours.
 */
export const ACCENT_LABELS: Record<Theme, Record<Accent, string>> = {
  horo: { acid: "gold", pink: "clay", violet: "heather", cyan: "sage" },
  classic: { acid: "acid", pink: "pink", violet: "violet", cyan: "cyan" },
};

export function load(): Settings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    // Merge over the defaults so a settings file written by an older build
    // doesn't leave new keys undefined.
    return { ...DEFAULTS, ...(JSON.parse(raw) as Partial<Settings>) };
  } catch {
    return { ...DEFAULTS };
  }
}

export function save(settings: Settings): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // A full or disabled localStorage shouldn't break the app; the user just
    // loses their preferences on restart.
  }
}

/** Switch the document to a theme's stylesheet. */
export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = THEMES.some((t) => t.name === theme)
    ? theme
    : DEFAULTS.theme;
}

/** Paint the chosen accent onto the document, as the design's `accent` prop does. */
export function applyAccent(accent: Accent): void {
  const [base, hover, press] = ACCENTS[accent] ?? ACCENTS.acid;
  const root = document.documentElement;
  // A custom property can hold a `var()` reference, so the accent aliases point
  // at palette tokens rather than carrying copies of their values.
  root.style.setProperty("--accent-primary", `var(${base})`);
  root.style.setProperty("--accent-primary-hover", `var(${hover})`);
  root.style.setProperty("--accent-primary-press", `var(${press})`);
}

/** Paint the chosen shade and contrast onto the document, for `tokens/tone.css`. */
export function applyTone({ shade, contrast }: Pick<Settings, "shade" | "contrast">): void {
  const root = document.documentElement;
  const mix = (SHADES.find((s) => s.name === shade) ?? SHADES[1]).mix;
  const lift = (CONTRASTS.find((c) => c.name === contrast) ?? CONTRASTS[1]).lift;
  root.style.setProperty("--shade", mix);
  root.style.setProperty("--contrast", lift);
}

/**
 * Paint the chosen font and message size onto the document.
 *
 * A custom family is put in front of the theme's own stack rather than replacing
 * it, so a misspelt or uninstalled font lands on that instead of Times. The
 * stack is referenced, not copied, so it follows a theme switch.
 */
export function applyTypography({
  fontFamily,
  messageFontSize,
}: Pick<Settings, "fontFamily" | "messageFontSize">): void {
  const root = document.documentElement;
  const family = fontFamily.trim();
  root.style.setProperty(
    "--font-body",
    family ? `${family}, var(--font-body-default)` : "var(--font-body-default)",
  );
  root.style.setProperty("--text-message", `${messageFontSize}px`);
}

/**
 * Zoom the web view, as the browser's own cmd+/cmd- would.
 *
 * Real page zoom (WebKit's `pageZoom`, WebView2's `ZoomFactor`) rather than a
 * CSS `zoom` on the root: it scales the layout viewport too, so the mobile
 * breakpoint and the desktop shell's width floor see the zoomed size, and
 * nothing measured in px from JS comes back scaled wrong.
 *
 * Outside Tauri (the browser harness, tests) there's no web view to zoom, and
 * a platform that can't do it just stays at 1 — neither is worth an error.
 */
export function applyZoom(zoom: number): void {
  try {
    void getCurrentWebview()
      .setZoom(zoom)
      .catch(() => {});
  } catch {
    // Not running in a Tauri web view.
  }
}

export interface AudioDevice {
  deviceId: string;
  label: string;
}

export interface MediaDevices {
  inputs: AudioDevice[];
  outputs: AudioDevice[];
  cameras: AudioDevice[];
  /**
   * The `deviceId` the system default currently resolves to, so a picker left
   * on "" can show the device that's actually in use. Null when we can't tell.
   */
  defaults: { input: string | null; output: string | null; camera: string | null };
}

/**
 * Chromium lists the defaults as extra `"default"` and `"communications"`
 * entries labelled "Default - <name>". They duplicate a real device, so they
 * come out of the list, and are only used to work out which one is default.
 */
const PSEUDO_DEVICES = new Set(["default", "communications"]);

/**
 * List the microphones, speakers and cameras we're allowed to see.
 *
 * Labels are blank until the user has granted access at least once, so this
 * asks for permission first — otherwise the picker shows a list of anonymous
 * "Device 1", "Device 2" entries that nobody can choose between.
 *
 * Camera permission is requested separately from the microphone: asking for
 * both at once means a refusal of either loses both sets of labels.
 */
export async function listMediaDevices(): Promise<MediaDevices> {
  // The track we get back from an unconstrained request is whatever the system
  // default is, which is the only portable way to find out: WebKit has no
  // "default" entry in `enumerateDevices`, and cameras have one nowhere.
  const granted: { audio?: string; video?: string } = {};
  for (const kind of ["audio", "video"] as const) {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ [kind]: true });
      granted[kind] = stream.getTracks()[0]?.getSettings().deviceId || undefined;
      // We only wanted the permission and the device, not the media.
      stream.getTracks().forEach((track) => track.stop());
    } catch {
      // Denied or absent — we can still enumerate, just without labels.
    }
  }

  const devices = await navigator.mediaDevices.enumerateDevices();
  const real = (kind: MediaDeviceKind) =>
    devices.filter((d) => d.kind === kind && !PSEUDO_DEVICES.has(d.deviceId));
  const pick = (kind: MediaDeviceKind, fallback: string) =>
    real(kind).map((d, i) => ({
      deviceId: d.deviceId,
      label: d.label || `${fallback} ${i + 1}`,
    }));

  /**
   * The real device behind `id`, which may be one of Chromium's pseudo
   * entries: those share a `groupId` with the device they stand for.
   */
  const resolve = (kind: MediaDeviceKind, id: string | undefined) => {
    if (!id) return null;
    if (!PSEUDO_DEVICES.has(id)) return id;
    const pseudo = devices.find((d) => d.kind === kind && d.deviceId === id);
    return real(kind).find((d) => pseudo && d.groupId === pseudo.groupId)?.deviceId ?? null;
  };

  return {
    inputs: pick("audioinput", "microphone"),
    outputs: pick("audiooutput", "speaker"),
    cameras: pick("videoinput", "camera"),
    defaults: {
      input: resolve("audioinput", granted.audio ?? "default"),
      // Nothing to open for a speaker, so outside Chromium the best guess is the
      // first one listed, which is the order the system hands them over in.
      output:
        resolve("audiooutput", "default") ?? real("audiooutput")[0]?.deviceId ?? null,
      camera: resolve("videoinput", granted.video),
    },
  };
}
