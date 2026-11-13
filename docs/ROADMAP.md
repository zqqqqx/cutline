# Roadmap

Phases 0–3 are released (v0.3.0). Phase 4 turns Cutline into a tool for creators: YouTubers,
TikTokers and streamers in gaming and lifestyle, who publish often, in several formats, and need
to get from a raw recording to a finished clip fast.

## Phase 4 – Creator edition

Legend: ✅ done · 🔨 in progress · ⏳ planned

### 4.1 Look and quality (done)

| Feature | Status |
|---|---|
| Person effects from the cut-out matte: blurred background, colored background, outline | ✅ |
| Smooth slow motion with interpolated frames | ✅ |
| Video noise reduction, deflicker | ✅ |
| Transparent export (ProRes 4444, VP9 with alpha) | ✅ |

### 4.2 Integrations (plugins, shipped but off by default)

Integration plugins are compiled in, listed under **Plugins**, and stay disabled until the user
turns them on and enters their own credentials. Nothing talks to a third-party service by default.

| Feature | Status |
|---|---|
| Integration plugin type: enable/disable, settings page, credentials stored locally | ✅ |
| **Epidemic Sound**: browse and search music right in the Audio tab (moods, genres, BPM, vocals), preview, add to the timeline (downloads the MP3 into the project), favorites (local, plus liked tracks from the account when connected), beat markers from the API, usage report on export | ✅ (tested against a local API mock) |
| Sign-in with Epidemic Sound Connect (OAuth with PKCE, system browser, loopback redirect) or a developer API key | ✅ (tested against a local API mock) |
| Further sources later: Freesound (SFX), Pixabay/Pexels (stock video), Google Fonts | ⏳ |

### 4.2b Your own library

| Feature | Status |
|---|---|
| Add your own stickers and images (PNG, WebP, GIF, SVG) and sound effects (WAV, MP3, OGG, FLAC) by button or drag & drop | ⏳ |
| Own folders for stickers and sounds, rename, move, remove; built-in items can be hidden | ⏳ |
| Favorites and search for stickers and sounds (built-in and own) | ⏳ |
| Presets: save the look of a clip (adjustments, filter, effects, animation, text style) and apply it in one click; manage presets | ⏳ |

### 4.3 Bring your projects (import)

Settings → **Import** turns importers on or off; projects open from the home screen
(**Import project…**) or by dropping the file on the window.

| Source | What comes over | Status |
|---|---|---|
| CapCut desktop drafts (`draft_content.json`), with a list of local drafts | Video, image, audio and text clips with timing, speed, volume, position, scale, rotation, opacity, flip; text content, color and size | 🔨 |
| Final Cut Pro / DaVinci Resolve (FCPXML) | Clips, timing, tracks | 🔨 |
| OpenTimelineIO (`.otio`) | Clips, timing, tracks | ⏳ |
| Premiere Pro (XML, xmeml) | Clips, timing, tracks | ⏳ |

Proprietary effects, stickers and templates of other editors can't be carried over; the importer
reports what it skipped.

### 4.4 Faster editing for talking heads and gameplay

| Feature | Why it matters | Status |
|---|---|---|
| **Jump cuts**: remove silences automatically (threshold, min. pause, padding) | Talking-head videos lose 20–40 % of dead air in one click | ⏳ |
| **Filler words**: find and cut "äh/ähm/uh/um/like" from the transcript | Cleaner speech without manual scrubbing | ⏳ |
| **Highlight finder** for gameplay: loud moments (shouts, kills, crowd) become markers or clips | Turns a 2-hour stream into a shortlist | ⏳ |
| **Markers and chapters**: add markers, export YouTube chapters (`00:00 Intro`) | Copy-paste into the video description | ⏳ |
| **Multi-track audio** from OBS recordings: each audio stream as its own track (game, mic, Discord) | Mix game and voice separately | ⏳ |

### 4.5 Short-form and social

| Feature | Status |
|---|---|
| **Animated word captions** (TikTok style): word-by-word highlight, pop, bounce, emoji-free styles, from Parakeet word timing | ⏳ |
| **Gaming layouts** for 9:16: facecam on top, gameplay below; split, picture-in-picture, blurred-fill | ⏳ |
| **Auto reframe** for gameplay (already in 3) plus "keep facecam" region | ⏳ |
| **Export presets**: TikTok, YouTube Shorts, Instagram Reels, YouTube 1080p/4K, Twitch clip; one click sets size, frame rate and bitrate | ⏳ |
| **Multi-export**: render 16:9 and 9:16 in one go | ⏳ |
| **Zoom punch-ins**: quick zoom on a moment (keyframes in one click), auto zoom on beats | ⏳ |
| **Thumbnail maker**: current frame + text template → PNG at 1280×720 | ⏳ |
| **Auto ducking**: music gets quieter under speech | ⏳ |
| Creator stickers and overlays: subscribe/like/follow animations, progress bar, countdown, webcam frames | ⏳ |

### 4.6 Your own library and presets

| Feature | Status |
|---|---|
| **Own stickers and images**: add PNG/SVG/GIF/WebP files (dialog or drag & drop), organized in your own folders | ⏳ |
| **Own sound effects**: add audio files, organize in folders, rename | ⏳ |
| **Favorites** (star) and **search** across built-in and own stickers and sound effects | ⏳ |
| **Remove**: delete own items, hide built-in ones you never use (restorable) | ⏳ |
| **Presets**: save a clip's look (filter, adjustments, effects, LUT, animation) or a text style as a named preset, apply with one click, rename and delete; import/export as a file to share | ⏳ |

### 4.7 Later in phase 4

Retouch (skin smoothing), HDR export, compound clips, multicam, templates with placeholders,
community registry for plugins and asset packs.
