/**
 * Turning Matrix data into the design's visual language: two-letter monograms,
 * the four neon accents, and lowercase relative times.
 */

export const NEON = [
  "var(--accent-primary)",
  "var(--accent-secondary)",
  "var(--accent-tertiary)",
  "var(--accent-quaternary)",
] as const;

/** Stable, well-distributed hash so a given ID always gets the same colour. */
function hash(input: string): number {
  let h = 2166136261;
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

export function accentFor(id: string): string {
  return NEON[hash(id) % NEON.length];
}

/**
 * Two characters for an avatar tile. Prefers the first letters of the first two
 * words ("tekno flyers" → "tf"), falls back to the first two of a single word.
 * Matrix IDs get their sigil stripped first so `@kii:uwu.gg` reads as "ki".
 */
export function initialsFor(name: string): string {
  const cleaned = name.replace(/^[@#!+]/, "").split(":")[0].trim();
  if (!cleaned) return "??";

  // Only letters and digits make it into a monogram: emoji and symbols
  // ("skitchy 🍁") would otherwise count as a word, and indexing by UTF-16 unit
  // splits them into a lone surrogate the font draws as a missing glyph.
  const words = cleaned
    .split(/[\s_\-.]+/)
    .map((word) => Array.from(word.replace(/[^\p{L}\p{N}]/gu, "")))
    .filter((chars) => chars.length > 0);
  if (words.length >= 2) {
    return (words[0][0] + words[1][0]).toLowerCase();
  }
  if (words.length === 1) {
    return words[0].slice(0, 2).join("").toLowerCase();
  }
  // Nothing but emoji/symbols: show the first whole character rather than "??".
  return Array.from(cleaned)[0];
}

/** The part of a Matrix ID people actually read: `@kii:uwu.gg` → `kii`. */
export function localpart(userId: string): string {
  return userId.replace(/^[@#!+]/, "").split(":")[0];
}

export function displayNameFor(
  userId: string,
  displayName?: string | null,
): string {
  return displayName?.trim() || localpart(userId);
}

const timeFormat = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

const dayFormat = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
});

const dateFormat = new Intl.DateTimeFormat(undefined, {
  day: "numeric",
  month: "short",
});

const fullDateFormat = new Intl.DateTimeFormat(undefined, {
  weekday: "long",
  day: "numeric",
  month: "long",
});

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** `19:04` — the exact time, used beside message authors. */
export function formatTime(timestamp: number): string {
  return timeFormat.format(new Date(timestamp));
}

/**
 * A compact stamp for the room list: time today, weekday this week, date beyond
 * that. Matches the design's `19:04` / `mon` mix.
 */
export function formatStamp(timestamp: number): string {
  if (!timestamp) return "";

  const then = new Date(timestamp);
  const daysApart = Math.round(
    (startOfDay(new Date()) - startOfDay(then)) / 86_400_000,
  );

  if (daysApart <= 0) return timeFormat.format(then);
  if (daysApart === 1) return "yest";
  if (daysApart < 7) return dayFormat.format(then).toLowerCase();
  return dateFormat.format(then).toLowerCase();
}

/** The pill that separates days in the timeline. */
export function formatDayDivider(timestamp: number): string {
  const then = new Date(timestamp);
  const daysApart = Math.round(
    (startOfDay(new Date()) - startOfDay(then)) / 86_400_000,
  );

  if (daysApart <= 0) return "today";
  if (daysApart === 1) return "yesterday";
  return fullDateFormat.format(then).toLowerCase();
}

export function formatBytes(bytes: number | null): string {
  if (bytes == null) return "";
  const units = ["b", "kb", "mb", "gb"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value < 10 && unit > 0 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export function formatDuration(ms: number | null): string {
  if (ms == null) return "";
  const total = Math.round(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

/** Join names the way the typing indicator reads them out. */
export function joinNames(names: string[]): string {
  if (names.length === 0) return "";
  if (names.length === 1) return names[0];
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  if (names.length === 3) return `${names[0]}, ${names[1]} and ${names[2]}`;
  return `${names[0]}, ${names[1]} and ${names.length - 2} others`;
}

/**
 * How long ago someone was last active, in the room list's voice.
 *
 * The server gives an age rather than an instant, and the backend anchors it at
 * poll time — so this keeps counting between polls instead of freezing at
 * whatever the last round trip said.
 */
export function formatLastSeen(lastActive: number | null): string {
  if (!lastActive) return "";

  const seconds = Math.max(0, Math.round((Date.now() - lastActive) / 1000));
  if (seconds < 90) return "just now";

  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;

  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;

  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d ago`;

  return `on ${dateFormat.format(new Date(lastActive)).toLowerCase()}`;
}
