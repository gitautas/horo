# horo → gpui + native RTC: rewrite plan

Status: draft, 2026-11-18. Baseline: v0.11.0 (`9ebf79b`).

## 1. Why, and what "done" means

Today horo is Tauri 2 + React (~23k lines TS/TSX) over a Rust Matrix backend
(~12k lines), with calls done by `livekit-client` inside the WebView. That
stack has three structural costs:

- **Calls depend on the WebView's WebRTC.** Linux distribution WebKitGTK ships
  with WebRTC compiled out, so voice cannot work there at all
  (`src-tauri/src/webrtc.rs` exists only to diagnose this). Mobile WebViews add
  their own permission and audio-session quirks.
- **Everything crosses an IPC bridge.** 73 `#[tauri::command]`s, `ts-rs`
  bindings, `events.rs` re-emitting SDK streams as JSON, a `horo://` scheme
  for media, and a second state store (zustand) mirroring Rust state.
- **Two languages, two toolchains, two test runners** for one app.

Done means: one Rust workspace, a gpui UI, and a native media engine; voice,
video and screen share working on macOS, Windows, Linux, iOS and Android;
feature parity with v0.11.0; interop with Element X / Element Call over
MatrixRTC unchanged; signed self-updating desktop builds as today.

Non-goals for the rewrite: new Matrix features, a visual redesign (port the
current look, including the fonts and Phosphor icons), and a web build.

## 2. Decisions to make up front

