# Lumen for TV: design reference

`look-sheet.html` is the 10-foot design of every TV screen, one 1920×1080 frame each (static and self-contained).
`node tv/design/render.cjs [dir]` renders the frames to PNGs (default `/tmp/lumen-tv-look/`). The app's TV
stylesheet is the block `/* ===== TV app` at the end of `extension/client.css` (everything under
`html[data-ytc-tv]`), and it uses the same numbers.

## Grid and tokens

| Token | Value | Notes |
|---|---|---|
| Root size | `font-size: 100vw / 60` | 1 rem = 16 CSS px on a 960×540 WebView (DPR 2), 32 px at 1920×1080, 64 px at 3840×2160: one layout at every resolution |
| Safe area | `--sx: 3rem`, `--sy: 1.6875rem` | 5 % per side (overscan). Content and focus rings stay inside; backgrounds and pictures may bleed |
| Ring | `--ring: .1875rem` (6 px at 1080p), offset `.1875rem` | mint `#b7ebca`, the only focus ring |
| Focus | scale 1.06 (cards, chips, pills), 1.08 (round player buttons), 1.04 (rail items), 1.02 (list rows) | plus a raised surface (`#212b2e`, mint for primary buttons); transform only, `--d-pop` (170 ms) `--e-out` |
| Colours | bg `#101315`, panels `#151a1d` / `#1c2428`, surface `#212b2e`, raised `#293936`, selected `#233d36`, mint `#b7ebca`, text `#edf3f3`, secondary `#a0afb1` | the desktop palette |
| Type (1080p px) | titles 48–56 (`1.5–1.75rem`), body 28 (`.875rem`), secondary 24 (`.75rem`), never below 22 (`.6875rem`: duration badges, the failure reason) | card titles 2 lines, ellipsis |

## Layout

- **Rail**: icons at x = 3.375 rem (2.5 rem tiles, pitch `--tvrow` 2.875 rem from 5.25 rem: Search, Home,
  Subscriptions, History, Watch later, Playlists, Liked), the Lumen mark on top, and at the bottom (25.875 rem)
  the account (Sign in / Account) and About & updates (a mint dot while an update is ready).
  The focus entering it opens it over the content (labels, a panel fading in from the left, the page's mint pill
  widening with them); the content never moves. Back from the content focuses the rail on the page's own item; →
  returns to where the focus was.
- **Pages**: content from x = 7.75 rem to 56.25 rem, starting 2.1875 rem from the top (the chips row or the
  heading, 2.25 rem tall), the grid 1.3125 rem below. **4 cards per row**, 11 rem wide (16:9 thumbnail 6.19 rem),
  1.5 rem gaps: a focused card grown 6 % with its ring still ends inside the safe area. The focused row keeps the
  first row's place while the remote walks the grid (instant scroll, so a held key never waits); a fixed scrim
  fades what scrolled under the top edge.
- **Search**: a screen of its own (the header, full screen): the 3 rem field (OK opens the TV keyboard; its
  microphone dictates), the hint, then recent searches and YouTube's suggestions as a list (→ on a recent search
  reaches its remove button). Results: the query stays on top as the field (↑ reaches it), the grid below.
- **Player**: full screen at once. The overlay (any key; hidden 4 s after the last one while playing): title
  1.5 rem and channel top-left over a scrim; at the bottom the time row (elapsed · chapter · remaining), the bar
  (.25 rem, .375 rem focused, chapter gaps, a 1 rem knob), and the 3 rem round buttons (play/pause, captions,
  speed, quality, like, Watch later, more info). ←/→ on the bar scrub 10 s per press (30, 60, 120 s while held)
  with the storyboard frame above the target; the seek goes out once, when the remote rests. ↓ from the buttons
  slides the overlay up 10 rem for the Up next row (8.5 rem cards). Menus and More info are panels from the right
  edge (19.5 / 26 rem, content inside the safe area), focus on the current value. Captions bottom-centre at
  1.625 rem, 7.75 rem higher while the overlay shows. The skip notice sits bottom-right (↑ from the last button).
- **Sign in / Account**: full screen: the steps on the left (the URL the device flow returned, the code at
  3.25 rem), the QR code of that URL on a light 13 rem tile on the right (`src/qr.rs`), Back first and focused.

- **About & updates**: full screen like Sign in: App and Interface versions, the last check and its result,
  one status card (up to date / update ready / app available with its download progress / the reason it failed)
  and its action focused (Check for updates, Update now, Install update, Retry). While an update is ready a small
  notice sits top right on the pages (never over a video) until the screen was opened.

## Motion

Focus moves are the main interaction on a TV, so they move (scale, ring, the rail opening, panels sliding in from
their edge) but never delay anything: the focus and the content are final at once, the motion is transform and
opacity only, 110–220 ms, and scrolling that follows the focus is instant. No backdrop blur, no animated shadows.
