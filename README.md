<p align="center">
  <img src="docs/brand/banner.svg" alt="Cutline – the open video editor that runs on your machine" width="100%">
</p>

<p align="center">
  <a href="https://github.com/zqqqqx/cutline/releases/latest"><img src="https://img.shields.io/github/v/release/zqqqqx/cutline?label=download&color=F4F4F6&labelColor=17171A" alt="Latest release"></a>
  <a href="https://github.com/zqqqqx/cutline/actions/workflows/ci.yml"><img src="https://github.com/zqqqqx/cutline/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/license-MPL--2.0-F4F4F6?labelColor=17171A" alt="MPL-2.0">
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-F4F4F6?labelColor=17171A" alt="Windows and Linux">
</p>

<p align="center">
  <b>A fast, local, open-source video editor written in Rust.</b><br>
  No account, no subscription, no watermark, no cloud. Your footage stays on your machine.
</p>

<p align="center">
  <a href="https://github.com/zqqqqx/cutline/releases/latest"><b>Download for Windows</b></a> ·
  <a href="https://github.com/zqqqqx/cutline/releases/latest"><b>Linux</b></a> ·
  <a href="#build-from-source">Build from source</a> ·
  <a href="docs/ROADMAP.md">Roadmap</a>
</p>

<p align="center">
  <img src="docs/images/editor.jpg" alt="The Cutline editor with a beach video, title text, music and voiceover" width="100%">
</p>

## Why Cutline

<table>
  <tr>
    <td width="33%" valign="top">
      <h3>Fully local</h3>
      Captions, background removal, tracking and beat detection run with open models on your computer. Nothing is uploaded.
    </td>
    <td width="33%" valign="top">
      <h3>Honest</h3>
      Every feature is free. No watermark, no ads, no upsell. Projects are plain JSON, the code is MPL-2.0.
    </td>
    <td width="33%" valign="top">
      <h3>Built for creators</h3>
      Jump cuts, filler words, auto ducking, highlights, multicam, vertical layouts and export presets for every platform.
    </td>
  </tr>
</table>

## One click for the tedious parts

<img src="docs/images/tools.jpg" alt="The Tools menu with jump cuts, filler words, auto ducking, highlights, sync, multicam, punch-in, vertical layout, thumbnail and recap" width="100%">

The **Tools** menu in the timeline does the work that usually eats an evening: remove pauses from talking
parts, cut filler words found by the local speech model, lower music under speech, turn loud moments of a
long recording into markers, sync several recordings by their audio and cut between them live, switch to a
9:16 layout with the facecam on top, or save the current frame as a thumbnail.

## Set up in a minute

<table>
  <tr>
    <td width="50%"><img src="docs/images/welcome.jpg" alt="Welcome screen"></td>
    <td width="50%"><img src="docs/images/uses.jpg" alt="Choosing what you use Cutline for"></td>
  </tr>
  <tr>
    <td>A short, skippable introduction on first start.</td>
    <td>Tell Cutline what you make. It suggests matching tools, models and plugins, and nothing is turned on without your okay.</td>
  </tr>
</table>

<img src="docs/images/tour.jpg" alt="Guided tour highlighting the timeline" width="100%">

A guided tour points out each part of the editor. You can restart it any time from **Menu → Tour**.

## Export for every platform

<img src="docs/images/export.jpg" alt="Export presets for YouTube, Shorts, TikTok, Reels, Instagram and Twitch" width="70%">

Presets for YouTube (1080p, 1440p, 4K), Shorts/TikTok/Reels, Instagram and Twitch, with hardware encoding,
transparent video and audio-only export. Cutline warns you when the project's aspect ratio does not match.

## Plugins

Integrations ship with Cutline but stay **off** until you turn them on. Keys and sign-ins stay on your computer.

| Plugin | What it adds |
|---|---|
| Epidemic Sound | Music and sound effects in the Audio tab (your own account) |
| Freesound | Hundreds of thousands of Creative Commons sound effects with license info (free key) |
| Pexels | Free stock videos and photos in the Media tab (free key) |
| Story tools | Characters with name, color and portrait; dialogue boxes and name tags in one click |

## Features

<details>
<summary><b>Full feature list</b></summary>

