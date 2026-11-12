# Architecture

Cutline is a Rust workspace. The desktop app is built with egui on wgpu; decoding, effects
and export run through FFmpeg as a separate process. Local AI features use ONNX Runtime and
the FFmpeg Whisper filter. Nothing in the editor needs an account or a network connection.

## Design principles

1. **One edit channel.** Every change is an `EditOp`: from the UI, from MCP clients and from
   plugins. All of them land in the same undo history, tagged with their source.
2. **Local first.** Projects are plain JSON (`.cutline`). Models are downloaded on demand,
   verified and stored locally. Nothing is uploaded.
3. **The preview matches the export.** The player and the exporter build their filter graphs
   from the same `cl-fx` chains, so what you see is what you get.
4. **No generative AI.** Analysis only: speech recognition, matting, tracking, beats and scenes.
5. **Quiet interface.** No upsell banners, badges or nag dialogs. Shortcuts are listed in every
   tooltip, the save state is always visible, and labels are sized to their text.

## Crates

```
crates/
  cl-core     Data model (project, tracks, clips, keyframes), EditOps, undo history, ripple/magnet
  cl-media    FFprobe/FFmpeg: probe, single-frame decode, playback streams, waveforms
  cl-fx       Creative library as FFmpeg filter chains: effects, filters, transitions (xfade),
              adjustments/HSL, masks, LUTs, animations, stickers (SVG), sound effects, text templates,
              voice changer, chroma key, stabilization, matte compositing
  cl-export   Export: builds one filter graph for the whole timeline (sprite pipeline), audio mix,
              hardware encoder detection (NVENC/QSV/AMF), SRT side file
  cl-analyze  Local analysis: Parakeet TDT / Whisper speech recognition, RobustVideoMatting,
              scene detection, beat detection, motion tracking, auto reframe
  cl-fonts    Font import and library (TTF/OTF/TTC/WOFF, variable and color fonts), per project or global
  cl-i18n     Fluent messages plus the runtime UI dictionary (`tr()`), pseudo-localization
  cl-plugin   Plugin manifests, validation and registry (asset packs)
  cl-mcp      MCP server (stdio and token-protected local HTTP); tools map to EditOps
  cl-update   Updates from GitHub Releases: check, download, SHA-256 verify, swap binary
  cl-app      Desktop app (egui/wgpu), binary `cutline`
plugins/
  cutline-basis  Reference asset pack (plugin.toml + WGSL)
installer/
  cutline.iss    Windows installer (Inno Setup, per-user, file association)
```

## Preview pipeline

```
timeline ──► visible layers at the playhead (incl. transitions)
          ──► per media clip: cl-fx preview graph (scale first, then effects at preview size)
          ──► decoder thread: newest request per clip wins, clips decoded in parallel,
              recent frames kept in an in-memory cache (scrubbing back is instant)
          ──► egui textures, composited with transforms, keyframes, animations and text
```

While playing, each video clip gets a continuous FFmpeg stream instead of single frames.
The preview resolution can be lowered (full, half, quarter) in Settings → Performance; export
always renders at full resolution.

## Export pipeline

Every clip becomes a "sprite": source → speed/reverse → effect chain → transform with keyframe
expressions → overlay on the canvas. Transitions use `xfade` and keep the total length.
Text is rendered with outline, box and shadow. Audio clips are mixed with fades, volume
keyframes, voice filters and enhancement. A matte from background removal is merged as an extra
input with `alphamerge`.

## Local AI

| Feature | Model | Runtime |
|---|---|---|
| Auto captions | NVIDIA Parakeet TDT 0.6B v3 (25 European languages, word timing) | ONNX Runtime |
| Caption fallback | Whisper base (99 languages) | FFmpeg `whisper` filter |
| Background removal | RobustVideoMatting (MobileNetV3) | ONNX Runtime → grayscale matte video |
| Scenes, beats, tracking, reframe, stabilization | classic signal processing | FFmpeg + Rust |

ONNX Runtime ships with the release packages. Models are listed under Settings → Models and
downloaded only when needed.

## MCP

The app can be controlled by MCP clients (AI assistants, scripts):

- **stdio:** `cutline --mcp project.cutline` runs headless.
- **HTTP:** Settings → MCP interface. Binds to `127.0.0.1`, requires a bearer token, rejects
  browser origins and can be set to read-only.

Tools: `project_info`, `list_timeline`, `list_media`, `list_fonts`, `history`, `import_media`,
`add_media_clip`, `add_text`, `split_clip`, `trim_clip`, `delete_clip`, `set_format`, `apply_ops`,
`undo`, `redo`. Resources: `project://current`, `project://timeline`.

## Plugins

Asset packs are folders with a `plugin.toml` manifest (id, version, localized names, license)
and assets: effects, transitions and filters as FFmpeg chains or WGSL shaders. See
`plugins/cutline-basis` for the reference pack.

## Translations

The interface is written in German. `cl_i18n::tr()` translates every visible string at runtime
from `crates/cl-i18n/locales/ui.en.tsv` (`German<TAB>English`, `{}` for values). Icons and
shortcuts are handled automatically. To find untranslated strings, run a debug build with
`CUTLINE_MISSING_TR=missing.txt` and click through the app. Settings → General → Language
switches at runtime; the pseudo language makes every string ~40 % longer to catch clipped labels.

## Roadmap

| Phase | Scope | Status |
|---|---|---|
| 0 Foundation | Workspace, data model, undo, FFmpeg probe/decode, UI skeleton, i18n, font import, MCP, plugin format | done |
| 1 Editing | Multi-track timeline, magnet, snapping, split/trim, transform, audio, text, voiceover, export with HW encoding | done |
| 2 Creative library | Effects, filters, transitions, LUTs, adjustments, masks, animations, keyframes, speed curves, stickers, sound effects, text templates | done |
| 3 Local pro tools | Auto captions, background removal, chroma key, stabilization, tracking, auto reframe, scenes, audio tools, voice changer, beats | done |
| 4 Polish | Retouch, body effects, optical flow, motion blur, HDR, alpha export, compound clips, multicam, templates, community registry | planned |
