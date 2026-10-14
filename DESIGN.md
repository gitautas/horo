# horo design

The visual language is **kawaii crossed with free-party/tekno rave flyer** —
bubbly and soft where it's cute, neon and glowing where it wants to feel alive
at 3am.

This document is the spec. The running app is the reference implementation; when
the two disagree, this file is what to argue from.

## The one rule

`src/styles/tokens/` and `src/styles/themes/` are the only places a colour,
radius, shadow or easing is written down. Everything else references a token
through `var()`.

There is currently **no raw hex anywhere in `src/`** outside those files. Worth
keeping that way — it's the property that makes the accent switcher and the
themes work at all, and the thing that quietly rots first. A translucent brand
colour is `color-mix(in srgb, var(--acid-500) 35%, transparent)`, not an
`rgba()` of its value.

Components still write sizes as px, but through `corner()`, `space()` and
`textSize()` from `src/lib/scale.ts`, so a theme can scale them. A raw
`borderRadius: 16` won't follow the theme.

Emphasis has its own tokens, so a theme can give it a different face from the
furniture: a title or heading is `--font-heading` sized with `headingSize()`,
and a section label is `labelText()` (or `RaveLabel`). Under classic these are
Baloo 2 and Unbounded caps, exactly as below. Buttons, badges and monograms stay
on `--font-display` / `--font-rave`.

## Themes

`data-theme` on `<html>` picks one; the `theme` setting switches it.

- **horo** (default, `themes/horo.css`) — grounded and compact, in the logo's
  colours: slate `#2b302e` base, cream `#fbecdc` text, gold `#f2ba25` accent.
  **Sora** for body and UI text, **Crimson Pro** for headings and section
  labels (sentence case, not rave caps). Small radii, tighter spacing, no tilts,
  no patterns, no glow. Both faces are bundled from `src/assets/fonts/` (OFL).
- **classic** (`tokens/` as written) — everything the rest of this document
  describes.

A theme only redefines tokens. It keeps the ramp names (`--acid-*`, `--pink-*`,
…) and puts its own colours in them — under horo they are gold, clay, heather
and sage — so the accent switcher works unchanged. The scale tokens
(`--radius-scale`, `--density`, `--text-scale`, `--tilt-scale`) are what make
it compact; classic has them all at 1. The rest of this document describes
classic.

## Colour

Flat dark-club base. **No gradients anywhere in this system.** One background
tone (near-black); accent colour carries all the variety.

| role | token | notes |
|---|---|---|
| base surfaces | `--ink-950` … `--ink-500` | `--surface-app` and `--surface-canvas` are the same flat value |
| acid green | `--accent-primary` | the loudest colour, primary actions |
| logo pink | `--accent-secondary` | |
| violet / cyan | `--accent-tertiary` / `--accent-quaternary` | highlights |
| status | `--status-online` / `-warning` / `-danger` | a separate green/amber/red trio, deliberately **not** the brand neons, so "it's fine" never reads as "it's acid green" |

The four accents are each one ramp: `-500` base, `-400` hover, `-600` pressed.
`applyAccent` in `src/lib/settings.ts` repoints `--accent-primary` at a
different ramp — which is why nothing may hardcode the acid values.

## Type

Four families, each with a job:

- **Baloo 2** (`--font-display`) — bubbly and rounded, all display/headline text. The kawaii half.
- **Unbounded** (`--font-rave`) — bold geometric, uppercase, wide tracking. Tags, stat labels, section headers. Flyer typography.
- **Nunito** (`--font-body`) — body copy. Rounded enough to match Baloo 2 without competing.
- **JetBrains Mono** (`--font-mono`) — anything technical: IDs, timestamps, status lines, key material.

Fonts are self-hosted via `@fontsource` and imported from `main.tsx`. The CSP
forbids remote stylesheets, so a CDN font is not an option.

## Voice

- Warm, a little silly, never corporate. Talk to the person like a friend running their own server.
- **Sentence case everywhere.** Never Title Case. Reserve ALL CAPS for tiny rave tags and section labels, where it reads as flyer typography rather than shouting.
- Second person for the app talking to you ("your homeserver", "your old messages").
- Tildes and soft trailing punctuation are welcome — "pick a room~", "everyone verified~".
- Error and empty states stay gentle and **specific**. "still syncing your rooms…", not "FAILED".

One deliberate exception to gentleness: anything about encryption or key loss
says the true thing plainly. "this deletes the encrypted store on this machine.
any messages whose keys aren't in your key backup become permanently unreadable
— including for you." Cute is for the furniture, not for consequences.

## Shape

- **Radii are generous.** 8/12/16/20/28 and a full pill. Nothing is sharp-cornered.
- **Borders** are 1px translucent white hairlines, `--border-subtle` (8%) → `--border-strong` (24%). No coloured borders as decoration.
- **Cards**: `--surface-card` or `--surface-card-raised`, 1px subtle border, 20px radius, soft ambient shadow.
- **Sticker shadows** (`--shadow-sticker-ink`) are the signature: a hard offset ink shadow on primary buttons and active tiles, often with a small rotation. This is what makes it look like a flyer rather than a dashboard.
- **Backgrounds** get black-and-white ink patterns — sunburst rays, rings, dots, checker, bars — at 7–10% alpha, masked so they fade rather than tile to the edge. Use `.uwu-pattern` + `.uwu-rays` et al. No colour-blend gradients, no photography, no grain.

## Motion

Two eases, and only two:

- `--ease-bounce` — anything playful and interactive. Button presses, toggle thumbs, tile hovers.
- `--ease-out` — passive state changes. Fades, progress.

Hover **lightens** one step (the `-hover` token) rather than darkening: this is
a dark UI, so lightening reads as "lit up". Press squishes rather than
recolouring. Nothing loops or autoplays except a couple of intentional pulses
(typing dots, a sending message).

## Icons

[Phosphor](https://phosphoricons.com), **fill** weight, self-hosted via
`@phosphor-icons/web`. Rounded filled glyphs are the closest match to the
kawaii-but-solid direction.

Emoji are not UI icons. A literal `♡` or `uwu` appearing as a *reaction* is
content, not iconography — that distinction is why the quick-reaction row is
text and the toolbar is Phosphor.

## Where the design lives in the code

`src/components/ui.tsx` holds the shared primitives — `Avatar`, `Button`,
`Tag`, `RaveLabel`, `ChannelBadge`, `PresenceDot`, `HoverRow`, `Spinner`,
`BackdropPattern`.

Everything else is inline style objects on components. That's fine for a UI this
size, but it means the spec is diffuse: the same 16px radius and
`--surface-card` border get retyped in a dozen files. **When a pattern appears a
third time, it belongs in `ui.tsx`.** That file is where the spec should
concentrate as this grows.

## Deviations from the original system, on purpose

The design system was drawn for a homelab dashboard. Two things changed on the
way to a chat client:

- Its component library (`Badge`, `Card`, `Dialog`, `Input`, …) had different
  prop signatures to what we needed. Ours are built to the same tokens and the
  same rules, not to its API.
- Date dividers are a hairline rule with centred text, not a pill. Pills read as
  badges — as *content* — and a date separator isn't content. This is the one
  place we knowingly diverge from the mockup.