| | |
|---|---|
| **Timeline** | Multiple video, text and audio tracks · track names, solo and clip color labels · markers with notes (copy as YouTube chapters) · In/Out range with loop · J/K/L shuttle · roll, slip, nudge and numeric timing · clip groups · paste attributes · main-track magnet (ripple) · snapping to clips, playhead and beats · marquee and multi-selection · copy/paste · freeze frame · reverse · speed 0.1–10× and speed curves · smooth slow motion with interpolated frames |
| **Player** | All layers live, picture-in-picture · drag, scale and rotate with snapping · zoom 50–400 % · large player mode · preview in its own window for a second screen · audio meters · proxy media for smooth playback of large recordings |
| **Creator tools** | Remove pauses (jump cuts) · remove filler words · auto ducking · highlights as markers · sync recordings by audio · multicam editing (1–9 while playing) · speaker recordings import · zoom punch-in · vertical 9:16 layout with facecam · thumbnail PNG · recap from markers · export presets for YouTube, Shorts/TikTok/Reels, Instagram and Twitch |
| **Library** | One asset system for stickers, sound effects, music, effects, filters, transitions, text templates, fonts and media: search, favorites, own folders, and every category can be moved, renamed or hidden · own stickers, sounds and presets |
| **Look** | Adjustments (light, color, HSL per range, sharpness, vignette, grain) · filters · LUT (.cube) · masks (linear, mirror, circle, rectangle) · blend and opacity · video noise reduction · deflicker |
| **Motion** | Keyframes for position, scale, rotation, opacity and volume · in/out/combo animations · transitions between clips |
| **Text** | Templates and text effects · outline, box and shadow · auto wrap · imported fonts · SRT import/export |
| **Audio** | Waveforms · volume keyframes and fades · voiceover recording with countdown · noise reduction · voice enhancement · loudness to −14 LUFS · bass/treble · 13 voice changers · music folder and sound effects |
| **Local AI** | Auto captions with NVIDIA Parakeet TDT 0.6B v3 (25 European languages, word timing), with Whisper as the fallback · background removal without a green screen (RobustVideoMatting), with blurred or colored background and person outline · chroma key with eyedropper · stabilization · object tracking · auto reframe for 9:16 · scene detection · beat detection |
| **Export** | MP4, MOV, MKV, WebM, GIF · audio-only WAV, MP3, M4A, FLAC · H.264, HEVC, AV1, VP9, ProRes · NVENC, Quick Sync and AMF hardware encoding · up to 4K · transparent video (ProRes 4444, VP9 with alpha) · SRT side file |
| **App** | Skippable introduction with plugin suggestions and a guided tour · project overview with search, duplicate and trash · project templates · autosave and automatic backups with restore · relink missing media · archive a project with its files · customizable shortcuts (Cutline, Premiere-like, Resolve-like) · optional plugins, off by default: Epidemic Sound, Freesound, Pexels, Story tools · English and German interface · adjustable UI scale · built-in updater |

</details>

## Install

**Windows:** download `cutline-windows-x64-setup.exe` from the [latest release](https://github.com/zqqqqx/cutline/releases/latest). The installer needs no admin rights. It adds Cutline to the Start menu and opens `.cutline` files. FFmpeg and ONNX Runtime are included. A portable `cutline-windows-x64.zip` is available as well.

**Linux:** download `cutline-linux-x64.zip`, unpack it and run `./cutline`. You need FFmpeg 7 or newer (8 for Whisper captions) installed on your system.

AI models are not bundled. Download the ones you need under **Settings → Models**.

## Build from source

Requirements: Rust (stable), and FFmpeg plus FFprobe on your `PATH` (or set `CUTLINE_FFMPEG_DIR`). On Windows you also need the MSVC build tools; on Linux, the usual X11/Wayland and ALSA development packages.

```bash
cargo test --workspace
cargo run -p cl-app --release
```

For local captions and background removal, place an ONNX Runtime 1.31 library (`onnxruntime.dll` / `libonnxruntime.so`) next to the binary or in the models folder.

<details>
<summary>Environment variables</summary>

| Variable | Purpose |
|---|---|
| `CUTLINE_HOME` | Data folder (fonts, plugins, settings); default `%APPDATA%\Cutline` |
| `CUTLINE_MODELS` | Folder for AI models; default `CUTLINE_HOME/Models` |
| `CUTLINE_FFMPEG_DIR` | Folder containing `ffmpeg` and `ffprobe` |
| `CUTLINE_NO_UPDATE=1` | Don't check for updates on startup |
| `CUTLINE_UI_SCALE`, `CUTLINE_WINDOW_SIZE`, `CUTLINE_WINDOW_POS`, `CUTLINE_MAXIMIZED=1` | Window and scale for testing |
| `CUTLINE_MISSING_TR=file` | Debug builds: log UI strings that have no translation yet |

</details>

## Keyboard shortcuts

| Key | Action | Key | Action |
|---|---|---|---|
| Space | Play / pause | Ctrl+B | Split at playhead |
| Q / W | Delete left / right of playhead | Del | Delete selection |
| Ctrl+C / X / V / D | Copy / cut / paste / duplicate | Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| ← / → | One frame back / forward | Home / End | Start / end |
| P / N | Magnet / snapping | V | Disable clip |
| R | Voiceover | F | Large player |
| Ctrl+I / S / O / E | Import / save / open / export | Ctrl+wheel | Zoom timeline |
| J / K / L | Shuttle back / pause / forward | I / O | In / Out point |
| M | Marker | Shift+Del | Ripple delete |
| Ctrl+G / Ctrl+Shift+G | Group / ungroup | Ctrl+Alt+V | Paste attributes |

## Control Cutline over MCP

**Headless (stdio):**

```json
{ "mcpServers": { "cutline": { "command": "cutline", "args": ["--mcp", "/path/to/project.cutline"] } } }
```

**Running app (HTTP):** go to Settings → MCP interface and turn it on. The server listens only on `127.0.0.1` and requires a bearer token. It can be set to read-only. Use the copy button to get a ready-made client configuration.

## Project layout

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the crates, the preview and export pipelines, local AI, plugins, translations and the roadmap.

## Contributing

Issues and pull requests are welcome. Run `cargo test --workspace` and `cargo clippy --workspace` before you send a change. To add translations, edit `crates/cl-i18n/locales/ui.en.tsv`.

## License

Cutline is licensed under the [Mozilla Public License 2.0](LICENSE). FFmpeg, ONNX Runtime and the AI models are separate programs and files under their own licenses.

---

<sub>Footage in the screenshots: "Sunset timelapse on the beach in Libreville, Gabon" and "Ocean surface waves 06" from Wikimedia Commons, both CC0. Brand assets and guidelines: [`docs/BRAND.md`](docs/BRAND.md).</sub>
