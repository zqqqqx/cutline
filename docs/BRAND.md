# Cutline brand

**Cutline – das offene Schnittprogramm für Creator.** / The open video editor for creators.
Einfach zu bedienen, schnell, komplett auf deinem Rechner. / Easy to use, fast, and entirely on your machine.

It is made for content creators: YouTube, Shorts/TikTok/Reels, streaming, podcasts. The brand stays out of the
way of the footage: dark, monochrome, one accent, no gradients on controls, no hype. Details and wording
examples: [`POSITIONING.md`](POSITIONING.md).

## Pillars

1. **Einfach / Simple** – familiar layout, drag in, cut, export; every function is unlocked from the first start.
2. **Schnell / Fast** – written in Rust, smooth preview with proxies, quick exports with hardware encoders.
3. **Offen / Open** – open source, runs locally, no account, no subscription, no watermark.

AI is described one way everywhere: "Lokale KI für Untertitel, Freistellen und Entrauschen – nichts wird
hochgeladen; keine erzeugende KI." / "Local AI for captions, cut-outs and noise removal – nothing is uploaded;
no generative AI."

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

## Wörterbuch

Fixed terms for German UI text and docs. English follows the same split.

| Begriff | Bedeutung / Regel |
|---|---|
| Clip | nicht "Element" |
| Medien | nicht "Material" |
| Inspektor | Panel für Clip-Einstellungen |
| Eigenschaften | Info-Fenster per Rechtsklick |
| Tempo | nicht "Geschwindigkeit" |
| Vorlage | Projektvorlage |
| Look | gespeicherte Clip-Einstellungen |
| Stil | Untertitel- und Textstil |
| Löschen | endgültig weg |
| Entfernen | aus einer Liste nehmen |
| Verbergen | ausblenden |
| Klinge | Werkzeug (Schneiden) |
| Teilen | Aktion (Clip am Playhead teilen) |
| Quick Actions | Creator-Werkzeuge (Menü in der Zeitleiste) |
| Im Explorer zeigen | Datei im Dateimanager anzeigen |

## Principles

1. **Local first.** Nothing leaves the computer unless the user turns on an integration.
2. **Off by default.** Plugins and integrations need the user's okay.
3. **Open.** Project files are JSON, the code is MPL-2.0, every feature is reachable without an account.
4. **Explain once.** The welcome tour shows the editor once; after that the UI gets out of the way.
