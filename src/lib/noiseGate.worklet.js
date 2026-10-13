/**
 * The noise gate itself, running on the audio thread.
 *
 * An AudioWorklet rather than a timer on the main thread: a voice app spends
 * most of its life minimised, and a hidden WebView throttles timers to about
 * once a second — a gate that opens a second late cuts off the start of every
 * sentence. The audio thread isn't throttled.
 *
 * Plain JavaScript because it's loaded by URL into its own global scope, which
 * TypeScript's DOM lib doesn't describe; see `noiseGate.ts` for the side that
 * talks to it.
 */

// How long the gate stays open after the level drops below the threshold, so
// the gaps between words don't chop a sentence into pieces.
const HOLD_SECONDS = 0.25;
// Opening is near-instant so the first syllable gets through; closing fades so
// it doesn't click.
const ATTACK_SECONDS = 0.003;
const RELEASE_SECONDS = 0.08;
// How often the main thread hears about the level. Fast enough for a meter
// that looks live, slow enough not to flood the message port.
const REPORT_SECONDS = 1 / 30;
// Below this, report silence rather than an ever-larger negative number.
const FLOOR_DB = -100;

class NoiseGate extends AudioWorkletProcessor {
  constructor() {
    super();
    /** dBFS, or `null` for a gate that never closes. */
    this.threshold = null;
    this.open = true;
    this.gain = 1;
    this.holdLeft = 0;
    this.peak = FLOOR_DB;
    this.sinceReport = 0;

    this.attack = 1 - Math.exp(-1 / (ATTACK_SECONDS * sampleRate));
    this.release = 1 - Math.exp(-1 / (RELEASE_SECONDS * sampleRate));

    this.port.onmessage = (event) => {
      if ("threshold" in event.data) this.threshold = event.data.threshold;
    };
  }

  process(inputs, outputs) {
    const input = inputs[0];
    const output = outputs[0];
    const frames = output[0]?.length ?? 128;

    // No input yet (or the track ended): pass silence, but keep the node alive.
    if (!input || input.length === 0) {
      for (const channel of output) channel.fill(0);
      return true;
    }

    // One level for the whole block, taken from the first channel — a
    // microphone is mono in every case that matters here.
    const samples = input[0];
    let sum = 0;
    for (let i = 0; i < samples.length; i++) sum += samples[i] * samples[i];
    const rms = Math.sqrt(sum / samples.length);
    const db = rms > 0 ? Math.max(FLOOR_DB, 20 * Math.log10(rms)) : FLOOR_DB;

    if (this.threshold === null || db >= this.threshold) {
      this.open = true;
      this.holdLeft = HOLD_SECONDS * sampleRate;
    } else if (this.holdLeft > 0) {
      this.holdLeft -= frames;
    } else {
      this.open = false;
    }

    const target = this.open ? 1 : 0;
    const rate = this.open ? this.attack : this.release;
    for (let i = 0; i < frames; i++) {
      this.gain += (target - this.gain) * rate;
      for (let c = 0; c < output.length; c++) {
        const source = input[c] ?? input[0];
        output[c][i] = source[i] * this.gain;
      }
    }

    this.peak = Math.max(this.peak, db);
    this.sinceReport += frames;
    if (this.sinceReport >= REPORT_SECONDS * sampleRate) {
      this.port.postMessage({ level: this.peak, open: this.open });
      this.peak = FLOOR_DB;
      this.sinceReport = 0;
    }

    return true;
  }
}

registerProcessor("horo-noise-gate", NoiseGate);
