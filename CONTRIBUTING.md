# Contributing to Cutline

Issues and pull requests are welcome. This page is for people who want to build Cutline or change its code.
If you only want to use it, the [README](README.md) and the [guide](https://github.com/zqqqqx/cutline/wiki) are the right place.

## Build from source

Requirements: a current stable Rust from [rustup](https://rustup.rs) (the Rust packaged by distributions such as Debian 13, 1.85, is too old), and FFmpeg 7 or newer plus FFprobe on your `PATH` (or set `CUTLINE_FFMPEG_DIR`). On Windows you also need the MSVC build tools; on Linux, the usual X11/Wayland and ALSA development packages.

```bash
cargo test --workspace
cargo run -p cl-app --release
```

For local captions and background removal, place an ONNX Runtime 1.31 library (`onnxruntime.dll` / `libonnxruntime.so`) next to the binary or in the models folder.

## Before you send a change

- Run `cargo test --workspace` and `cargo clippy --workspace`.
- A bug fix comes with a test that fails without it.
- Every change that users notice gets a line under `## Unreleased` in [`CHANGELOG.md`](CHANGELOG.md), in English and German, written for users rather than developers. The release notes on GitHub and the "What's new" view in Cutline are taken from that file.
- The code is English: names, comments, log lines and interface texts. Every new interface text gets its German line in `translations/de_DE/ui.tsv` (`English<TAB>German`; a test fails without it).
- Product texts follow [`docs/POSITIONING.md`](docs/POSITIONING.md) and [`docs/BRAND.md`](docs/BRAND.md).

The export test renders every project in `crates/cl-export/tests/projects` with generated stand-in media. To add a case, save a project there. To run it before every commit:

```bash
git config core.hooksPath .githooks
```

## Where things are

[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) describes the crates, the preview and export pipelines, local AI, plugins and translations. [`docs/ROADMAP.md`](docs/ROADMAP.md) lists what is planned.

## Environment variables

| Variable | Purpose |
|---|---|
| `CUTLINE_HOME` | Data folder (fonts, plugins, settings); default `%APPDATA%\Cutline` |
| `CUTLINE_MODELS` | Folder for AI models; default `CUTLINE_HOME/Models` |
| `CUTLINE_FFMPEG_DIR` | Folder containing `ffmpeg` and `ffprobe` |
| `CUTLINE_NO_UPDATE=1` | Don't check for updates on startup |
| `CUTLINE_UI_SCALE`, `CUTLINE_WINDOW_SIZE`, `CUTLINE_WINDOW_POS`, `CUTLINE_MAXIMIZED=1` | Window and scale for testing |
| `CUTLINE_MISSING_TR=file` | Debug builds: log UI strings that have no translation yet |
| `CUTLINE_SHOTS=folder` | Save the window as PNG whenever a file named `take` appears in that folder (for docs) |
| `CUTLINE_DEBUG_HITS=1` | Debug builds: highlight the widget under the pointer |
| `CUTLINE_PERF=1` | Log preview frame rate, stream starts and the largest gap between frames once per second |
| `CUTLINE_NO_ONBOARDING=1` | Skip the first-start introduction |

## Security

Please report security problems as described in [`SECURITY.md`](SECURITY.md), not as a public issue.
