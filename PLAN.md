# Planned features

Features researched but not built. Each section is meant to be enough to start
from cold: the spec, the exact event shapes, the APIs that already exist in the
version we pin, which files change, and the traps.

Two protocol features (custom emoji, room backgrounds), then interface work that
came out of using the thing, then room categories. Sections marked *built* are
kept as a record of what the code does and what it cost to find out — read those
before changing anything near them.

Read [ARCHITECTURE.md](ARCHITECTURE.md) first — several traps there (the media
cache, the sanitiser, zustand selectors) are load-bearing for this work.

Verified against `matrix-sdk 0.18`, `ruma-events 0.34`, `livekit-client 2.x`.

---

## 1. Custom emoji and stickers

The largest thing in this file, and the most valuable. FluffyChat, Cinny and
Commet all implement it, so packs made elsewhere should just work.

### Spec: MSC2545 image packs

Packs live in two places, both plain JSON:

**Room pack** — state event `im.ponies.room_emotes`, state key is the pack id
(`""` for the room's default pack). Anyone with permission can add packs to a
room.

**Personal pack** — account data `im.ponies.user_emotes`. Yours everywhere.

**Which room packs are globally enabled** — account data
`im.ponies.emote_rooms`:

```json
{ "rooms": { "!room:server": { "pack-id": {} } } }
```

Pack content, identical in both cases:

```json
{
  "pack": {
    "display_name": "my pack",
    "avatar_url": "mxc://…",
    "usage": ["emoticon", "sticker"],
    "attribution": "optional"
  },
  "images": {
    "shortcode": {
      "url": "mxc://…",
      "body": "optional description",
      "usage": ["emoticon"],
      "info": { "w": 128, "h": 128, "mimetype": "image/png", "size": 4096 }
    }
  }
}
```

`usage` on an image overrides the pack's. Absent usage means both. `emoticon`
means it can be sent inline and used as a reaction; `sticker` means it can be
sent as a standalone `m.sticker`.

### Sending

**Inline emoji** — a normal `m.room.message` whose `formatted_body` contains:

```html
<img data-mx-emoticon src="mxc://…" alt=":shortcode:" title=":shortcode:" height="32" />
```

The plain `body` should carry `:shortcode:` so clients without support see
something readable.

**Sticker** — `AnyMessageLikeEventContent::Sticker(StickerEventContent)` through
the existing `Timeline::send`. The variant exists in ruma 0.34.

**Reaction** — MSC4027: the reaction key is the `mxc://` URI itself rather than
a unicode grapheme. Support is patchy; send it, and render unknown keys
gracefully.

### What already works

Worth knowing before estimating — the receive path is most of the way there.

- **Stickers already render.** `dto.rs` maps `MsgLikeKind::Sticker` to
  `ContentDto::Sticker` and `MessageRow` draws it.
- **Inline custom emoji already render.** `richText.tsx` allows `<img>` with an
  `mxc:` source, so an emoji sent from FluffyChat shows up today — but at the
  240×180 block size the renderer uses for inline images. It needs to detect
  `data-mx-emoticon` and render at roughly `1.4em`, `vertical-align: middle`,
  inline.
- **Custom-emoji reactions are broken-looking.** `MessageRow` renders
  `reaction.key` as text, so an mxc key shows as a raw URI. Needs an
  `key.startsWith("mxc://")` branch drawing an `<img>`.

### Work

**Rust** — new `src-tauri/src/matrix/packs.rs`:

- `get_image_packs()` — read `im.ponies.user_emotes` via
  `client.account().account_data_raw("im.ponies.user_emotes")`, plus each room's
  `im.ponies.room_emotes` via `room.get_state_event(...)`, filtered by
  `im.ponies.emote_rooms`. Return a flat `Vec<ImagePackDto>`.
- `save_user_pack(pack)` — `account().set_account_data_raw(...)`.
- `save_room_pack(room_id, pack_id, pack)` — `room.send_state_event_raw(...)`.
- `upload_image(path) -> mxc` — `client.media().upload(&mime, bytes, None)`.
  Signature confirmed: `upload(&Mime, Vec<u8>, Option<RequestConfig>)`.
- `send_sticker(room_id, mxc, body, info)`.

**Importing packs.** Two sources, both ours to define:

- *Loose files* — a multi-file picker; shortcode defaults to the filename stem.
- *Discord/Slack export* — these are lists of `{name, url}` (Discord emoji are
  `https://cdn.discordapp.com/emojis/<id>.png`). Fetch each in Rust with
  `reqwest`, upload to our media repo, build a pack. **Do the fetching in Rust,
  not the WebView** — the CSP forbids remote origins, and it keeps a remote URL
  from ever being handed to an `<img>`.

Cap the number and size of images per import; a malicious "pack" is otherwise an
easy way to fill someone's media repo.

**Frontend**

- `src/lib/packs.ts` — types and IPC wrappers.
- `src/components/EmojiPicker.tsx` — tabbed by pack, one grid, a search box.
  Opens from the composer's smiley and from the message hover toolbar (for
  reactions). Reuse `QUICK_REACTIONS` as the "frequently used" row.
