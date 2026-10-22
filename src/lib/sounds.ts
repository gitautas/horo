/**
 * The built-in notification sounds.
 *
 * They're synthesised with the Web Audio API rather than shipped as files. A
 * notification chirp is a handful of sine and square tones with an envelope on
 * them, which is a few lines of code and no bytes in the bundle — and it means
 * the set can be tuned by editing numbers rather than by finding, licensing and
 * committing a pile of audio.
 */

type Wave = OscillatorType;

interface Note {
  /** Seconds after the sound starts. */
  at: number;
  /** Hertz. */
  freq: number;
  /** Seconds the tone rings for, envelope included. */
  dur: number;
  /** Slide to this frequency over the note's life, for chirps and pops. */
  to?: number;
  wave?: Wave;
  /** Relative loudness, before the user's volume is applied. */
  level?: number;
  /** Hold at full level and only fade at the very end, instead of decaying. */
  sustain?: boolean;
}

export interface Sound {
  id: string;
  label: string;
  notes: Note[];
}

/** Equal temperament, so the tunes below can be written as note names. */
function hz(semitonesFromA4: number): number {
  return 440 * 2 ** (semitonesFromA4 / 12);
}

/** Short, quiet, and over before you've finished reading the message. */
export const MESSAGE_SOUNDS: Sound[] = [
  {
    id: "blip",
    label: "blip",
    notes: [
      { at: 0, freq: hz(7), dur: 0.09, wave: "triangle" },
      { at: 0.075, freq: hz(14), dur: 0.13, wave: "triangle" },
    ],
  },
  {
    id: "pop",
    label: "pop",
    notes: [{ at: 0, freq: 620, to: 240, dur: 0.13, wave: "sine", level: 1.1 }],
  },
  {
    id: "bubble",
    label: "bubble",
    notes: [{ at: 0, freq: 300, to: 900, dur: 0.12, wave: "sine", level: 1.1 }],
  },
  {
    id: "chime",
    label: "chime",
    notes: [
      { at: 0, freq: hz(4), dur: 0.5, wave: "sine" },
      { at: 0.02, freq: hz(11), dur: 0.55, wave: "sine", level: 0.7 },
      { at: 0.04, freq: hz(16), dur: 0.6, wave: "sine", level: 0.5 },
    ],
  },
  {
    id: "sparkle",
    label: "sparkle",
    notes: [
      { at: 0, freq: hz(16), dur: 0.08, wave: "sine" },
      { at: 0.06, freq: hz(21), dur: 0.08, wave: "sine", level: 0.8 },
      { at: 0.12, freq: hz(28), dur: 0.22, wave: "sine", level: 0.6 },
    ],
  },
  {
    id: "knock",
    label: "knock",
    notes: [
      { at: 0, freq: 180, to: 90, dur: 0.1, wave: "sine", level: 1.3 },
      { at: 0.13, freq: 180, to: 90, dur: 0.1, wave: "sine", level: 1.3 },
    ],
  },
  {
    id: "uwu",
    label: "uwu",
    notes: [
      { at: 0, freq: hz(9), dur: 0.11, wave: "square", level: 0.5 },
      { at: 0.1, freq: hz(16), dur: 0.11, wave: "square", level: 0.5 },
      { at: 0.2, freq: hz(21), dur: 0.26, wave: "square", level: 0.5 },
    ],
  },
];

/** Longer and more insistent — a call is asking you a question. */
export const CALL_SOUNDS: Sound[] = [
  {
    id: "ring",
    label: "ring",
    notes: ring(),
  },
  {
    id: "arcade",
    label: "arcade",
    notes: [0, 0.7].flatMap((offset) =>
      [9, 16, 21, 28].map((step, i) => ({
        at: offset + i * 0.09,
        freq: hz(step),
        dur: 0.1,
        wave: "square" as Wave,
        level: 0.55,
      })),
    ),
  },
  {
    id: "bell",
    label: "bell",
    notes: [0, 0.55, 1.1].flatMap((at) => [
      { at, freq: hz(12), dur: 0.7, wave: "triangle" as Wave },
      { at: at + 0.01, freq: hz(24), dur: 0.5, wave: "sine" as Wave, level: 0.45 },
    ]),
  },
  {
    id: "chirp",
    label: "chirp",
    notes: [0, 0.28, 0.56, 0.84].map((at) => ({
      at,
      freq: 700,
      to: 1500,
      dur: 0.12,
      wave: "sine" as Wave,
      level: 0.9,
    })),
  },
];

/** The two-tone warble of a desk phone: 440 and 480 Hz together, twice. */
function ring(): Note[] {
  return [0, 0.9].flatMap((at) => [
    { at, freq: 440, dur: 0.55, wave: "sine" as Wave, level: 0.8 },
    { at, freq: 480, dur: 0.55, wave: "sine" as Wave, level: 0.8 },
  ]);
}

export const NONE = "none";

