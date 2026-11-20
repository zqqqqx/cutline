# Positioning

Single reference for all product texts (README, app, installer, store pages, release notes). Lines marked DE or
"German wording" are the German product texts on purpose.

## Tagline

- EN: **The open video editor for creators.** Easy to use, fast, and entirely on your machine.
- DE: **Das offene Schnittprogramm für Creator.** Einfach zu bedienen, schnell, komplett auf deinem Rechner.

## Audience

Content creators: YouTube, Shorts/TikTok/Reels, streaming, podcasts. Not narrowed to one genre.

## Pillars

| | Simple (Einfach) | Fast (Schnell) | Open (Offen) |
|---|---|---|---|
| Claim | Familiar controls: drag in, cut, export. Every function unlocked from the first start. | Written in Rust, smooth preview, fast export. | Open source, local, no account, no subscription, no watermark. |
| Proof 1 | Quick Actions: remove pauses, filler words, auto-ducking, vertical layout (`timeline.rs`) | Proxy media for smooth playback of large recordings (`app.rs`) | No account, no subscription, no watermark; projects are JSON, code under MPL-2.0 |
| Proof 2 | Export presets for YouTube, Shorts, TikTok, Instagram, Twitch (`cl-export`) | Hardware encoders NVENC, Quick Sync, AMF (`cl-export/src/lib.rs`) | Local AI (captions with Parakeet/Whisper, cut-outs, noise removal), MCP interface (`--mcp`) and plugin system |

## AI wording

- EN: Local AI for captions, cut-outs and noise removal – nothing is uploaded; no generative AI.
- DE: Lokale KI für Untertitel, Freistellen und Entrauschen – nichts wird hochgeladen; keine erzeugende KI.

## Voice

Short, concrete, second person, no exclamation marks, no hype words, say what happens. Do not name other editors
in product texts (importers and shortcut presets are the exception). Terms: see the glossary in `BRAND.md`.

## Do / Don't

German wording in quotes, English meaning in brackets.

| Do | Don't |
|---|---|
| "Einfach zu bedienen, schnell, komplett auf deinem Rechner." (Easy to use, fast, and entirely on your machine.) | "Die ultimative KI-Revolution für deine Videos!" (The ultimate AI revolution for your videos!) |
| "Kein Konto, kein Abo, kein Wasserzeichen." (No account, no subscription, no watermark.) – as proof under Open | "Fair und frei von Abzocke" (Fair and free of rip-offs) – headline, vague |
| "Entfernt Pausen aus Sprechpassagen." (Removes pauses from speech.) | "Perfekt für Gaming-Streamer" (Perfect for gaming streamers) – narrows the audience |