| # | Decision | Recommendation | Why |
|---|----------|----------------|-----|
| D1 | Mobile UI | gpui on desktop now; mobile via a `gpui-mobile` fork, gated by a spike (Phase 0) | Zed's gpui is desktop-only. Mobile exists only as community forks (wgpu on Metal/Vulkan) of varying maturity. This is the single largest risk in the plan. |
| D2 | Widget kit | `gpui-component` (longbridge) for inputs, lists, popovers, dialogs, then own components on top | Writing a text input with IME, selection and undo from scratch is months of work. |
| D3 | RTC transport | `str0m` (sans-I/O WebRTC) behind our own `MediaEngine` trait | Pure Rust, no libwebrtc, no threads of its own, builds for every target. The trait leaves the LiveKit Rust SDK (libwebrtc, what Zed uses) as a fallback if str0m blocks us. |
| D4 | SFU | Stay on LiveKit, implement its signal protocol ourselves | Required for Element Call interop; the protocol is public protobuf over WSS (`livekit-protocol` crate has the types). |
| D5 | Echo cancellation | `sonora` (WebRTC's AEC3, NS and AGC2 ported to Rust) on every platform | One canceller, any devices, no libwebrtc to build. Measured against macOS VoiceProcessingIO in Phase 2: both bring a loudspeaker chirp's echo down to the room's noise floor, so the platform path was dropped. |
| D6 | Video codecs | VP8 baseline (libvpx) + H.264 via hardware encoders where available (VideoToolbox, MediaCodec, Media Foundation) | VP8 is what LiveKit and Element Call negotiate by default; hardware H.264 saves battery on phones. |
| D7 | Packaging/updates | `cargo-packager` + its updater | Replaces the Tauri bundler and `tauri-plugin-updater` with the same artefacts (dmg/.app.tar.gz, NSIS/MSI, AppImage/deb) and a signed update manifest. Existing installs need one last Tauri release that points at the new feed. |
| D8 | Migration strategy | Big-bang UI rewrite on a branch; backend extracted first and kept shippable in Tauri meanwhile | The Matrix layer is reused, so the old app can keep shipping off the same core crate while the gpui shell catches up. |

## 3. Target architecture

```
horo/                     (cargo workspace)
├─ crates/
│  ├─ horo-core/          Matrix: auth, session, rooms, timeline, spaces,
│  │                      presence, packs, media cache, verification, directory.
│  │                      Moved from src-tauri/src/matrix + verification.rs.
│  │                      No UI, no tauri. Exposes typed async APIs + eyeball
│  │                      observables instead of commands + JSON events.
│  ├─ horo-rtc/           Calls, one crate in modules: `matrixrtc` (membership/
│  │                      notification from rtc.rs, OpenID → JWT exchange),
│  │                      `livekit` (signal client: WSS + protobuf, join,
│  │                      publish, subscribe, simulcast layers, reconnect/
│  │                      resume), and behind the `media` feature: `peer`
│  │                      (str0m peer connections, ICE), `turn`, `audio`
│  │                      (jitter buffer, Opus, AEC/NS, noise gate, level
│  │                      meter), `devices` (per-platform capture/playback:
│  │                      audio I/O now; camera, screen capture, audio
│  │                      session/routing, permissions later), `call` (the
│  │                      engine), later RTP pacing, VP8/H.264, stats. The
│  │                      `horo-call` CLI is its binary. Started as six
│  │                      crates (horo-rtc, -livekit, -media, -devices, -call);
│  │                      merged once they settled.
│  ├─ horo-platform/      Notifications, keychain, file dialogs, opener,
│  │                      haptics, updater, deep links, background/lifecycle.
│  └─ horo-ui/            gpui views, theme, components, assets, i18n hooks.
├─ apps/
│  ├─ desktop/            gpui main, packaging config
│  ├─ ios/                Xcode host project, Info.plist, entitlements
│  └─ android/            Gradle host project, NativeActivity/GameActivity
└─ xtask/                 build, bundle, release, bindings-free now
```

**Data flow.** `horo-core` owns a tokio runtime. Its observables
(`eyeball_im::VectorDiff` from `matrix-sdk-ui` timelines and room lists) are
forwarded into gpui `Entity`s on the foreground executor; views `observe` those
entities. No serialisation, no DTO layer: `dto.rs` (1k lines) and
`src/lib/types.ts` / `ipc.ts` disappear, and `horo://` media becomes an
in-process image cache that hands gpui `RenderImage`s.

**Media threading.** str0m is driven by one dedicated thread per call
(sockets + timers), audio by the device's real-time callback, video encode and
decode on their own worker threads. Video frames reach the UI as
`CVPixelBuffer` via gpui's `surface` element on Apple platforms and as uploaded
textures elsewhere.

## 4. The custom RTC stack in detail

### 4.1 Signalling (`horo_rtc::matrixrtc`, `horo_rtc::livekit`)

Keep what `rtc.rs` does today, unchanged in behaviour:
`org.matrix.msc3401.call.member` state with refresh, OpenID token →
LiveKit JWT service, `m.rtc.notification` when first in, ring timeout, leave.

New: a LiveKit client written against the signal protocol.

- `JoinRequest` over `wss://…/rtc?access_token=…`, protocol version pinned.
- Two transports as LiveKit expects (publisher + subscriber), each a str0m
  `Rtc`; negotiate offers/answers and trickle ICE over the signal socket.
  Evaluate LiveKit's single-PC mode once the two-PC path works.
- Track publish/unpublish, mute, `UpdateSubscription`, `UpdateTrackSettings`
  (requested resolution per visible tile), simulcast layer selection,
  speaker updates, connection quality, `Leave`, server-initiated migrate.
- Reconnect: signal resume first, full rejoin on failure, with the current
  `describeDisconnect` reasons carried over to the UI.
- Test against a local `livekit-server --dev` in CI, plus an Element Call
  interop checklist run by hand before each release.

### 4.2 Transport (`horo_rtc::peer`, `horo_rtc::turn`)

- str0m for ICE-lite-against-SFU, DTLS-SRTP, RTCP, SCTP data channel (LiveKit
  uses data channels for reliable/lossy data packets).
- **TURN.** str0m does not include a TURN client, and LiveKit hands out TURN
  (UDP and TLS on 443) for restrictive networks. Write a small TURN client
  (allocate, permissions, channel bind, refresh) and relay through it. TURN/TLS
  is the path that gets through corporate firewalls; treat it as required.
- Congestion control: str0m's TWCC + bandwidth estimate drives encoder bitrate
  and simulcast layer publishing.
- Crypto backend: confirm str0m's backend works on all five targets (it
  previously required OpenSSL; check current pluggable backends — aws-lc-rs
  is already in our graph via rustls).

### 4.3 Audio

```
mic → device callback → resample 48k → [AEC/NS/AGC] → noise gate → Opus enc → RTP
RTP → jitter buffer → Opus dec (+PLC/FEC) → mix N speakers → resample → speaker
                                              └→ AEC far-end reference
```

