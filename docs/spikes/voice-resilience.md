# Voice engine: reconnects and bad networks

Phase 2's exit bar asks for calls that survive a network switch and stay
usable under 5% loss and 80 ms of jitter. This is where the engine stands
against both, measured with `crates/horo-rtc/tests/engine.rs` against the
local `livekit-server --dev` (v1.9.1) on macOS.

## Reconnecting

What sets it off, and what happens:

| Trigger | Noticed after | Then |
|---|---|---|
| Signalling drops | at once | resume |
| Nothing received on a peer connection for 4 s (`PeerEvent::Path`) | 4 s | resume |
| The default route changes (checked every 2 s, no packets sent) | ≤ 2 s | resume |
| The app says the network changed (`Call::reconnect`) | at once | resume |
| The server asks (`Leave` with `RESUME` / `RECONNECT`) | at once | resume / rejoin |
| A peer connection dies, or the server won't resume | at once | rejoin |

**Resume** keeps the session: a new signal connection with `reconnect=1`
and the participant's sid, then both peers gather afresh (new sockets, new
TURN allocations, old candidates invalidated) and restart ICE; DTLS and
SRTP carry on. Nobody else sees us leave. Two resume attempts, ten seconds
each, then **rejoin**: a fresh join as a new participant, the microphone
republished, with a new token from `CallOptions::refresh_token` when there
is one (the CLI gets it through MatrixRTC). Ninety seconds without success
ends the call.

ICE's own verdict on a dead path came too late to use: with the media cut
and signalling up, str0m never reported the connection down, and the server
timed the participant out after 19 s. Hence the 4 s silence check.

Measured, Bob listening to Alice's tone:

| Test | Back after | Bob heard |
|---|---|---|
| Alice's network cut (signalling and media) | 1.26 s, resumed | almost no gap (Opus concealment covers it) |
| Bob's network cut | 1.24 s, resumed | ~0.6 s of silence |
| Alice's media cut, signalling up | noticed at 3.6 s, back 1.2 s later, resumed | ~3.9 s of silence |
| Network-change hint while healthy | 1.26 s, resumed | no gap |
| Alice's publisher connection killed | 1.26 s, rejoined | Bob saw her leave and return 40 ms apart |

## 5% loss and 80 ms of jitter

Peers can simulate a bad network on what they receive (`Impairment`: loss,
a fixed delay, jitter that reorders; seeded, in-process, any OS). The test
puts both ends behind 5% loss and 20–100 ms of delay.

The first run found str0m's audio reordering in the way: by default it holds
audio back up to 15 packets, or a second, waiting for a lost packet, then
releases a burst. The jitter buffer took each stall for a late network and
each burst for too much delay: the tone at its level 60% of the time, 363
frames invented, 357 dropped, 11 rebuffers. horo's jitter buffer orders by
RTP time itself, so str0m's reordering is now off:

| | Before | After |
|---|---|---|
| Tone at its level (after 3 s) | 60.4% | 97.8% |
| Rebuilt by FEC / concealed | 67 / 2 | 80 / 8 |
| Stretched / dropped / rebuffers | 363 / 357 / 11 | 5 / 1 / 0 |
| Jitter buffer target | 126 ms | 74 ms |
| Reconnects | 0 | 0 |

The same for half an hour (`HORO_SOAK_SECONDS=1800`): the tone at its level
98.3% of the time, no reconnects and no rebuffers; of 85,438 packets
received, FEC rebuilt 4,180 lost frames and concealment filled 335; 63
frames stretched, 58 dropped, 16 late; the target held at 79 ms.

## Still open

- Windows and Linux: CI now builds the voice crates and runs their unit
  tests on Windows and macOS, and everything (servers included) on Linux;
  nobody has made a call on Windows or Linux yet.
- A four-person call with Element Call participants, and with call E2EE
  (Element Call's default) still not implemented, the room has to have it
  off.
- Time-stretching in the jitter buffer (it drops or invents whole frames).
- Reconnect while TURN-only: covered by the same code, not yet tested
  through the TURN server.
