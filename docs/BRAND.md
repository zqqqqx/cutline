# Cutline brand

Cutline is a calm, honest tool. The brand stays out of the way of the footage: dark, monochrome,
one bright accent, no gradients on controls, no hype.

## Name

- Always **Cutline**: one word, capital C. Never "CutLine", "Cut Line" or "CUTLINE" in running text.
- Features get plain descriptive names ("Remove pauses", "Vertical layout"), not invented brand words.
- Don't name other editors in product text. The importers and shortcut presets are the only places where
  another product's name helps the user.

## Logo

| File | Use |
|---|---|
| [`brand/logo.svg`](brand/logo.svg) | App icon, favicon, avatar: white tile, three clips on the left, one cut, three clips on the right |
| [`brand/banner.svg`](brand/banner.svg) | README and release header |
| `assets/logo.png`, `assets/cutline.ico` | Raster versions for the window and the installer |

The logo is drawn in code from the same geometry (`theme::logo`, `theme::LOGO_SVG`), so the window icon,
the title bar and the welcome screen always match. Keep clear space of at least a quarter of the logo
width around it. Don't recolor the tile, rotate it or add effects.

## Colors

Defined in `crates/cl-app/src/theme.rs`.

| Token | Hex | Use |
|---|---|---|
| `BG_APP` | `#0C0C0E` | Window background |
| `BG_CARD` | `#17171A` | Panels, dialogs |
| `BG_RAISED` | `#222226` | Secondary buttons, raised surfaces |
| `BORDER` | `#29292E` | Hairlines |
| `TEXT` | `#ECECEE` | Primary text |
| `TEXT_DIM` | `#8C8C93` | Secondary text |
| `TEXT_FAINT` | `#58585F` | Hints, captions |
| `ACCENT` | `#F4F4F6` | The one accent: primary buttons, selection, playhead |
| `ACCENT_INK` | `#101012` | Text on the accent |
| `DANGER` | `#E5484D` | Destructive actions only |
| `SNAP` | `#FFD84D` | Snapping guides only |

Color in the interface belongs to the user's media: clip colors, waveforms, thumbnails. The chrome
itself stays neutral.

## Type

The interface uses the platform's UI font (Segoe UI on Windows, DejaVu Sans on Linux, SF on macOS) with
Phosphor icons. Graphics outside the app (banner, social images) use Inter or Segoe UI. Headings are
bold and tight, body text is regular. Sizes in the app: 28 (welcome), 22 (dialog titles), 16 (tour titles),
12.5 (labels), 11 (secondary), 10.5 (captions).

## Voice

- Short, concrete, second person: "Remove pauses from the selected clip", not "Leverage AI-powered
  silence optimization".
- Say what happens and where data goes. "Runs locally", "The key stays on this computer".
- No upsell, no exclamation marks, no fake urgency. Optional things are labeled optional.
- German and English are both first-class; every UI string goes through `tr()`.

## Principles

1. **Local first.** Nothing leaves the computer unless the user turns on an integration.
2. **Off by default.** Plugins and integrations need the user's okay.
3. **Open.** Project files are JSON, the code is MPL-2.0, every feature is reachable without an account.
4. **Explain once.** The welcome tour shows the editor once; after that the UI gets out of the way.