- Composer: `:shortcode:` autocomplete. A popup over the composer filtering as
  you type, Tab/Enter to accept. **This is the fiddliest part** — leave it last,
  the picker is usable without it.
- `richText.tsx`: inline sizing for `data-mx-emoticon`.
- `MessageRow`: image reactions, and a sticker-sized branch.
- Settings: a "emoji & stickers" section for managing and importing packs.

### Traps

- Shortcodes collide across packs. Resolution order should be personal pack,
  then this room's, then other enabled rooms — and the picker should show which
  pack an emoji came from when ambiguous.
- Sending an emoji means sending Markdown *and* HTML. The composer currently
  always sends `markdown: true`; injecting `<img>` into a Markdown body needs
  care, or build the `formatted_body` directly and bypass the Markdown path.
- Animated GIF emoji at 32px still decode at full size. Fine for a few, a
  problem for a wall of them.

---

## 2. Room backgrounds

Small, self-contained, and entirely ours.

### There is no MSC for this

Nothing standard exists. Element, FluffyChat and Cinny don't have it; SchildiChat
does wallpapers but locally, not shared. So "set it for everyone" means a
namespaced state event that only horo reads:

```
type:      gg.uwu.room_background
state_key: ""
content:   { "url": "mxc://…", "blur": 0.0–1.0, "dim": 0.0–1.0 }
```

Being namespaced is the point — if an MSC ever lands, we read both and write the
standard one. Don't squat on an `m.*` name.

A local-only background (just for you) belongs in account data or
`localStorage`, keyed by room ID. Offer both: "just for me" and "for everyone in
this room".

### Work

**Rust** (`matrix/rooms.rs`):
- `get_room_background(room_id)` — `room.get_state_event("gg.uwu.room_background", "")`,
  deserialise leniently, return `None` on anything unexpected.
- `set_room_background(room_id, url, blur, dim)` — `send_state_event_raw`.
  Requires power to send state; surface a clear error when the user can't.
- Reuse the `upload_image` command from feature 1 — worth building that first
  either way.

**Frontend**
- `RoomInfo.tsx`: a "background" section — a preview, an upload button
  (`@tauri-apps/plugin-dialog`), a blur slider, a dim slider, a "for everyone"
  toggle, and remove.
- `ChatPane.tsx`: render the image behind the timeline —
  `position: absolute; inset: 0; background-image: url(horo://media/…);
  background-size: cover; filter: blur(Npx); z-index: 0`, with the timeline and
  composer above it. The existing `.uwu-pattern` is the precedent.
- Add `background` to `RoomSummary` so the room list doesn't need a second
  round trip.

### Traps

- **Legibility is the whole problem.** Message text sits directly on this. The
  dim layer isn't optional garnish — without it a light photo makes the room
  unreadable. Consider forcing a minimum dim, and check the timeline against a
  white image before calling it done.
- Blur on a large image is expensive to repaint. Blur once into a scaled-down
  copy rather than applying a large CSS blur radius to a full-size background on
  every scroll.
- The `horo://` media protocol serves this fine — but a background is a *large*
  image, so request a thumbnail rather than the original.

---

## 3. Interface work

Four asks, all frontend except where noted. None of them need a spec — they're
ours to decide — so what's written down here is the shape and the traps. Three
are built; one isn't.

### 3a. Zoom into a picture — built

`Lightbox.tsx`, opened from timeline images and stickers, room avatars, and the
avatar and cover on a profile card. It asks for the original — `mediaUrl` with
no width or height — because a thumbnail scaled up is blurry *and* a size is a
cache key. One image at a time: an original-size animated GIF decodes at
original size. Save goes through the `save_media` command that already existed.

### 3b. Stage attachments in the composer

Paste and drop currently send immediately, one event per file. They should land
in the composer as pending attachments instead — thumbnails in a tray above the
text box, removable, sent with the message when you hit send. This is what makes
"three screenshots and a sentence" one message instead of four.

**Work**

- `Draft` in `src/store/index.ts` grows `attachments: PendingAttachment[]` —
  `{ id, name, mime, size, previewUrl, bytes | path }`. Files pasted arrive as
  bytes, dropped ones as paths; keep both shapes rather than reading every
  dropped file into memory.
- `Composer.tsx`: the tray, a remove button per item, and `submit()` sending the
  attachments before the text.
- `lib/upload.ts` becomes staging rather than sending; `DropZone` and the paste
  handler call into it unchanged.

