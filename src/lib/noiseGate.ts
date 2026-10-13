/**
 * A noise gate for the microphone, and the level meter that comes with it.
 *
 * The same gate serves two places: the call, where it sits between the
 * microphone and the SFU as a LiveKit track processor, and the voice settings,
 * where a test capture drives the meter you tune the threshold against. Both
 * report the level they see, so the meter in settings shows exactly what the
 * call would send.
 */

import type { AudioProcessorOptions, Track, TrackProcessor } from "livekit-client";

// `no-inline`: a small asset would otherwise become a `data:` URL, which the
// CSP's `script-src 'self'` refuses to load as a worklet.
import workletUrl from "./noiseGate.worklet.js?url&no-inline";

import { GATE_OFF_DB, type Settings } from "./settings";

/**
 * The quietest level the meter draws; anything below reads as silence. The
 * same as the slider's bottom, so "all the way left" means "never gated".
 */
export const METER_FLOOR_DB = GATE_OFF_DB;

export interface GateReading {
  /** Peak level since the last reading, in dBFS. */
  level: number;
  /** Whether the gate is letting sound through. */
  open: boolean;
}

type ReadingListener = (reading: GateReading) => void;

/** The threshold the gate should use, or `null` when it's wide open. */
export function gateThreshold(settings: Settings): number | null {
  return settings.inputSensitivity > GATE_OFF_DB ? settings.inputSensitivity : null;
}

export function gateSupported(): boolean {
  return (
    typeof AudioContext !== "undefined" &&
    typeof AudioWorkletNode !== "undefined" &&
    "audioWorklet" in AudioContext.prototype
  );
}

/**
 * One microphone track in, one gated track out.
 *
 * Owns its `AudioContext` rather than borrowing LiveKit's: the room only makes
 * one when mixing playback through Web Audio, which we don't, and a processor
 * restart (switching microphones) doesn't hand one over anyway.
 */
class Gate {
  readonly context = new AudioContext();
  private node: AudioWorkletNode | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private destination = this.context.createMediaStreamDestination();
  private threshold: number | null = null;
  private listeners = new Set<ReadingListener>();

  get output(): MediaStreamTrack {
    return this.destination.stream.getAudioTracks()[0];
  }

  async start(track: MediaStreamTrack): Promise<void> {
    if (!this.node) {
      await this.context.audioWorklet.addModule(workletUrl);
      this.node = new AudioWorkletNode(this.context, "horo-noise-gate", {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1],
      });
      this.node.port.onmessage = (event: MessageEvent<GateReading>) => {
        for (const listener of this.listeners) listener(event.data);
      };
      this.node.connect(this.destination);
      this.node.port.postMessage({ threshold: this.threshold });
    }

    // A context created outside a click starts suspended, and a suspended
    // context turns the microphone into silence. Better to find out here and
    // send the raw track than to put someone on the call who can't be heard.
    await this.context.resume();
    if (this.context.state !== "running") {
      throw new Error("the audio context wouldn't start");
    }

    this.source?.disconnect();
    this.source = this.context.createMediaStreamSource(new MediaStream([track]));
    this.source.connect(this.node);
  }

  setThreshold(threshold: number | null) {
    this.threshold = threshold;
    this.node?.port.postMessage({ threshold });
  }

  onReading(listener: ReadingListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  async close(): Promise<void> {
    this.listeners.clear();
    this.source?.disconnect();
    this.node?.disconnect();
    this.output?.stop();
    await this.context.close().catch(() => {});
  }
}

/** The gate as a LiveKit audio processor, for the call's microphone track. */
export class NoiseGateProcessor
  implements TrackProcessor<Track.Kind.Audio, AudioProcessorOptions>
{
  readonly name = "horo-noise-gate";
  private gate = new Gate();
  processedTrack?: MediaStreamTrack;

  /** LiveKit refuses a processor unless the track has a context to offer it. */
  get context(): AudioContext {
    return this.gate.context;
  }

  async init(options: AudioProcessorOptions): Promise<void> {
    await this.gate.start(options.track);
    this.processedTrack = this.gate.output;
  }

  async restart(options: AudioProcessorOptions): Promise<void> {
    await this.gate.start(options.track);
  }

  async destroy(): Promise<void> {
    await this.gate.close();
    this.processedTrack = undefined;
  }

  setThreshold(threshold: number | null) {
    this.gate.setThreshold(threshold);
  }

  onReading(listener: ReadingListener): () => void {
    return this.gate.onReading(listener);
  }
}

/**
 * A microphone capture that goes nowhere but the meter.
 *
 * Captured with the same processing the call asks for, so the level you tune
 * against is the level the gate will see mid-call.
 */
export class MicTest {
  private gate = new Gate();
  private stream: MediaStream | null = null;
  private closed = false;

  static async open(deviceId: string, threshold: number | null): Promise<MicTest> {
    const test = new MicTest();
    test.gate.setThreshold(threshold);
    try {
      test.stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
          ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
        },
      });
      await test.gate.start(test.stream.getAudioTracks()[0]);
    } catch (error) {
      await test.close();
      throw error;
    }
    return test;
  }

  setThreshold(threshold: number | null) {
    this.gate.setThreshold(threshold);
  }

  onReading(listener: ReadingListener): () => void {
    return this.gate.onReading(listener);
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.stream?.getTracks().forEach((track) => track.stop());
    await this.gate.close();
  }
}
