# Roadmap

Phases 0–3 are released (v0.3.0); phase 4 is done. Phase 4 turns Cutline into a tool for creators: YouTubers,
TikTokers and streamers in gaming and lifestyle, who publish often, in several formats, and need
to get from a raw recording to a finished clip fast.

## Phase 4 – Creator edition (done)

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
| Add your own stickers and images (PNG, WebP, GIF, SVG) and sound effects (WAV, MP3, OGG, FLAC) by button or drag & drop | ✅ |
| Own folders for stickers and sounds, rename, move, remove; built-in items can be hidden | ✅ |
| Favorites and search for stickers and sounds (built-in and own) | ✅ |
| Presets: save the look of a clip (adjustments, filter, effects, animation, text style) and apply it in one click; manage presets | ✅ |

### 4.3 Bring your projects (import)

Settings → **Import** turns importers on or off; projects open from the home screen
(**Import project…**) or by dropping the file on the window.

| Source | What comes over | Status |
|---|---|---|
| CapCut desktop drafts (`draft_content.json`), with a list of local drafts | Video, image, audio and text clips with timing, speed, volume, position, scale, rotation, opacity, flip; text content, color and size | ✅ |
| Final Cut Pro / DaVinci Resolve (FCPXML) | Clips, timing, tracks | ✅ |
| OpenTimelineIO (`.otio`) | Clips, timing, tracks | ⏳ |
| Premiere Pro (XML, xmeml) | Clips, timing, tracks | ⏳ |

Proprietary effects, stickers and templates of other editors can't be carried over; the importer
reports what it skipped.

## Phase 5 – Series edition

Phase 5 is for people who publish **episodes**: story-driven series, scripted shows, long
let's-plays and podcasts with several speakers, cut down into shorts afterwards. That means
hours of footage per episode, several recordings of the same scene, a script, many voices –
and a weekly deadline. Every item below is local, free and works offline.

Legend: ⏳ planned · 🔨 in progress · ✅ done

### 5.1 Everyday comfort (QoL)

| Feature | Why |
|---|---|
| **J / K / L** shuttle (play back, pause, play forward, tap again for 2×/4×), **audio scrubbing** | The fastest way to find a moment |
| **In/Out points** (I / O), loop playback of the range, export or render just the range | Review a scene over and over |
| **Markers** (M) with color, note and duration; marker list; jump between markers | Notes for retakes, chapters, sound cues |
| **Roll, slip and slide** edits, ripple delete (Shift+Del), numeric trim (type `+12f`, `-0.5s`) | Frame-precise editing without zooming |
| **Clip color labels** and **track names**, solo (S) per track, track height presets | Order in timelines with 20+ tracks |
| **Compound clips / scenes**: group clips into one, open and edit inside, reuse | Keep long episodes manageable |
| **Paste attributes** (choose which: look, audio, transform, speed) | Faster than presets for one-offs |
| **Relink missing media** and **collect & archive** (project + used files + fonts in one folder) | Move projects between PCs or editors |
| **Auto backups** every N minutes with version list and one-click restore; crash recovery | Never lose an episode |
| **Proxy media** for 1440p/4K/high-frame-rate recordings, created in the background | Smooth editing of long recordings |
| **Customizable shortcuts** with presets (Cutline, Premiere-like, Resolve-like) | Muscle memory from other editors |
| **Second-monitor preview** (fullscreen player on another screen) | Watch while editing |
| **Audio meters** in the player and per track, loudness readout (LUFS) | Consistent levels across episodes |
| **Project templates**: intro, outro, lower thirds, end screen ready to fill | Same structure every week |
| **Bin organization**: folders, tags, star ratings, notes and search for media | Find the right take among hundreds |

**Done in 5.1 so far:** custom shortcuts with presets · J/K/L shuttle · In/Out with loop and range export · markers with notes and YouTube chapters · ripple delete, jump to edits · track names, solo, clip color labels · automatic backups with restore · relink missing media · archive project · audio meters · **one asset system everywhere** (stickers, sound effects, music, effects, filters, transitions, text templates, text effects, fonts): search, favorites, own folders, and every category or folder can be moved, renamed or hidden.

### 5.2 Story and multi-speaker production