**Traps**

- **Matrix has no multi-file message** in the stable spec — each attachment is
  its own event. MSC4274 galleries do exist and `Timeline::send_gallery` is
  right there in matrix-sdk-ui 0.18, but behind the `unstable-msc4274` feature
  flag, and other clients will show a gallery as separate files at best. Send N
  events unless we decide to turn the flag on.
- A caption attaches to *one* event. Decide where the composer text goes: on the
  first attachment (Element's behaviour) or as its own message after.
- Pasted bytes sit in memory until send. The 100mb cap in `upload.ts` is now a
  cap on the whole tray, not on one file.
- Preview URLs from `URL.createObjectURL` need revoking when an item is removed
  or the draft is cleared, or the WebView holds every screenshot you pasted all
  session.

### 3c. The DM sidebar shows the person — built

`ProfileHeader` draws cover, avatar, name, handle and status, and both the
profile card and the top of a DM's sidebar use it, so the two can't drift.
`RoomInfo` falls back to the room header whenever it can't tell who the DM is
with — you can be alone in a direct room, or still be flagged direct after
other people joined — and a DM shows the person's bio where a room would show
its topic.

`src/lib/profiles.ts` is the cache the third caller made necessary: one entry
per user, in-flight requests deduped so two components mounting together ask
once, five minute freshness, and an explicit invalidate after you edit your own
profile. Not in the store — nothing about it is UI state.

### 3d. Room toggles as icons — built

Mute, favourite and low priority are one row of `IconToggle`s. That component
and the switch from `SettingsView` both live in `ui.tsx` now — there were two
hand-rolled switches drawn differently in two files.

Renaming and leaving moved out of that panel entirely, into a dialog behind the
pencil. `Modal` is in `ui.tsx` for the same reason as `IconToggle`.

---

## 4. Creating and joining rooms — built

A `+` in the room list header; create and join in one dialog. `create_room`
takes a name, topic, public or invite-only, an optional alias and encryption.
The empty state offers the button it used to only describe.

What this cost, and is worth knowing before touching any of it again:

- **An alias is not a permission.** It's a nickname resolving to a room ID; an
  invite-only room can have one without becoming joinable. Who gets in is the
  join rule. The field was originally hidden behind "anyone can join", which
  implied a connection that doesn't exist.
- **A new room must be filed at both ends.** `m.space.child` in the space is the
  half other clients read, but the store's copy of a space's children is only
  re-read on a 60 second poll, so the room appeared loose for up to a minute.
  The room's own `m.space.parent` lands with the room's own diff, immediately.
  We hold every power in a room we just made, so that half always succeeds; the
  space's half can fail on someone else's space, and only warns.
Still missing here: editing a room's avatar, and inviting people from anywhere
other than a room you're already in.

### Leaving is supported; forgetting is deliberately not

Leaving was built, removed, and then restored — but only the safe half. The
dialog behind the pencil leaves a room and stops there. It never calls
`forget_room`, and shouldn't. Don't add that without reading this.

Four separate faults came out of the original attempt, three of them fixed —
all four belong to forgetting, not to leaving:

- **`forget` refuses while the room still looks joined.** `leave` only sends the
  request; the state turns Left when sync brings it back — except `leave_impl`
  *also* calls `base_client().room_left()` itself, so "is it Left yet" answers
  yes instantly and means nothing. Forgetting on that signal fires while sync is
  still working on the room.
- **An open timeline subscribes to the room's event-cache rows**, and forgetting
  deletes exactly those rows. That panicked the SDK from a background task —
  "The chunk is not found" — and took the process down.
- **Deleting those rows under in-flight writes** produces
  `FOREIGN KEY constraint failed` from `handle_room_updates`, repeatedly.
- **And the one that couldn't be fixed from here:** a forgotten room comes back.
  `forget_room` does remove it from the state store, but a later sync writes it
  back, and `restore_session` runs with `RoomLoadSettings::All`, which loads
  every room in that store at startup. So the room returns after a restart,
  needing to be forgotten again, for ever. The server is not confused — asking
  for the room's members answers 403 — only the local cache is.

The workaround was to remember forgotten rooms and refuse to show them. That
worked, and it was still a client-side lie about the contents of its own store,
so it went too. If this comes back, the honest fixes are upstream: `forget_room`
needs to stop a room being re-admitted by a sync already in flight, or
`RoomLoadSettings` needs to not resurrect it.

**Known bug, accepted for now: leaving takes two goes.** The room stays in the
sidebar after the first leave and only disappears on a second attempt. The
second attempt cannot be what fixes it — `Room::leave` filters out any room
whose state is already `Left` (`room/mod.rs:489`), sends nothing, and returns
`Ok(())` — so the trigger is something else settling in the background, not the
extra request. Pushing the summary from `leave_room` with `emit_room_refresh`,
which is the fix for the identical-looking mute bug, was tried and did **not**
help; it's been reverted rather than left in as a fix that isn't one. The
`tracing::debug!` in `rooms::leave` reports the room's state on entry, which is
the next thing to read when this is picked up.

**Leaving without forgetting never produced a ghost** — a left room is filtered
by `membership == "left"` and nothing gets deleted — which is why that is the
version now shipping. `rooms::leave` closes the room's timelines first (an open
timeline holds a subscription to the room's event cache, and a stream pulling on
a room the server has stopped sending is at best pointless), then leaves. The UI
puts it at the foot of the room settings dialog behind a confirmation, because
getting back into a private room needs someone else to invite you.

