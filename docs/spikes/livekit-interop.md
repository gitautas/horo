# Spike: horo's call engine against livekit-client

Every call before this had horo on both ends. Element Call (and horo's own
WebView build) publish with livekit-client, which has its own defaults, so
this puts the two in one room: horo-call's engine sending a 440 Hz tone,
livekit-client 2.21.0 in a browser sending 660 Hz, each measuring what it
hears. Local `livekit-server --dev` v1.9.1, macOS, the browser in Claude's
built-in browser pane (Chromium).

```sh
scripts/test-livekit/start.sh
scripts/interop/run.sh 60      # serves the page, prints its URL, runs horo
```

## What livekit-client sends

- Opus, `usedtx=1;useinbandfec=1;stereo=1;maxaveragebitrate=510000`: DTX on
  (its default for microphone tracks), and flagged stereo (a mono decoder
  takes it as is).
- No RED: the sender's codec stayed `audio/opus`.
- In 8 s of silence it sent 52 packets, against ~400 for 8 s of tone:
  DTX's 1–2 byte packets.
- Muting stops the server forwarding the track altogether.

## Results

| | Heard | Notes |
|---|---|---|
| browser ← horo | 439 Hz at −15.1 dBFS (sent: −15.05) | 0 lost, 0 concealed, browser jitter buffer ~30 ms |
| horo ← browser, steady tone | 660 Hz at −15.0 dBFS, purity ~100–135 dB | target ~28 ms, nothing concealed, late or dropped |
| horo ← browser, muted then unmuted | silence, then the tone | the track goes idle after 1 s; unmuting starts a fresh buffer cleanly |
| horo ← browser, DTX silence (before the fix) | silence, then ~1 s of damaged tone | 191 frames stretched, 18 rebuffers, 7 dropped and 9 concealed on resuming |
| horo ← browser, DTX silence (after the fix) | silence, then the tone at once | 478 DTX frames, 0 stretched, 0 rebuffered, 0 dropped |

## The DTX fix

horo's jitter buffer took the gaps between DTX packets for a network that
had stalled: it held the playhead to wait, gave up after 200 ms and
rebuffered, then had to trim the delay it had built up once speech came
back. Every pause in an Element Call participant's speech would have cost a
glitch on resuming.

Now a packet of 2 bytes or less (Opus with no audio in it: DTX) puts the
buffer in a DTX state, where a missing frame is played as Opus comfort noise
at its proper time (`Played::Silent`), until a real packet arrives or a
second passes without any. `jitter.rs` has a test with WebRTC's pattern (a
1-byte packet every 400 ms).

## Still open

- horo doesn't send DTX itself (the encoder has it off): fine for interop,
  a bandwidth saving to have later.
- RED: livekit-client didn't use it here. If an SFU or client sends
  `audio/red`, str0m's answer has to leave it out (it does today, offering
  only Opus) or horo has to unwrap it.
- Comfort noise is silence here because the browser's input was digital
  silence; with a real microphone it's Opus's noise estimate.
- Call E2EE (Element Call's default) is still not implemented, so this is the
  unencrypted case only.
