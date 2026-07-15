# Home — editorial-calm redesign

**Date:** 2026-05-30
**Scope:** `pages/src/home.rs` + the `home page` / `For You` CSS block in `nira/assets/main.css` (~1675–2470). Discover (`discover.rs`) is explicitly **out of scope** this pass — it inherits the shared tokens in a follow-up.

## Problem

Home accreted two design eras: a large "For You" dashboard (spotlight + 2×2 daily mixes + quick tiles + 4 compact rails + scenes grid) was bolted *on top of* the original three activity sections (Recently played / liked / Listened lately). Result:

- Two whole pages stacked into one endless scroll; redundant (likes/plays surface top *and* bottom).
- Three competing section-header treatments on one page.
- ≥5 distinct card languages (`cover-card`, `mix-card`, `quick-tile`, spotlight cards, `feed-line`).
- Three clashing accent worlds: indigo theme accent, peach-gold hover/glow (`rgba(255,214,153)`), and 8 cycling rainbow hues (`--tile-hue`/`--mix-hue`).

No single visual voice → "looks bad".

## Direction (user-chosen)

- **Editorial calm**: keep black + indigo + slate, keep JetBrains Mono. Calm comes from hierarchy, whitespace, and accent discipline — *not* a font change.
- **Declutter + unify**: merge the For-You dashboard and the activity sections into one continuous flow; one card language.

## Target structure

```
Home
├─ Stage "Made for you"           ← the single expressive moment
│    large lead cover + eyebrow/title/seed + Play · Shuffle · Surprise · Refresh
│    (no border box, generous whitespace)
├─ Rails (uniform cover-card language, horizontal scroll):
│    Recently played · Daily Mixes · Because you played ·
│    From your artists · From your likes · Trending · SoundCloud scenes
│    (scenes are just more rails — no boxed grid panel)
└─ Listened lately                ← the one dense timeline list at the bottom (stays)
```

## Design system changes (shared, tokens)

- **Remove the rainbow hue system**: delete `--tile-hue` / `--mix-hue` usage and `accent_hue()` in `home.rs`. Mix-cards/tiles no longer carry random per-item HSL borders/gradients.
- **Remove peach-gold**: `cover-card` hover border `rgba(255,214,153,0.4)` and the spotlight peach radial glow → indigo (`--accent-soft`, low alpha).
- **One accent = indigo.** Semantic exceptions stay small: like-rose only on hearts; provider badges (Spotify/SoundCloud) keep brand colour.
- **Fewer boxes**: `.for-you-spotlight` and `.for-you-scenes` lose their full-border panel chrome → open sections separated by space + a thin rule. Add a consistent vertical-rhythm value for section spacing.

## Typography

- **One** section-header treatment (currently two: big `for-you-header` clamp vs small uppercase `home-section-header`). Quiet: small, lightly-tracked uppercase eyebrow + optional right-aligned action.
- The stage title is the **only** large expressive heading.
- Drop the generic hint paragraph ("What you've been doing lately…").

## Code changes (`home.rs`)

- New single **`Rail`** wrapper component (header eyebrow/title/action + horizontal card row) replaces `CompactRail`, the `cover-row` usages, `DailyMixesGrid`, and the scenes-grid logic.
- **Drop** `ForYouQuickTiles` / `ForYouTile` (4th card language; content already covered by Daily Mixes).
- **Daily-mix card** keeps the 2×2 mosaic art but adopts the same dimensions/chrome as `cover-card` (no coloured border, no special overlay-label treatment).
- Activity sections become rails (Recently played/liked); Listened lately stays as the bottom timeline list.
- Remove `accent_hue()` helper.

## CSS changes (`main.css`)

- Rework the block ~1675–2470 into a leaner unified set; `.cover-card` is the canonical card and rails reuse it.
- Daily-mix mosaic art reuses cover-card art dimensions.
- Discover CSS untouched.

## Out of scope / unchanged

Data flows, hooks, recommendation engine, queue/playback logic, Provider badges, empty/error states, skeletons, context menu. Black + indigo + slate palette and JetBrains Mono stay. This is visual + structural only.

## Success criteria

- One card language across Home; no rainbow/peach accents; indigo-only.
- One section-header style; one expressive heading (the stage).
- No duplicated module between a "dashboard" and an "activity" half — one continuous flow.
- `cargo check -p pages` compiles; app renders Home without layout overflow.
