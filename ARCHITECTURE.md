# horo architecture

Rust owns the protocol; the WebView owns the pixels and the media.

```
src-tauri/src/
  lib.rs          tauri builder, command registry, horo:// media protocol
  commands.rs     the whole IPC surface — thin wrappers, no logic
  dto.rs          the wire format (camelCase); src/lib/bindings is generated from it
  error.rs        one error type, serialised as { kind, message }
  events.rs       events pushed to the frontend + the tasks producing them
  matrix/
    core.rs       MatrixCore: client, sync service, open timelines, room mirror
    session.rs    session persistence (OS keychain, file fallback)
    auth.rs       discovery, password/SSO login, restore, sync bootstrap
    rooms.rs      room summaries + the sliding-sync diff stream
    timeline.rs   open/close timelines, send, edit, react, redact, paginate
    media.rs      mxc:// fetching behind the horo:// scheme
    presence.rs   who's online, polled for whoever is on screen
  verification.rs emoji SAS + key recovery
  rtc.rs          MatrixRTC membership + LiveKit token exchange
  update.rs       which update path this build can take, and what's newest

src/
  lib/ipc.ts      typed wrappers over every command and event
  lib/bindings/   the wire types as TypeScript, generated — `npm run bindings`
  lib/types.ts    re-exports them, plus the helpers that read them
  lib/diff.ts     applies VectorDiffs from the backend
  lib/richText.ts renders other people's HTML without trusting it
  lib/blobMedia.ts video/audio over IPC, because a custom scheme can't serve it
  lib/call.ts     LiveKit room, mic, participant state
  lib/noiseGate.ts mic noise gate + level meter, on an AudioWorklet
  lib/presence.ts refcounts who the UI is drawing, so Rust polls no one else
  lib/update.ts   the update state machine, both the in-app and manual paths
  store/          zustand store: a projection of Rust's truth + UI state
  components/     the design, implemented
```

## Why the pieces sit where they do

**matrix-rust-sdk in the backend.** Keys and crypto never enter the WebView. The
SQLite store is encrypted with a passphrase held in the OS keychain.

**matrix-sdk-ui, not raw sync.** `SyncService` + `RoomListService` + `Timeline`
are the layer Element X is built on. They give us sliding sync, and timeline
aggregation — edits collapse into the original, reactions attach to their
target, replies resolve — which is most of "Discord parity" for free.

**Diffs, not snapshots.** The room list and each timeline stream `VectorDiff`s.
The frontend applies them in place, so a busy room doesn't re-render the world.

**Media over a custom scheme, not IPC.** `horo://media/<encoded-mxc>?w=&h=` lets
the WebView cache and lazy-load avatars like any other image. Pushing base64
through IPC would cost megabytes per render. *Except* for video and audio — see
below.

**WebRTC in the WebView.** Rust does the Matrix half of a call (membership state
events, OpenID token → LiveKit JWT); `livekit-client` does the media. The
WebView already has echo cancellation and device handling we'd otherwise have to
rebuild.

## Traps

Every one of these cost real time to find. They are not obvious from the docs.

### The room list carries one event per room, not a timeline

`DEFAULT_LIST_TIMELINE_LIMIT` is 1 — that single event is the *sidebar preview*.
Opening a room must call `RoomListService::subscribe_to_rooms`, which raises the
limit and pulls the required state. Skip it and every room opens showing exactly
one message.

### Scroll-triggered pagination can't bootstrap itself

A room showing one message isn't scrollable, so the scroll handler never fires
and the user has no way to ask for history. `TimelineView` paginates until the
content overflows, then hands over to scrolling. It's gated on the timeline
actually being open — the room is selected before `open_timeline` resolves, and
paginating in that gap raises `NoTimeline`.

### Fresh allocations in zustand selectors loop forever

zustand compares snapshots by identity, so `s.timelines[key] ?? []` returns a new
array on every read and re-renders until React throws error #185. Derived arrays
are memoised in the component (`filterRooms`) and empties are module-level
constants (`NO_ITEMS`).

### Sync starts before the UI mounts

The session is restored — and syncing begins — before React subscribes, so the
backend's first room-list push has no listener. `MatrixCore.rooms` mirrors the
list and `get_rooms` serves a snapshot once the frontend is listening.

### `Device::is_verified()` is true for your own device before you verify it

It means "cross-signed **or** locally trusted", and the SDK locally trusts the
device it's running on. Use `is_verified_with_cross_signing()` for anything the
user reads as a security claim.

### Verifying a device does not, by itself, decrypt anything

Two separate defaults have to be changed before verification does what the user
thinks it does.

`EncryptionSettings::backup_download_strategy` defaults to `Manual`, meaning the
SDK *never* fetches room keys from the server-side backup. A newly verified
device receives the backup's decryption key over `m.secret.send` and then sits on
it. `auto_enable_backups` defaults to false, so on a fresh account there is no
backup to fetch from either. `auth::encryption_settings` sets both.