| Feature | Why |
|---|---|
| **Multi-POV sync**: drop several recordings of the same scene, Cutline aligns them by audio | Every player or camera recorded the scene – no manual syncing |
| **Multicam editing**: switch angles live while playing (1–9), then fine-tune the cuts | Cut dialogue scenes between viewpoints in real time |
| **Speaker tracks**: import per-speaker recordings from voice calls (one file per person), name and color each speaker, align to the scene | Clean voices, separate volume per person |
| **Text-based editing**: edit the video by editing its transcript – delete words, reorder sentences | Cut dialogue like a document |
| **Script import** (plain text, Fountain): match lines to the recordings, show which lines are covered, jump to the best take | Scripted episodes come together from the script |
| **Speaker-aware captions**: captions colored and labeled per speaker, automatic speaker detection | Readable dialogue captions |
| **Dialogue boxes**: text box with portrait, name and typewriter animation, driven by captions or script lines | Classic story-game look in one click |
| **Character cards**: name, color, portrait per character; reused by name tags, dialogue boxes and captions | One place to define the cast |
| **Name tags / lower thirds** that follow a character (uses motion tracking) | Introduce characters without manual keyframes |
| **Scene list**: episode outline from markers or scenes, drag to reorder whole scenes | Restructure an episode quickly |
| **"Previously on" builder**: pick marked moments from earlier episodes, auto-cut a recap with music | Recaps in minutes |
| **Cinematic tools**: letterbox ratios, camera shake presets, flashback and dream looks, day/night grades, slow push-ins | Story mood without plugins |
| **Sound design helpers**: ambience beds that loop seamlessly, footstep and impact packs, room-tone fill for gaps | Fuller sound with little effort |

### 5.3 Creator tools (moved from phase 4)

| Feature | Why |
|---|---|
| **Jump cuts**: remove silences (threshold, min. pause, padding) | Cuts 20–40 % dead air from talking parts |
| **Filler words**: find and cut "äh/ähm/uh/um" from the transcript | Cleaner speech |
| **Highlight finder**: loud or busy moments become markers or clips | Turns hours of footage into a shortlist |
| **YouTube chapters** from markers (`00:00 Intro`), copy to clipboard | Ready for the description |
| **Multi-track audio** from OBS recordings: each stream on its own track | Mix game, mic and call separately |
| **Animated word captions**: word-by-word highlight, pop and bounce styles | Short-form standard |
| **Vertical layouts** for 9:16: facecam over gameplay, split, blurred fill | Shorts from widescreen recordings |
| **Export presets** (Shorts, TikTok, Reels, YouTube 1080p/1440p/4K) and **multi-export** (16:9 and 9:16 in one go) | One click per platform |
| **Shorts from an episode**: pick highlights, auto reframe to 9:16, add captions, export as a batch | Promotion clips for every episode |
| **Zoom punch-ins** in one click, auto zoom on beats | Energy in talking parts |
| **Thumbnail maker**: frame or cut-out person + text styles + outline/glow → 1280×720 PNG, A/B variants | Thumbnails without another app |
| **Auto ducking**: music gets quieter under speech | Clear voices, no manual keyframes |
| **Creator overlays**: subscribe/follow animations, progress bar, countdown, webcam frames | Standard elements, own design |

### 5.4 Plugins (shipped, off by default, need your own account where noted)

| Plugin | What it does |
|---|---|
| **Freesound** (account) | Search and add CC sound effects inside the Audio tab, with license info |
| **Pixabay / Pexels** (API key) | Stock video, images and music inside the Media tab |
| **Google Fonts** | Browse and install open fonts for one project or all |
| **Voice-call recordings** | Import a per-speaker recording archive (one track per person) and create speaker tracks |
| **OBS** (local websocket) | Set markers while recording, import recordings with their markers and audio streams |
| **YouTube upload** (account) | Upload with title, description, chapters, tags, thumbnail and schedule |
| **Local text-to-speech** (Piper, open voices) | Scratch narration and placeholder lines while the real voice is not recorded yet |
| **Character portraits** | Render portraits from images or supported game skin files for character cards |
| **Asset pack registry** | Install community packs (sound effects, overlays, LUTs, dialogue box styles) |

### 5.5 Where Cutline goes further than typical free editors

- **No account, no paywall, no watermark, no upload.** Every feature, including captions and
  background removal, runs on your computer.
- **Open project format** (JSON) plus FCPXML import; projects stay yours.
- **Real font import** and a library you control (own stickers, sounds, presets).
- **Scriptable**: MCP for assistants and scripts, plugins for services.
- **Pro features without the pro price**: proxies, multicam, text-based editing, transparent
  export, color spaces, loudness targets.
- **Honest UI**: no upsell, shortcuts in every tooltip, translations checked by humans.

### 5.6 Later

Retouch (skin smoothing), HDR export, body effects, optical-flow retiming per clip, collaboration
(shared project packages with comments), Linux/macOS installers.
