# Cutline brand

**Cutline – the open video editor for creators.** Easy to use, fast, and entirely on your machine.
German wording: **Cutline – das offene Schnittprogramm für Creator.** Einfach zu bedienen, schnell, komplett auf
deinem Rechner.

It is made for content creators: YouTube, Shorts/TikTok/Reels, streaming, podcasts. The brand stays out of the
way of the footage: dark, monochrome, one accent, no gradients on controls, no hype. Details and wording
examples: [`POSITIONING.md`](POSITIONING.md).

## Pillars

1. **Simple** (German: Einfach) – familiar layout, drag in, cut, export; every function is unlocked from the first start.
2. **Fast** (Schnell) – written in Rust, smooth preview with proxies, quick exports with hardware encoders.
3. **Open** (Offen) – open source, runs locally, no account, no subscription, no watermark.

AI is described one way everywhere: "Local AI for captions, cut-outs and noise removal – nothing is uploaded;
no generative AI." German wording: "Lokale KI für Untertitel, Freistellen und Entrauschen – nichts wird
hochgeladen; keine erzeugende KI."

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

One accent colour. The default is white; users can pick a preset or their own colour in Settings. Status colours
(warning, error, success) and clip colours are not accent colours. The logo tile is never recoloured.

The accent marks **state and the one primary action**, never content:

- Accent: selected clip or tile, active-tab indicator, toggles that are on, slider fill and knob, the primary
  button, playhead, focus ring and text cursor, progress, keyframes, in/out marks, drop targets.
- Not accent: window title, wordmark, page and panel headings, section titles, labels, body text, numbers,
  timecode, empty-state titles. These use `TEXT` or `TEXT_DIM`.
- Active text items (tabs, navigation, segments) keep neutral text; only their small indicator takes the accent.
- Icons are neutral unless they show an ON state.

Color in the interface belongs to the user's media: clip colors, waveforms, thumbnails. The chrome
itself stays neutral.

## Type

The interface uses the platform's UI font (Segoe UI on Windows, DejaVu Sans on Linux, SF on macOS) with
Phosphor icons. Graphics outside the app (banner, social images) use Inter or Segoe UI. Headings are
bold and tight, body text is regular. Design tokens going forward: type scale 10.5 / 11.5 / 12.5 / 14 / 18 / 26 and corner radii 4 / 6 / 10.
Existing sizes (28, 22, 16, 11) move to this scale as screens are touched.

## Voice

- Short, concrete, second person: "Remove pauses from the selected clip", not "Leverage AI-powered
  silence optimization".
- Say what happens and where data goes. "Runs locally", "The key stays on this computer".
- No upsell, no exclamation marks, no fake urgency. Optional things are labeled optional.
- German and English are both first-class; every UI string goes through `tr()`.

## Glossary (German terms)

Fixed terms for German UI text. The left column is the German wording on purpose; English follows the same
split.

| German term | Meaning / rule |
|---|---|
| Clip | not "Element" |
| Medien | media; not "Material" |
| Inspektor | panel for clip settings |
| Eigenschaften | properties: info window via right-click |
| Tempo | speed; not "Geschwindigkeit" |
| Vorlage | project template |
| Look | saved clip settings |
| Stil | caption and text style |
| Löschen | delete: gone for good |
| Entfernen | remove from a list |
| Verbergen | hide |
| Klinge | blade: tool (cutting) |
| Teilen | split: action (split clip at the playhead) |
| Schnellaktionen | creator tools (menu in the timeline); English "Quick Actions" |
| Im Explorer zeigen | show the file in the file manager |

## Principles

1. **Local first.** Nothing leaves the computer unless the user turns on an integration.
2. **Off by default.** Plugins and integrations need the user's okay.
3. **Open.** Project files are JSON, the code is MPL-2.0, every feature is reachable without an account.
4. **Explain once.** The welcome tour shows the editor once; after that the UI gets out of the way.