Even with keys arriving, the timelines already on screen don't necessarily
re-read them: `Timeline` retries decryption for every event when it is *built*,
which is why restarting the app "fixed" it. `verification::unlock_history` does
the same work without the restart — wait for the backup to come up, download the
keys for each open room, then `Timeline::retry_decryption` the sessions still
showing as undecryptable. `recover()` runs it too, for the same reason.

### Space membership is recorded twice and neither half is reliable

The space lists its children with `m.space.child`; a room *may* point back with
`m.space.parent`, but most never do. Filtering on `parentSpaces` alone shows an
empty space. `filterRooms` takes the union of both.

### A room type is not permission to hide the room

MSC1772 says a client should ignore room types it doesn't understand, and taking
that literally hides real rooms: voice rooms carry a type, and there are three
spellings in the wild (`m.video_room`, `io.element.video`,
`org.matrix.msc3417.call`) of which ruma models one, behind a feature flag. A
room the user joined and can see in every other client vanishing from the sidebar
is far worse than an unfamiliar room appearing in it, so `is_utility_room_type`
names the types we hide — ours, and only ours — rather than the ones we keep.

### An update is only ours to install where we own the install

Tauri's updater replaces a bundle in place, so it works exactly where there is a
bundle we put there: a `.app`, an AppImage, an NSIS install root. The release
workflow also ships a `.deb`, an `.apk` and an `.ipa`, and those belong to apt
or to the phone — writing over them from inside the app would fail at best.

The trap is Linux, where **the same binary ships both ways**. Nothing at compile
time distinguishes the copy inside the AppImage from the one `dpkg` unpacked, so
the decision has to be made at runtime, and the test is not a guess: the AppImage
runtime exports `APPIMAGE` pointing at the image it launched from, and that is
the exact variable the updater needs in order to know what to overwrite. Its
presence is therefore the precise condition under which the update would work,
which is why `update_mode` reads it rather than sniffing a path.

The frontend never makes this call — it asks Rust and renders the answer.

### The updater manifest cannot be written by the build matrix

`latest.json` maps every platform to a signed bundle, and five matrix rows
finish at five different times. `tauri-action` will maintain it for you by
downloading the current manifest, adding its own platform and uploading the
result — which across parallel rows is five read-modify-writes of one file,
where the last writer wins and the losers' platforms disappear silently. A
release that looks complete then offers nothing to half its users.

So the upload is off, and `publish` builds the manifest once, after
every row has landed, from the assets that are *actually attached* — not from
what the matrix was supposed to produce. A platform whose build flaked cannot
reach the manifest, and a manifest entry cannot point at a missing asset.

The input that switches it off is `uploadUpdaterJson`, and the name has to be
exact. It defaults to **true**, and an unrecognised input is a warning rather
than an error, so a plausible near-miss — `includeUpdaterJson`, say — leaves the
racing default switched on while the run still goes green. If you touch this,
read the run log for `Unexpected input(s)` rather than trusting the tick.

### The verification modal has to outrank every other overlay

There is a timeout running on the *other* device. A modal that opens behind the
settings pane is a verification that fails, and settings is exactly where the
user is when they start one, so `VerificationModal` sits above every other
`zIndex` in the app.

### WKWebView won't load `<video>`/`<audio>` from a custom URI scheme

Media elements are handed to AVFoundation, which never consults a
`WKURLSchemeHandler` — the request simply never arrives and the element reports
"format not supported" with nothing in the network log. Images are fine over
`horo://`; media goes over IPC and gets wrapped in a `blob:` URL
(`src/lib/blobMedia.ts`). A blob typed `application/octet-stream` won't play
either, so the container is sniffed from the bytes.

### A thumbnail size is a cache key, and remote thumbnails fail slowly

`?w=&h=` goes into the URL, so every distinct pixel size is a separate entry in
the WebView cache, the SDK's media store *and* the homeserver's. Two components
drawing the same avatar at 34px and 66px fetch it twice. That's merely wasteful
for local media — but for *remote* media the server has to federate the original
before it can resize, and when that federation stalls the request sits for the
server's full timeout (ten seconds on Continuwuity) and comes back
`400 M_UNKNOWN` from the thumbnail endpoint, which reads like a client bug and
isn't one.

Four things keep it bearable. `Avatar` asks for one fixed size whatever it draws
at, so a person's picture is a single fetch app-wide and CSS does the scaling —
the profile card at 66px and the timeline at 38px share one cache entry.
`mediaUrl` snaps everything else up to one of a dozen standard buckets. A failed
thumbnail retries as the whole file, which often works because only the resize
needed the original. And a failed request is remembered for a minute so
re-renders fail instantly instead of queueing more ten-second waits — keyed by
URI *and size*, because "this size failed" is not "this media is unavailable",
and keying on the URI alone blanks avatars that were rendering fine.

### Encrypted attachments need the event's `MediaSource`, not just the URI

The keys live in the event; rebuilding a `MediaSource::Plain` from the `mxc://`
downloads ciphertext. Timeline conversion records the real source in
`media::remember_source` and the fetch path looks it up.