function find(list: Sound[], id: string): Sound | null {
  return list.find((sound) => sound.id === id) ?? null;
}

// One context for the whole app. Browsers cap how many you may create, and
// making one per chirp would leak them.
let context: AudioContext | null = null;

function audio(): AudioContext | null {
  try {
    context ??= new AudioContext();
    // A context created before the first user gesture starts suspended; the
    // first sound after the user has clicked anything will resume it.
    if (context.state === "suspended") void context.resume();
    return context;
  } catch {
    // No Web Audio (or no output device at all) — notifications still show,
    // they're just silent.
    return null;
  }
}

function playNotes(notes: Note[], volume: number): OscillatorNode[] {
  const ctx = audio();
  if (!ctx || volume <= 0) return [];

  const start = ctx.currentTime + 0.01;
  const started: OscillatorNode[] = [];

  for (const note of notes) {
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = note.wave ?? "sine";

    const at = start + note.at;
    const end = at + note.dur;

    osc.frequency.setValueAtTime(note.freq, at);
    if (note.to !== undefined) {
      // Exponential rather than linear: pitch is heard logarithmically, so a
      // linear sweep sounds like it slows down as it falls.
      osc.frequency.exponentialRampToValueAtTime(Math.max(note.to, 1), end);
    }

    // A short attack instead of an instant one keeps the tone from clicking,
    // and the exponential tail is what makes it read as a chime rather than a
    // beep that was cut off.
    const peak = Math.max(0.0001, 0.22 * volume * (note.level ?? 1));
    gain.gain.setValueAtTime(0.0001, at);
    gain.gain.exponentialRampToValueAtTime(peak, at + 0.008);
    if (note.sustain) gain.gain.setValueAtTime(peak, end - 0.04);
    gain.gain.exponentialRampToValueAtTime(0.0001, end);

    osc.connect(gain).connect(ctx.destination);
    osc.start(at);
    osc.stop(end + 0.02);
    started.push(osc);
  }

  return started;
}

/**
 * Play one of the built-in sounds by id.
 *
 * Unknown ids and `"none"` are silence rather than an error: a settings file
 * from a build that had a sound this one doesn't shouldn't throw on every
 * message.
 */
export function play(list: Sound[], id: string, volume: number): void {
  if (id === NONE) return;
  const sound = find(list, id);
  if (sound) playNotes(sound.notes, volume);
}

export const playMessageSound = (id: string, volume: number) =>
  play(MESSAGE_SOUNDS, id, volume);

export const playCallSound = (id: string, volume: number) =>
  play(CALL_SOUNDS, id, volume);

/** When the last note ends, so a repeat can be timed off the sound itself. */
function length(notes: Note[]): number {
  return notes.reduce((end, note) => Math.max(end, note.at + note.dur), 0);
}

/** Silence between one ring and the next. A phone breathes between rings. */
const RING_GAP = 1.6;

/**
 * What a caller hears while the other end rings: 440 and 480 Hz together for
 * two seconds, then four of silence, which is the ringback North American
 * phones use. It's held steady rather than decaying like the rings above, and
 * quieter: this is "it's ringing over there", not a phone ringing here.
 */
const RINGBACK: Note[] = [440, 480].map((freq) => ({
  at: 0,
  freq,
  dur: 2,
  wave: "sine" as Wave,
  level: 0.35,
  sustain: true,
}));

const RINGBACK_GAP = 4;

/**
 * Ring until the returned function is called.
 *
 * Repeats are scheduled one burst at a time rather than queued up front, so
 * stopping is immediate: the oscillators still sounding are stopped by hand,
 * because an answered call has to go quiet the moment it is answered, not at
 * the end of whichever note was already playing.
 *
 * Returns a no-op stopper when there is nothing to play — `"none"`, an unknown
 * id, or a build with no Web Audio — so callers never have to special-case it.
 */
export function startRinging(id: string, volume: number): () => void {
  const sound = id === NONE ? null : find(CALL_SOUNDS, id);
  if (!sound) return () => {};
  return repeat(sound.notes, volume, RING_GAP);
}

/**
 * Ringback until the returned function is called.
 *
 * Not one of the user's call sounds, and played even when those are set to
 * none: it isn't a notification, it's how a caller knows the call went out.
 */
export function startRingback(volume: number): () => void {
  return repeat(RINGBACK, volume, RINGBACK_GAP);
}

function repeat(notes: Note[], volume: number, gap: number): () => void {
  if (volume <= 0) return () => {};

  let ringing: OscillatorNode[] = [];

  const once = () => {
    ringing = playNotes(notes, volume);
  };

  once();
  const timer = window.setInterval(once, (length(notes) + gap) * 1000);

  return () => {
    window.clearInterval(timer);
    for (const osc of ringing) {
      try {
        osc.stop();
      } catch {
        // Already stopped, or the context went away underneath us.
      }
    }
    ringing = [];
  };
}