- Device I/O: `cpal` on desktop and Android (AAudio), CoreAudio directly on
  iOS so we can own `AVAudioSession` (category `playAndRecord`, mode
  `voiceChat`, Bluetooth/route changes, interruptions from phone calls).
- Voice processing: `sonora` everywhere, on the 48 kHz frames between the
  devices and the engine (`horo_rtc::audio::echo`). On iOS the session's
  mode must not be `voiceChat`, which would stack Apple's processing on top.
- Jitter buffer: adaptive, per remote track, with Opus in-band FEC and PLC.
  This is the piece that decides call quality; budget time for it and for
  tuning under `tc netem` loss/jitter.
- Port `noiseGate.ts` + its worklet to Rust (it is small) and feed the same
  meter to the mic settings screen.
- Active-speaker: use LiveKit `SpeakersChanged`, local VAD for our own ring.
- Output device selection and per-participant volume.

### 4.4 Video

- Camera: AVFoundation (macOS/iOS), Media Foundation (Windows), V4L2 or the
  PipeWire camera portal (Linux), Camera2 via JNI (Android). Output NV12/I420.
- Screen share: ScreenCaptureKit (macOS; gpui already wraps capture sources),
  Windows.Graphics.Capture, PipeWire + xdg-desktop-portal ScreenCast (Linux,
  Wayland and X11), MediaProjection (Android). iOS screen share needs a
  ReplayKit broadcast upload extension with frames passed over an App Group —
  schedule it last and treat as optional.
- Encode: VP8 via libvpx everywhere; H.264 via hardware encoders where present,
  openh264 fallback. Simulcast: 3 layers for camera, 1–2 for screen.
- Decode: matching decoders; drop to keyframe requests (PLI/FIR) on loss.
- Render: Apple `surface(CVPixelBuffer)`; elsewhere convert to BGRA on a worker
  and swap into a gpui image each frame. Measure this early (Phase 0); if
  per-frame image upload is too slow, add a texture-update path to gpui.

### 4.5 Call E2EE (optional, after parity)

v0.11.0 does not encrypt call media. Element Call's per-participant keys
(MSC4143 encryption keys over to-device) + LiveKit frame encryption
(AES-GCM over the encoded frame) slot in between encoder and packetiser. Plan
the frame pipeline with an insertion point for it; ship it after parity.

### 4.6 Fallback

If str0m is blocked on something we cannot fix in reasonable time (TURN, a
LiveKit negotiation detail, a platform), implement `MediaEngine` with the
LiveKit Rust SDK instead. Cost: a large prebuilt libwebrtc per target and its
own audio device module, but it is proven on all five platforms. Decide at the
end of Phase 2 at the latest.

## 5. UI port inventory

Every React surface, mapped to a gpui view. Rough size is lines of TSX today.

| Area | Today | Notes for gpui |
|------|-------|----------------|
| Shell & layout | `App.tsx` 619, `viewport.ts`, `ResizeHandle` | Desktop three-pane + room info; phone stack navigation below 768pt. Window drag regions native. |
| Login | `LoginScreen` 786 | Password, SSO (system browser + loopback/deep link), QR login. |
| Spaces | `SpacesRail` 478, `SpaceDialogs` 432, `ExploreSpaces` 568 | Discord-style servers; drag to reorder. |
| Room list | `RoomList` 964, `RoomDialogs`, `CreateRoom`, `InvitePane` | Virtualised list from the sliding-sync room list stream. |
| Timeline | `TimelineView` 592, `MessageRow` 1216, `ReadReceipts` | Variable-height virtual list anchored at the bottom, back-pagination, reactions, edits, replies, receipts. Hardest view after the composer. |
| Rich text | `richText.tsx` 480, `linkify`, `cutout.ts` | Matrix HTML subset → gpui `StyledText`/`InteractiveText`; code blocks in JetBrains Mono; sticker/cut-out detection moves to Rust. |
| Composer | `Composer` 716, `EmojiPicker` 819, `AttachSheet`, `DropZone` | Multi-line input with IME, mentions, `:emoji:` and pack autocomplete, paste/drop images. Depends on D2. |
| Media | `Lightbox`, `blobMedia.ts`, `photos.rs` | Image/video viewer, zoom, save; animated GIF/WebP decode. |
| Profiles & presence | `ProfileCard` 449, `ProfileHeader`, `Presence` | |
| Room settings & info | `RoomSettings`, `RoomInfo` 497 | |
| Packs | `PackSettings` 648 | |
| Verification | `VerificationModal` 398 | Emoji SAS, QR. |
| Settings | `SettingsView` 1489, `MicSettings`, `CameraBackdrop`, `settingsUi` | Mic meter and camera preview now fed by `horo_rtc::devices`. |
| Calls | `CallStage` 561, `CallBar`, `VoicePanel`, `IncomingCallModal`, `call.ts` 655 | Tile grid with speaking rings, screen-share focus, device switching. |
| System | `UpdateNotice`, `notify.ts`, `sounds.ts`, `ContextMenu`, `ErrorBoundary` | Native notifications, sound playback through the same audio output, native context menus where the platform has them. |