---

## 5. Room categories, Discord style

Rooms inside a space should group under collapsible headings, ordered
deliberately rather than by recency.

### Subspaces are the categories

Decided rather than discovered: a category is a **subspace**, so the grouping is
the same one Element shows and the same one other people see in their own
spaces. The alternative — horo-only folders in account data — would let you
arrange anything, including rooms in no space at all, but nobody else would see
it and it would ignore how a space's owner organised things.

The cost of the choice: rearranging a space needs power *in that space*, so you
can't regroup someone else's server to taste.

### The event already carries what's needed

`m.space.child` has more in it than we keep:

```
content: { via: ["server"], order: "aaa", suggested: false }
```

`order` is a string sorted by Unicode codepoint, ascii `\x20`–`\x7E` only, 50
characters max; children without one come after those with one, ordered by their
create event's timestamp. Invalid `order` values must be ignored rather than
treated as absent — ruma already returns `None` for them.

`space_children` currently throws all of this away and returns bare IDs, and
`SpaceSummary.children` is a flat `Vec<String>` with no idea which children are
themselves spaces.

### Work

**Rust** — `space_children` keeps `{ id, order, suggested }`, and marks which
children are spaces (`client.get_room(id).is_some_and(|r| r.is_space())`, which
is a store read, not a request). `SpaceSummary` grows a structured `children`.
Sorting belongs here, not in the UI: `order` first, then create timestamp.

**Frontend** — `groupRooms` learns about subspaces, so the sidebar's sections
come from the space rather than from the fixed invites/DMs/rooms split when a
space is selected. Sections collapse, remember their state in local settings,
and roll up unread counts when closed. "New category" is a subspace, added to
the create dialog.

### Traps

- **A child can be in several spaces**, and a room can be a child of a space
  that's also a child of the space you're looking at. Decide what to do about
  depth before building it — one level of nesting is what Discord has and is
  probably all this wants.
- A space's children include rooms you haven't joined. They're worth showing as
  something joinable rather than hiding — that's the one place the room list can
  usefully offer a room you don't have.
- Reordering means writing `m.space.child` back with a new `order`, which needs
  power in the space. Fall back to read-only ordering when it's not ours.
- The children list is still only re-read on that 60 second poll. Any change we
  make to a space needs `refreshSpaces()` after it, the same as room creation
  does now.

---

---

## 6. Profile cover photo and bio — built

Kept as a record of what the code does and why, not as work to do.

Bio and status use Commet's keys (`chat.commet.profile_bio`, an object with a
`body`; `chat.commet.profile_status`, a bare string) so a bio written in either
client shows in the other. Cover photos are `gg.uwu.cover_url` — nothing else
writes one, so only horo reads it. Continuwuity implements MSC4133 without
advertising `uk.tcpip.msc4133`, which is why an earlier capability check
concluded wrongly: test the endpoint, not the advertisement.

Editing is in settings → account. Reading is the profile card, which hangs off
any avatar in the app (`src/components/ProfileCard.tsx`) and also shows shared
rooms and a way into a DM.

**Shared rooms are two sources unioned**, because neither is complete: MSC2666
(`/_matrix/client/unstable/uk.half-shot.msc2666/user/mutual_rooms`), which not
every server implements, plus a local scan with `get_member_no_sync`, which only
sees rooms whose member list has been loaded. Neither is worth a network round
trip per room, so an incomplete list is the accepted trade.

Remaining: profile fields are refetched each time a card opens. Fine at this
size, worth a cache if cards get opened in bulk.

## Suggested order

1. Room categories (5). The biggest of what's left, and the one that changes how
   the sidebar reads. Backend first: keep `order` and `suggested`, and mark
   which children are spaces.
2. Staging attachments in the composer (3b). The only item with a real design
   decision in it — N events or the MSC4274 gallery flag.
3. Room backgrounds (2) — self-contained, finishable in one sitting. The upload
   it needs already exists as `upload_media`, built for cover photos.
4. Custom emoji and stickers (1) — the big one, in this order: pack storage →
   picker → sending → reactions → import → autocomplete.
