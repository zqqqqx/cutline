# Cutline – Inhaltspakete (Zweig `packs`)

*English: this branch holds downloadable CC0 content packs (sounds, stickers) for the Cutline video editor; each folder becomes one ZIP on the `packs` release.*

Dieser Zweig hat keine gemeinsame Geschichte mit `main`. Jeder Ordner mit `plugin.toml` ist ein
Inhaltspaket (`kind = "asset-pack"`). Bei jedem Push baut `.github/workflows/packs.yml` je Ordner
eine ZIP (`<ordner>.zip`, `plugin.toml` im Stamm) und eine `index.json` und hängt beides an das
Release mit dem festen Tag `packs` (Vorabversion, kein App-Update). Die App lädt die Pakete von dort.

## Pakete

| Ordner | ID | Inhalt | Einträge |
|---|---|---|---|
| `kenney-oberflaeche` | `kenney.oberflaeche` | Klicks, Schalter, Fenster-, Hinweis- und Digitaltöne | 213 Sounds |
| `kenney-treffer` | `kenney.treffer` | Aufprall, Schritte, Alltagsgeräusche, Karten und Würfel | 233 Sounds |
| `kenney-scifi` | `kenney.scifi` | Laser, Raumschiffe, Explosionen, Technik | 73 Sounds |
| `kenney-jingles` | `kenney.jingles` | Kurze Musikstücke: 8-Bit, Hits, Pizzicato, Saxofon, Steeldrum | 85 Sounds |
| `kenney-stimmen` | `kenney.stimmen` | Englische Ansagen (männlich, weiblich, Kampfansagen) | 138 Sounds |
| `kenney-emotes` | `kenney.emotes` | Sprechblasen (Vektor) | 30 Sticker |
| `kenney-partikel` | `kenney.partikel` | Funken, Flammen, Lichter, Rauch, Spuren (PNG mit Transparenz) | 95 Sticker |
| `kenney-symbole` | `kenney.symbole` | Weiße Symbole: Pfeile, Wiedergabe, Bedienung, Spiel (Vektor) | 105 Sticker |

Sounds: OGG Vorbis, 48 kHz, mono (echtes Stereo bleibt stereo), Spitzenpegel zwischen −12 und −1 dB
(sonst auf −3 dB gebracht). Bilder: SVG oder PNG mit höchstens 512 px Kante.

## Lizenz

Alle Inhalte stammen von Kenney (<https://kenney.nl>) und stehen unter **CC0 1.0**
(<https://creativecommons.org/publicdomain/zero/1.0/>). Die Original-Lizenzdateien liegen als
`LICENSE-<slug>.txt` in jedem Paket; Quellen:
[Interface Sounds](https://kenney.nl/assets/interface-sounds),
[UI Audio](https://kenney.nl/assets/ui-audio),
[Digital Audio](https://kenney.nl/assets/digital-audio),
[Impact Sounds](https://kenney.nl/assets/impact-sounds),
[RPG Audio](https://kenney.nl/assets/rpg-audio),
[Casino Audio](https://kenney.nl/assets/casino-audio),
[Sci-fi Sounds](https://kenney.nl/assets/sci-fi-sounds),
[Music Jingles](https://kenney.nl/assets/music-jingles),
[Voiceover Pack](https://kenney.nl/assets/voiceover-pack),
[Voiceover Pack: Fighter](https://kenney.nl/assets/voiceover-pack-fighter),
[Emotes Pack](https://kenney.nl/assets/emotes-pack),
[Particle Pack](https://kenney.nl/assets/particle-pack),
[Smoke Particles](https://kenney.nl/assets/smoke-particles),
[Game Icons](https://kenney.nl/assets/game-icons).

## Paket ergänzen oder neu bauen

1. Kenney-ZIPs je in einen eigenen Ordner `kenney_<slug>/` entpacken.
2. Neue Quelle in `tools/bau.py` (`PAKETE`) eintragen, Grundnamen → deutscher Name und Kategorie
   in `tools/namen.py` (fehlt einer, bricht der Bau mit dem Namen ab).
3. `python tools/bau.py <entpackt> .` – schreibt die Ordner `kenney-*` neu (braucht FFmpeg).
4. Prüfen: `python tools/index.py . <zip-ordner>` erzeugt die `index.json` lokal aus fertigen ZIPs.
5. Version in `tools/bau.py` (`VERSION`) erhöhen, wenn sich ein Paket ändert; committen und pushen.

Eigene, nicht von Kenney stammende Pakete: Ordner mit `plugin.toml` im Format der mitgelieferten
Pakete (`plugins/` in `main`) anlegen – der Workflow packt jeden Ordner mit `plugin.toml`.

## `index.json`

```json
{ "format": 1, "packs": [ { "id": "kenney.scifi", "name": {"de": "…", "en": "…"}, "version": "1.0.0",
  "api": 1, "description": {"de": "…", "en": "…"}, "file": "kenney-scifi.zip", "size": 1905992,
  "sha256": "…", "counts": {"sound": 73}, "license": "CC0-1.0" } ] }
```

Download: `https://github.com/zqqqqx/cutline/releases/download/packs/<file>` (gleiches Schema für `index.json`).