Cross-cutting: theme tokens from `styles/app.css`, bundled fonts (Nunito,
Baloo 2, Unbounded, JetBrains Mono), Phosphor icons as SVG assets, emoji
data from `unicode-emoji-json` converted at build time, accessibility (gpui's
a11y support is limited — record gaps), keyboard shortcuts as gpui actions.

Tests: the vitest suites (composer, emoji picker, packs, notify, rich text,
receipts, presence) become `gpui::TestAppContext` tests against the same cases.

## 6. Platform services

| Service | Today | Replacement |
|---------|-------|-------------|
| Secrets | `keyring` + iOS data-protection store | Unchanged (already Rust). |
| Notifications | `tauri-plugin-notification` | `UNUserNotificationCenter` (Apple), WinRT toasts, `notify-rust`/portal (Linux), `NotificationManager` (Android). Push stays out of scope as today. |
| File dialogs | `tauri-plugin-dialog` | `rfd` on desktop; `UIDocumentPicker`/`PHPicker`, Android SAF intents on mobile. |
| Open links | `tauri-plugin-opener` | gpui `open_url` on desktop; platform intents on mobile. |
| Haptics | `tauri-plugin-haptics` | `UIImpactFeedbackGenerator`, Android `Vibrator`. |
| Updates | `tauri-plugin-updater`/`process` | `cargo-packager-updater`; mobile keeps the "new version available" notice from `update.rs`. |
| Permissions | WebView prompts | Explicit mic/camera/screen permission flows per platform, with the Info.plist / manifest strings. |
| Lifecycle | Tauri | Background/foreground hooks: pause sync on iOS background, keep audio session alive during a call (iOS `audio` + `voip` background modes; Android foreground service of type `microphone`/`camera`). |
| CallKit / ConnectionService | none | Optional later: native incoming-call UI on phones. |

## 7. Phases

Estimates assume one developer working mostly full-time with Claude; they are
for sequencing, not commitments.

### Phase 0 — Spikes (2–3 weeks) — go/no-go

Throwaway code; answer the questions that could sink the plan.

1. **gpui on phones.** Pick a `gpui-mobile` fork, run a scrolling list + text
   input on a real iPhone and Android device. Check: IME and soft keyboard,
   safe areas, touch scrolling with momentum, lifecycle, frame rate. Exit:
   all usable, or a list of gaps small enough to fix in our fork.
2. **gpui text input.** `gpui-component` input with IME (Japanese/Chinese),
   paste, undo, multi-line growth, inline autocomplete popover.
3. **Video into gpui.** 720p30 decoded frames rendered on each desktop OS at
   <10% of a core for the upload path.
4. **str0m ↔ LiveKit.** Join a `livekit-server --dev` room, publish an Opus
   tone, receive another participant's audio. Then the same via TURN/TLS
   only.
5. **Audio on iOS.** VPIO + `AVAudioSession` from Rust, round-trip echo test
   on speakerphone.

Go/no-go: if (1) fails, keep mobile on the Tauri shell over `horo-core` and
ship gpui on desktop only, revisiting mobile later. If (4) fails in a way we
can't fix, take the §4.6 fallback.

### Phase 1 — Extract `horo-core` (2 weeks), shippable in the old app

- Create the workspace; move `src-tauri/src/matrix/*`, `verification.rs`,
  `photos.rs` media logic and `rtc.rs` membership code into crates. The Matrix
  layer is already nearly Tauri-free (`auth.rs` is the main exception).