### The SDK's media cache is keyed by URI alone

A plain and an encrypted source for the same URI share one entry, so fetching
encrypted media as plain poisons the cache with ciphertext — and fixing the
fetch isn't enough on its own. `purge_poisoned_media_cache` drops the cache once,
behind a marker file.

### Presence never arrives over sliding sync

`m.presence` is an EDU, and EDUs come down legacy `/sync`. `SyncService` is the
only sync we run and there is no presence extension in this SDK, so nothing ever
reaches an event handler — which looks exactly like a homeserver with presence
disabled. `presence.rs` polls `GET /presence/{user}/status` instead, for the set
of people the UI says it is drawing.

That set has to be small, because it is one request per person per round. It
can't be decided in one place either — the member list, the sidebar's DMs and
the open profile card each know their own people — so `lib/presence.ts`
refcounts interest and pushes the union to Rust.

The response's `last_active_ago` is an *age*, and stops being true the moment it
arrives; it's anchored to an absolute instant at receipt so "last seen 12m ago"
keeps counting between polls rather than freezing at whatever the last round
trip said.

### A server with presence off looks exactly like a server where nobody is home

Plenty of deployments run with presence disabled, and there is no capability
flag to ask. Painting that as a grey "offline" dot on every person in the app is
a confident lie, so "we don't know" draws *nothing* — no dot, no last-seen line.
The backend only reports `supported: false` when every request in a round is
refused; one failure among several is just a user we can't see.

### macOS needs `NSMicrophoneUsageDescription`

Without it in `Info.plist`, `enumerateDevices` returns nothing, no permission
prompt ever appears, and calls fail silently with no microphone.
`entitlements.plist` carries the matching
`com.apple.security.device.audio-input` for hardened-runtime builds.

## Rendering other people's HTML

Messages carry a `formatted_body`: an HTML subset written by whichever client
sent it, served by a homeserver we don't control. In a Tauri WebView, script
execution isn't a stolen cookie — it's the IPC bridge, and through it the session
and the message keys.

So `src/lib/richText.tsx` never touches `innerHTML`. It parses the markup inert
with `DOMParser`, walks it, and rebuilds it as React elements from an allowlist.
Unknown tags contribute their text and nothing else. Because we *construct*
elements rather than inject markup, an attribute we failed to anticipate has no
path to becoming executable. Links are restricted to http/https/mailto/matrix,
and images to `mxc:` — a remote `<img>` would leak the reader's IP to whoever
sent the message.

`src/lib/richText.test.tsx` is the guard on that, and reads as a list of the ways
it could go wrong.

## Calls

MatrixRTC as Element Call implements it, so calls interoperate with Element X:

1. Publish `org.matrix.msc3401.call.member` keyed `_@user:server_DEVICEID`,
   naming a preferred LiveKit focus.
2. Pick the focus from the oldest existing membership, so joiners land on the SFU
   already in use rather than starting a second one.
3. Trade a homeserver OpenID token at `<focus>/sfu/get` for a room-scoped JWT.
4. Connect `livekit-client` to the SFU with that JWT.

The SFU comes from `org.matrix.msc4143.rtc_foci` in the homeserver's
`.well-known`, or the override in settings.

Signalling reaches LiveKit over 443; **media does not**. If a call connects and
then immediately drops, the SFU's media ports (`rtc.udp_port` and
`rtc.tcp_port`) aren't reachable — and if you're testing from inside the same
LAN as the server, check NAT hairpinning before anything else.

## Where secrets live

`session.json` in the app data dir holds only non-secret pointers (which user,
which homeserver, which store directory). The access token and the SQLite
passphrase go to the OS keychain. If no keychain is available the app falls back
to a `0600` file **and says so in the UI** — the weaker storage is never silent.

## Wire types

Every type that crosses the bridge derives `ts_rs::TS` in Rust, and
`npm run bindings` writes it out to `src/lib/bindings`. Nothing on the
TypeScript side is written by hand, so the two cannot drift: change a Rust type,
regenerate, and `tsc` points at every use that no longer fits. CI regenerates
too, and fails if the committed files differ.

ts-rs reads the serde attributes, so what it writes is what serde actually
sends. Two things it can't see:

- A value the frontend branches on should be a Rust enum, not a `String`. A
  string comes out as `string`, and a typo in a comparison then compiles.
- A field the backend accepts being left out — `#[serde(default)]` on an input
  type — also needs `#[ts(optional)]`, or the generated type demands it.

`.cargo/config.toml` sits at the repo root, not in `src-tauri/`, because cargo
looks for it from the directory it runs in. It is what keeps a `u64` a `number`
rather than a `bigint`, so regenerate through `npm run bindings`, which runs
cargo from the right place, rather than from somewhere outside the repo.

## Tests

```bash
npm test                                              # frontend
cargo test --manifest-path src-tauri/Cargo.toml --lib # backend
```

The frontend suite is almost entirely the HTML sanitiser. The backend suite is
almost entirely media URL and byte-range parsing. Both are the places where
untrusted input meets code that has to be exactly right.