- Replace JSON event emission with typed observables; keep a thin Tauri
  adapter (`commands.rs`, `events.rs`, `dto.rs`) on top so v0.12 still ships
  from the existing React app.
- Unit tests for core against a local Synapse/Conduit in CI.

### Phase 2 — Desktop voice engine (5–6 weeks)

- `horo_rtc::livekit` signal client, `horo_rtc::audio` path, TURN client,
  jitter buffer, echo cancellation (sonora), noise gate, device selection.
- A headless CLI (`horo-call`) that joins a MatrixRTC call from the terminal:
  the test harness for everything after this.
- Exit: 30-minute 4-person call with Element Call participants on macOS,
  Windows and Linux, including under 5% loss / 80ms jitter; reconnect after a
  network switch.
- Optional: wire `horo-call` into the Tauri app on Linux to finally fix voice
  there before the UI rewrite lands.

### Phase 3 — gpui desktop shell (6–8 weeks)

Order: theme/assets → shell & navigation → login & session restore → room list
→ timeline (read-only) → composer → spaces → media/lightbox → settings →
profiles/presence → verification → packs → notifications/sounds → updater.
Exit: daily-drivable on macOS with no React running.

### Phase 4 — Calls UI + video (5–6 weeks)

- Voice UI first (CallBar, VoicePanel, incoming call, ringing).
- Camera capture/encode/decode per desktop OS, simulcast, tile grid,
  subscription quality by tile size.
- Screen share on macOS, Windows, Linux (Wayland via portal).
- Exit: parity with v0.11.0 calls on all three desktop OSes; Linux voice works.

### Phase 5 — Mobile (6–10 weeks, depends on Phase 0)

- Host projects for iOS and Android, gpui-mobile integration, phone layout.
- Audio session, background modes, foreground service, permissions.
- Camera on both; screen share on Android; iOS broadcast extension last.
- Notifications, file pickers, haptics, deep links for SSO.
- Exit: parity with the current mobile builds on a real iPhone and Android
  phone.

### Phase 6 — Packaging, migration, release (2 weeks)

- `cargo-packager` configs; rewrite `release.yml`/`ci.yml` (macOS universal or
  two arches, Windows, Linux AppImage+deb, Android apk, iOS ipa).
- Data migration: same SQLite store path and keychain entries, so users stay
  logged in and keep their keys. Test upgrade from v0.11 on each platform.
- Updater handover: last Tauri release (v0.x) whose update manifest points to
  the first gpui release, signed with the existing key or carrying the new
  public key.
- Remove `src/`, `src-tauri/`, `node_modules`, npm scripts.

**Total: roughly 28–39 weeks** of focused work, with mobile and video as the
widest error bars.

## 8. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| gpui mobile forks immature (input, keyboard, a11y, lifecycle) | High | High | Phase 0 gate; Tauri-mobile fallback over `horo-core`; upstream fixes to the fork we pick. |
| gpui API churn (it is pre-1.0 and versioned with Zed) | High | Medium | Pin a git rev; bump deliberately once a month. |
| Call quality worse than libwebrtc (jitter buffer, BWE, AEC) | Medium | High | Netem test matrix from Phase 2; APM for processing; LiveKit SDK fallback behind `MediaEngine`. |
| LiveKit protocol changes | Low | Medium | Pin protocol version; CI against pinned and latest `livekit-server`. |
| Accessibility regressions vs WebView | High | Medium | Track gaps explicitly; contribute to gpui a11y where cheap. |
| C dependencies (libopus, libvpx, APM) complicate cross-compiles | Medium | Medium | Vendor and build via `cc`/`cmake` in xtask; cache in CI; Android NDK + iOS toolchains already in CI. |
| iOS signing (free personal team) | Certain | Medium | Unchanged from today: the iOS release job needs a paid team. |
| Scope creep during a long rewrite | Medium | High | Feature freeze on the old app except fixes; parity checklist from §5 is the definition of done. |

## 9. First concrete steps

1. Decide D1–D8 (or amend them).
2. Create `crates/horo-core` and move `matrix/` into it behind the Tauri
   adapter (Phase 1, low risk, valuable even if the rewrite stops).
3. Run the five Phase 0 spikes in parallel on throwaway branches.
