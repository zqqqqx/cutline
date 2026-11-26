//! Proxy files: small, quickly decodable copies of large videos (above 1080p, above 30 fps
//! or HEVC/AV1/ProRes) for a smooth preview. The export always uses the originals.
//! Location: `<data>/cache/proxies/<id>-<height>[-<frame rate>fps].mp4` (id from path, size
//! and modification time; without frame rate = source frame rate up to 60, as older proxies were named).
//!
//! Built in parts (see [`parts`]): each finished part is usable at once, finished
//! parts survive restarts, at the end they become the usual single-file proxy.
//!
//! The build does not disturb the player: priority below normal, paused shortly after each single-frame
//! job (click, scrubbing) and during playback as soon as playback falls behind;
//! hardware encoders first.

pub(crate) mod parts;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use cl_core::{Clip, ClipContent, MediaId, MediaKind, MediaRef, Rational, Time};
use cl_export::{available_encoders, Encoder};
use eframe::egui::{self, RichText};

pub use parts::Pick;
use parts::{Focus, Parting, MAX_FAILS, PART_SECS};

use crate::app::App;
use crate::paths;
use crate::tasks::TaskDone;
use crate::theme::{self, icon};
use crate::widgets;

pub fn dir() -> PathBuf {
    paths::data_dir().join("cache").join("proxies")
}

/// Proxy never larger than the original
fn proxy_height(m: &MediaRef, height: u32) -> u32 {
    height.min(m.height.max(2)) & !1
}

/// Id of the source (changes when the file changes).
fn media_hash(m: &MediaRef) -> u64 {
    let meta = std::fs::metadata(&m.path).ok();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.path.hash(&mut h);
    meta.as_ref().map(|m| m.len()).hash(&mut h);
    meta.and_then(|m| m.modified().ok()).hash(&mut h);
    h.finish()
}

/// What a proxy should be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProxySpec {
    pub height: u32,
    /// Project frame rate if the source is faster; `None` = like the source (up to 60)
    pub fps: Option<Rational>,
    /// Keyframe interval in frames (at most 0.5 s: fast seeking)
    pub gop: u32,
}

/// Proxy for a medium at the chosen height and the project frame rate: a 60 fps source
/// in a 30 fps project is stored at 30 fps (half as much to decode).
pub fn proxy_spec(m: &MediaRef, height: u32, project_fps: Rational) -> ProxySpec {
    let src = m.fps.filter(|f| f.num > 0 && f.den > 0).map_or(30.0, |f| f.as_f64());
    let fps = (project_fps.num > 0 && project_fps.den > 0 && src > project_fps.as_f64() + 0.01)
        .then_some(project_fps);
    let out = fps.map_or(src.min(60.0), |f| f.as_f64());
    ProxySpec {
        height: proxy_height(m, height),
        fps,
        gop: ((out / 2.0).round() as u32).max(1),
    }
}

fn file_name(hash: u64, height: u32, fps: Option<Rational>) -> String {
    match fps {
        Some(f) => format!("{hash:016x}-{height}-{}_{}fps.mp4", f.num, f.den),
        None => format!("{hash:016x}-{height}.mp4"),
    }
}

/// File names usable as proxy, in order of preference: wanted height, then larger
/// (an existing 720p proxy is enough for the 540p setting too – no rebuild); per height first
/// in project frame rate, then in source frame rate (older proxies too).
fn candidates(m: &MediaRef, height: u32, project_fps: Rational, hash: u64) -> Vec<(String, u32)> {
    let spec = proxy_spec(m, height, project_fps);
    let mut hs = vec![spec.height];
    for h in PROXY_HEIGHTS.iter().map(|p| p.0).filter(|h| *h > height) {
        let h = proxy_height(m, h);
        if !hs.contains(&h) {
            hs.push(h);
        }
    }
    hs.into_iter()
        .flat_map(|h| {
            spec.fps
                .map(|f| (file_name(hash, h, Some(f)), h))
                .into_iter()
                .chain([(file_name(hash, h, None), h)])
        })
        .collect()
}

/// Path where a new proxy is created.
pub fn proxy_path(m: &MediaRef, height: u32, project_fps: Rational) -> PathBuf {
    proxy_path_in(&dir(), m, height, project_fps)
}

fn proxy_path_in(d: &Path, m: &MediaRef, height: u32, project_fps: Rational) -> PathBuf {
    let s = proxy_spec(m, height, project_fps);
    d.join(file_name(media_hash(m), s.height, s.fps))
}

/// Existing usable proxy with its height.
pub fn existing_proxy(m: &MediaRef, height: u32, project_fps: Rational) -> Option<(PathBuf, u32)> {
    existing_proxy_in(&dir(), m, height, project_fps)
}

fn existing_proxy_in(d: &Path, m: &MediaRef, height: u32, project_fps: Rational) -> Option<(PathBuf, u32)> {
    candidates(m, height, project_fps, media_hash(m))
        .into_iter()
        .map(|(n, h)| (d.join(n), h))
        .find(|(p, _)| p.is_file())
}

/// Is a proxy worth it?
pub fn wants_proxy(m: &MediaRef) -> bool {
    // proxies are H.264 without alpha: a transparent video always plays from the original
    if m.kind != MediaKind::Video || m.has_alpha {
        return false;
    }
    let fps = m.fps.map(|f| f.as_f64()).unwrap_or(30.0);
    let codec = m.video_codec.as_deref().unwrap_or("").to_ascii_lowercase();
    m.height > 1080
        || m.width > 1920
        || fps > 31.0
        || ["hevc", "av1", "prores", "vp9"]
            .iter()
            .any(|c| codec.contains(c))
}

/// Selectable proxy heights (Settings → Performance)
pub const PROXY_HEIGHTS: [(u32, &str); 3] = [(540, "Schnell · 540p"), (720, "Ausgewogen · 720p"), (1080, "Hoch · 1080p")];

// ------------------------------------------------------------------ Encoders

/// Order of the encoders as in the export ([`cl_export::encoder_chain`], `pref` = setting
/// „Hardware-Kodierung“) without those that failed in this session, software always last.
pub fn encoder_chain(available: &[Encoder], broken: &[Encoder], pref: &str) -> Vec<Encoder> {
    cl_export::encoder_chain(available, pref)
        .into_iter()
        .filter(|e| *e == Encoder::Software || !broken.contains(e))
        .collect()
}

static ENCODER_PREF: Mutex<String> = Mutex::new(String::new());

/// Setting „Hardware-Kodierung“ (set by the settings on load and change)
pub fn set_encoder_pref(pref: &str) {
    *ENCODER_PREF.lock().unwrap_or_else(|e| e.into_inner()) = pref.to_string();
}

pub fn encoder_pref() -> String {
    ENCODER_PREF.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Found encoders without waiting: `None` while detection runs in the background.
pub fn found_encoders() -> Option<&'static [Encoder]> {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(available_encoders);
    });
    cl_export::encoders_known()
}

/// Encoders that failed during a proxy build in this session
fn broken_encoders() -> std::sync::MutexGuard<'static, Vec<Encoder>> {
    static B: Mutex<Vec<Encoder>> = Mutex::new(Vec::new());
    B.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hardware decoding of the source during the proxy build: only with NVENC and only for sources whose
/// software decoding is slow (above 1080p, HEVC/AV1/VP9). Measured: 1080p H.264 is faster in software
/// (17× instead of 10× real time), 4K HEVC 10-bit with CUDA 2.8× instead of 2.2×.
fn hw_decode(enc: Encoder, m: &MediaRef) -> Option<&'static str> {
    let codec = m.video_codec.as_deref().unwrap_or("").to_ascii_lowercase();
    let heavy = m.height > 1080 || m.width > 1920 || ["hevc", "av1", "vp9"].iter().any(|c| codec.contains(c));
    (enc == Encoder::Nvenc && heavy).then_some("cuda")
}

/// Per encoder: first with hardware decoding (if sensible), then without.
fn decode_attempts(enc: Encoder, m: &MediaRef) -> Vec<Option<&'static str>> {
    match hw_decode(enc, m) {
        Some(h) => vec![Some(h), None],
        None => vec![None],
    }
}

/// Encoder arguments (quality: 540p ≈ 17 MB per minute, target ≤ 25)
fn codec_args(enc: Encoder) -> &'static [&'static str] {
    match enc {
        Encoder::Nvenc => &["-c:v", "h264_nvenc", "-preset", "p4", "-rc", "constqp", "-qp", "26"],
        Encoder::Qsv => &["-c:v", "h264_qsv", "-preset", "veryfast", "-global_quality", "26"],
        Encoder::Amf => &["-c:v", "h264_amf", "-quality", "speed", "-rc", "cqp", "-qp_i", "26", "-qp_p", "26"],
        // bitrate by size: see `part_args`
        Encoder::Mf => &["-c:v", "h264_mf", "-pix_fmt", "nv12", "-hw_encoding", "1"],
        Encoder::Software => &["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"],
    }
}

/// Frame rate of the proxy: project frame rate, otherwise the source's (at most 60, unknown 30)
fn out_fps(m: &MediaRef, spec: &ProxySpec) -> Rational {
    spec.fps.unwrap_or_else(|| match m.fps.filter(|f| f.num > 0 && f.den > 0) {
        Some(f) if f.as_f64() <= 60.01 => f,
        Some(_) => Rational::new(60, 1),
        None => Rational::new(30, 1),
    })
}

/// FFmpeg arguments for part `k` (input and output without target file). `st` = start time of the
/// source's video stream: with `-copyts` each part sees the same timestamps as a build in
/// one go and computes on the same frame grid (no duplicate/missing frame at the seam).
fn part_args(src: &Path, st: f64, pt: &Parting, k: u32, height: u32, gop: u32, enc: Encoder, hw: Option<&str>) -> Vec<String> {
    let s = pt.start(k);
    let mut a: Vec<String> = Vec::new();
    if let Some(h) = hw {
        a.extend(["-hwaccel".into(), h.into()]);
    }
    // read from the keyframe a bit before the start: the frame at the start can lie before it
    a.extend(["-noaccurate_seek".into(), "-ss".into(), format!("{:.6}", (s - 0.5).max(0.0)), "-copyts".into(), "-i".into()]);
    a.push(src.to_string_lossy().into_owned());
    let vf = format!(
        "fps={}/{}:start_time={:.6},setpts=PTS-STARTPTS,scale=-2:{height}:flags=bicubic,format=yuv420p",
        pt.fps.num,
        pt.fps.den,
        st + s
    );
    a.extend(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &vf].map(String::from));
    a.extend(codec_args(enc).iter().map(|s| s.to_string()));
    if enc == Encoder::Mf {
        // without reliable constant quality: bitrate, 540p30 ≈ 2.5 Mbit/s (≈ 17 MB per minute)
        a.extend(["-b:v".into(), format!("{}k", (height as f64 * height as f64 * pt.fps.as_f64() / 3500.0).round() as u32)]);
    }
    // without B-frames: parts of different encoders (fallback) otherwise have different lead times,
    // joined they produced a duplicate frame at the seam; faster for seeking anyway
    a.extend(["-bf".into(), "0".into(), "-g".into(), gop.to_string(), "-frames:v".into(), pt.per.to_string()]);
    // FFmpeg 7 forgets the frame rate after `setpts`: without `-r` the last frame of each part would have
    // duration 0 and be dropped when joining
    a.extend(["-r".into(), format!("{}/{}", pt.fps.num, pt.fps.den)]);
    a
}

// ------------------------------------------------------------------ Consideration for the player

static PLAYING: AtomicBool = AtomicBool::new(false);
/// Playback fell behind in this play session (holds until stop)
static BEHIND: AtomicBool = AtomicBool::new(false);
/// Paused by the user (task display): the running build pauses, no new one begins
static USER_PAUSED: AtomicBool = AtomicBool::new(false);
static LAST_ACTIVITY: Mutex<Option<Instant>> = Mutex::new(None);

/// Playback is running (background work such as object tracking pauses meanwhile).
pub fn playing() -> bool {
    PLAYING.load(Ordering::Relaxed)
}

/// The player has just requested or received a single frame (click, scrubbing).
pub fn note_activity() {
    *LAST_ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

/// Playback state (from the UI thread each frame): is it running, and does the core currently report
/// lag (automatic quality lowered or turned off = unknown)?
pub fn set_play_state(playing: bool, behind_now: bool) {
    PLAYING.store(playing, Ordering::Relaxed);
    let prev = BEHIND.load(Ordering::Relaxed);
    BEHIND.store(latch_behind(prev, playing, behind_now), Ordering::Relaxed);
}

/// Remember lag until playback ends: otherwise the build would resume after the quality recovered,
/// slow things down again – a back and forth at the pace of the automatic quality.
pub fn latch_behind(prev: bool, playing: bool, behind_now: bool) -> bool {
    playing && (prev || behind_now)
}

fn idle() -> Duration {
    LAST_ACTIVITY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map_or(Duration::MAX, |t| t.elapsed())
}

/// Pause right after a single-frame job …
const PAUSE_WITHIN: Duration = Duration::from_millis(300);
/// … and resume only after a rest, so fast clicking does not switch constantly.
const RESUME_AFTER: Duration = Duration::from_millis(800);

/// Should the proxy build be paused (now)? During playback it keeps running (one part encoder,
/// priority below normal), unless playback falls behind (`behind`); when stopped each
/// click pauses it briefly.
pub fn want_pause(paused: bool, playing: bool, behind: bool, idle: Duration) -> bool {
    if playing {
        return behind;
    }
    idle < if paused { RESUME_AFTER } else { PAUSE_WITHIN }
}

fn pause_now(paused: bool) -> bool {
    USER_PAUSED.load(Ordering::Relaxed) || want_pause(paused, PLAYING.load(Ordering::Relaxed), BEHIND.load(Ordering::Relaxed), idle())
}

// ------------------------------------------------------------------ Build

/// Result of a proxy build (for the measurement point).
#[derive(Debug, Clone, Copy)]
pub struct BuildReport {
    /// Run time without pauses
    pub secs: f64,
    /// paused (playback, clicks)
    pub paused: f64,
}

/// Create a proxy: encoders in order (see [`encoder_chain`]), per encoder first with, then
/// without hardware decoding. An encoder that fails is avoided for the session.
pub fn build(
    m: &MediaRef,
    out: &Path,
    spec: &ProxySpec,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<BuildReport, String> {
    let chain = encoder_chain(available_encoders(), &broken_encoders(), &encoder_pref());
    let mut last = String::new();
    for enc in chain {
        for hw in decode_attempts(enc, m) {
            match build_with(m, out, spec, enc, hw, progress, cancel) {
                Ok(r) => {
                    // build in one go (duration unknown): otherwise it would never count as finished
                    with_mine(&m.path, out, Entry::built);
                    return Ok(r);
                }
                Err(e) if e == "Abgebrochen" => return Err(e),
                Err(e) => last = e,
            }
        }
        if enc != Encoder::Software {
            broken_encoders().push(enc);
        }
    }
    Err(last)
}

#[allow(clippy::too_many_arguments)]
fn build_with(
    m: &MediaRef,
    out: &Path,
    spec: &ProxySpec,
    enc: Encoder,
    hw: Option<&str>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<BuildReport, String> {
    std::fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    let tmp = parts::tmp_of(out);
    let mut cmd = encode_command()?;
    // the same way as one part over everything: same frame grid, same start offset as the original
    let whole = Parting { fps: out_fps(m, spec), per: u32::MAX, count: 1 };
    cmd.args(part_args(&m.path, probe_start(&m.path), &whole, 0, spec.height, spec.gop, enc, hw));
    let dur = m.duration.as_secs_f64().max(0.1);
    let (report, n) = run_encode(cmd, &tmp, out, dur, progress, cancel)?;
    if n == 0 {
        return Err("Proxy konnte nicht erstellt werden".into());
    }
    if crate::app::perf() {
        let mb = std::fs::metadata(out).map_or(0.0, |x| x.len() as f64 / 1e6);
        crate::applog::perf_line!(
            "perf: proxy {} enc={:?}{} {:.1}× echtzeit {:.1} MB/min pausiert {:.1} s",
            m.path.file_name().unwrap_or_default().to_string_lossy(),
            enc,
            hw.map(|h| format!("+{h}")).unwrap_or_default(),
            dur / report.secs,
            mb / (dur / 60.0),
            report.paused
        );
    }
    Ok(report)
}

/// FFmpeg for the proxy build: priority below normal, progress on stdout
fn encode_command() -> Result<Command, String> {
    let mut cmd = cl_media::ffmpeg_command().map_err(|e| e.to_string())?;
    cl_media::lower_priority(&mut cmd);
    cmd.args(["-v", "error", "-y", "-nostdin", "-progress", "pipe:1", "-nostats"]);
    Ok(cmd)
}

/// Let FFmpeg write to `tmp` and rename to `out` on success (a finished file
/// is never half). Pauses the process while the player has priority ([`pause_now`]).
/// Result: run time and number of frames written.
fn run_encode(
    mut cmd: Command,
    tmp: &Path,
    out: &Path,
    dur: f64,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(BuildReport, u64), String> {
    let mut child = cmd
        .arg(tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // unread, the full buffer would stall FFmpeg
        .spawn()
        .map_err(|e| e.to_string())?;
    // read progress in its own thread: when paused FFmpeg writes nothing, the control
    // below must still keep running (resume, cancel).
    let done_us = Arc::new(AtomicU64::new(0));
    let frames = Arc::new(AtomicU64::new(0));
    let reader = child.stdout.take().map(|out| {
        let (done_us, frames) = (done_us.clone(), frames.clone());
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<u64>().ok()) {
                    done_us.store(us, Ordering::Relaxed);
                } else if let Some(n) = line.strip_prefix("frame=").and_then(|v| v.trim().parse::<u64>().ok()) {
                    frames.store(n, Ordering::Relaxed);
                }
            }
        })
    });
    let dur = dur.max(0.1);
    let start = Instant::now();
    let (mut paused, mut paused_total, mut paused_since) = (false, Duration::ZERO, None::<Instant>);
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            // wait until FFmpeg has released the file (Windows), otherwise `.part.mp4` stays
            let _ = child.wait();
            let _ = std::fs::remove_file(tmp);
            return Err("Abgebrochen".into());
        }
        let want = pause_now(paused);
        if want != paused && cl_media::suspend(&child, want) {
            paused = want;
            if want {
                paused_since = Some(Instant::now());
            } else if let Some(t) = paused_since.take() {
                paused_total += t.elapsed();
            }
        }
        progress((done_us.load(Ordering::Relaxed) as f64 / 1e6 / dur).clamp(0.0, 1.0) as f32);
        match child.try_wait() {
            // cancelled in this very round (e.g. from `progress`): the cancel wins, also when
            // FFmpeg has just finished – the check at the top of the loop handles it
            Ok(Some(_)) if cancel.load(Ordering::Relaxed) => continue,
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(tmp);
                return Err(e.to_string());
            }
        }
    };
    if let Some(r) = reader {
        let _ = r.join();
    }
    if !status.success() {
        let _ = std::fs::remove_file(tmp);
        return Err("Proxy konnte nicht erstellt werden".into());
    }
    let paused = (paused_total + paused_since.map_or(Duration::ZERO, |t| t.elapsed())).as_secs_f64();
    let report = BuildReport {
        secs: (start.elapsed().as_secs_f64() - paused).max(1e-3),
        paused,
    };
    let n = frames.load(Ordering::Relaxed);
    if n == 0 {
        // no frame written: rename nothing, the caller decides
        let _ = std::fs::remove_file(tmp);
        return Ok((report, 0));
    }
    std::fs::rename(tmp, out).map_err(|e| e.to_string())?;
    Ok((report, n))
}

/// Start time of the file (earliest stream; sources with offset, e.g. from cameras or cut
/// files). Not that of the video track: `-ss` on the original counts from the file start; if the picture begins
/// later than the sound, the `fps` filter pads the front with the first frame – like the original.
fn probe_start(src: &Path) -> f64 {
    cl_media::ffprobe_command()
        .ok()
        .and_then(|mut c| {
            c.args(["-v", "error", "-show_entries", "format=start_time", "-of", "csv=p=0"])
                .arg(src)
                .stdin(Stdio::null())
                .output()
                .ok()
        })
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// Build part `k` (encoder chain as in [`build`]). An encoder only counts as broken when
/// another one manages the same part – otherwise it is the source. Result: number of frames
/// (0: the video ends before this part, the file stays empty).
#[allow(clippy::too_many_arguments)]
fn build_part(
    m: &MediaRef,
    dir: &Path,
    pt: &Parting,
    st: f64,
    k: u32,
    spec: &ProxySpec,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<u64, String> {
    let mut failed = Vec::new();
    let mut last = String::new();
    // release first: the list is locked again in the loop
    let chain = encoder_chain(available_encoders(), &broken_encoders(), &encoder_pref());
    for enc in chain {
        for hw in decode_attempts(enc, m) {
            match build_part_with(m, dir, pt, st, k, spec, enc, hw, progress, cancel) {
                Ok(n) => {
                    broken_encoders().extend(failed.iter().filter(|e| **e != Encoder::Software));
                    return Ok(n);
                }
                Err(e) if e == "Abgebrochen" => return Err(e),
                Err(e) => last = e,
            }
        }
        failed.push(enc);
    }
    Err(last)
}

#[allow(clippy::too_many_arguments)]
fn build_part_with(
    m: &MediaRef,
    dir: &Path,
    pt: &Parting,
    st: f64,
    k: u32,
    spec: &ProxySpec,
    enc: Encoder,
    hw: Option<&str>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<u64, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let out = dir.join(parts::part_name(k));
    let tmp = parts::tmp_of(&out);
    let mut cmd = encode_command()?;
    cmd.args(part_args(&m.path, st, pt, k, spec.height, spec.gop, enc, hw));
    let secs = pt.start(k + 1) - pt.start(k);
    let (_, n) = run_encode(cmd, &tmp, &out, secs, progress, cancel)?;
    if n == 0 {
        // past the end of the video (sound longer than picture): empty part
        std::fs::write(&out, b"").map_err(|e| e.to_string())?;
    }
    Ok(n)
}

/// Join all parts losslessly into the single-file proxy `out` (finish first, then rename).
fn merge(dir: &Path, pt: &Parting, out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let list = parts::concat_list(dir, 0, pt.count).map_err(|e| e.to_string())?;
    let tmp = parts::tmp_of(out);
    let mut cmd = encode_command()?;
    cmd.args(["-f", "concat", "-i"]).arg(&list).args(["-map", "0:v:0", "-c", "copy", "-movflags", "+faststart"]);
    let dur = pt.start(pt.count);
    match run_encode(cmd, &tmp, out, dur, &|_| {}, cancel)? {
        (_, 0) => Err("Proxy-Stücke ohne Bild".into()),
        _ => Ok(()),
    }
}

/// Order of the proxy jobs: media at the playhead first, then in the visible area of the
/// timeline, then the rest (in media order).
pub fn next_proxy(candidates: &[MediaId], at_playhead: &HashSet<MediaId>, visible: &HashSet<MediaId>) -> Option<MediaId> {
    candidates
        .iter()
        .min_by_key(|id| if at_playhead.contains(id) { 0 } else if visible.contains(id) { 1 } else { 2 })
        .copied()
}

/// Result of a search for proxies (background): per video the existing proxy, plus the
/// media for which one should be built (or parts are missing)
type Scan = (Vec<(MediaId, Option<(PathBuf, u32)>)>, Vec<MediaId>);

/// Proxy management without file system in the UI thread: search for existing/missing proxies in the
/// background, result in the cache; counters for the overall display.
#[derive(Default)]
pub struct ProxyState {
    /// existing proxy per medium (valid for `key`)
    known: HashMap<MediaId, Option<(PathBuf, u32)>>,
    /// Setting for which `known` holds: (height, project frame rate)
    key: Option<(u32, Rational)>,
    scan: Option<std::sync::mpsc::Receiver<Scan>>,
    /// last search: time, number of media, project revision
    scanned: Option<(Instant, usize, u64)>,
    /// last search found nothing to build
    idle: bool,
    /// built / total in this round (display „Proxys 3 / 40“)
    pub done: usize,
    pub total: usize,
    /// cancelled by the user: no more in this project
    pub stopped: bool,
}

impl ProxyState {
    /// Other project: counters and search anew; existing proxies stay valid (id from the file)
    pub fn reset(&mut self) {
        *self = Self { key: self.key, known: std::mem::take(&mut self.known), ..Default::default() };
    }

    /// Existing proxy from the cache (`None` = unknown, `Some(None)` = none)
    pub fn cached(&self, m: MediaId, key: (u32, Rational)) -> Option<Option<(PathBuf, u32)>> {
        (self.key == Some(key)).then(|| self.known.get(&m).cloned()).flatten()
    }

    pub fn paused() -> bool {
        USER_PAUSED.load(Ordering::Relaxed)
    }

    pub fn set_paused(on: bool) {
        USER_PAUSED.store(on, Ordering::Relaxed);
    }
}

/// New search due? At once when the number of media changes; after 2 s while something is missing
/// or the project changed (e.g. relinked file); otherwise (nothing to do) only every 30 s.
pub fn scan_due(scanned: Option<(Instant, usize, u64)>, media: usize, rev: u64, idle: bool) -> bool {
    scanned.is_none_or(|(t, n, r)| {
        let every = if idle && r == rev { 30 } else { 2 };
        n != media || t.elapsed() >= Duration::from_secs(every)
    })
}

/// Start the next proxy job? Only if something is missing, none is running and it is not paused
/// (otherwise each job would end by yielding priority and the next would begin despite the pause).
pub fn start_next(missing: usize, running: bool, paused: bool) -> bool {
    missing > 0 && !running && !paused
}

/// New total of the round: finished plus missing; nothing missing → round over.
pub fn round(done: usize, missing: usize) -> (usize, usize) {
    if missing == 0 {
        (0, 0)
    } else {
        (done, done + missing)
    }
}

// ------------------------------------------------------------------ Part inventory

/// State of a source's proxy: written by the build (background), read by the player and the timeline
/// (without file system per frame).
struct Entry {
    /// chosen proxy height and project frame rate the entry applies to
    want: (u32, Rational),
    /// Target: the finished single-file proxy (the parts folder next to it)
    out: PathBuf,
    spec: ProxySpec,
    /// `None`: duration unknown, build in one go
    pt: Option<Parting>,
    dur: f64,
    ready: Vec<bool>,
    fails: Vec<u8>,
    /// finished proxy present (also an older single-file proxy) …
    complete: bool,
    /// … and its file (stream without file access in the UI thread)
    file: Option<PathBuf>,
    /// Build failed or cancelled: not again in this session
    dead: bool,
    /// finished source ranges (timeline), recomputed only on changes
    ranges: Arc<Vec<(f64, f64)>>,
}

impl Entry {
    fn changed(&mut self) {
        self.ranges = Arc::new(match self.pt {
            _ if self.complete => vec![(0.0, self.dur.max(0.0))],
            Some(pt) => parts::ready_ranges(&pt, &self.ready, self.dur),
            None => Vec::new(),
        });
    }

    /// Own build is finished: the file lies at `out`
    fn built(&mut self) {
        self.complete = true;
        self.file = Some(self.out.clone());
        self.changed();
    }

    /// Is the part at source time `t` missing (without division: the whole proxy) and can it be built?
    fn needs(&self, t: f64) -> bool {
        !self.complete
            && !self.dead
            && self.pt.is_none_or(|pt| {
                let k = pt.part_of(t) as usize;
                !self.ready.get(k).copied().unwrap_or(true) && self.fails.get(k).is_some_and(|f| *f < MAX_FAILS)
            })
    }
}

fn registry() -> MutexGuard<'static, HashMap<PathBuf, Entry>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
    R.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Where the work should go, per source (set by the UI thread, see [`App::update_focus`])
fn focus() -> MutexGuard<'static, HashMap<PathBuf, Focus>> {
    static F: OnceLock<Mutex<HashMap<PathBuf, Focus>>> = OnceLock::new();
    F.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Change the entry of the running build (only if it still belongs to this target)
fn with_mine<R>(src: &Path, out: &Path, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
    registry().get_mut(src).filter(|e| e.out == out).map(f)
}

/// Bring the entry for `m` up to date (every 2 s): detect a finished proxy, after a restart or
/// changed source read the parts from disk. `true`: something is still missing.
/// Runs in the background search; file system only outside the lock (player and timeline
/// read the inventory each frame). Result: existing proxy, and whether something is left to build.
fn refresh_entry(root: &Path, m: &MediaRef, h: u32, fps: Rational, playing: bool) -> (Option<(PathBuf, u32)>, bool) {
    let found = existing_proxy_in(root, m, h, fps);
    let full = found.is_some();
    let out = proxy_path_in(root, m, h, fps);
    if registry().get(&m.path).is_none_or(|e| e.want != (h, fps) || e.out != out) {
        let spec = proxy_spec(m, h, fps);
        let dur = m.duration.as_secs_f64();
        let pt = Parting::new(dur, out_fps(m, &spec), PART_SECS);
        let pdir = parts::parts_dir(&out);
        let ready = pt.map_or_else(Vec::new, |pt| parts::scan_ready(&pdir, pt.count));
        parts::remove_stale(root, &m.path, &pdir);
        let fails = vec![0; ready.len()];
        let file = found.as_ref().map(|f| f.0.clone());
        let mut e = Entry { want: (h, fps), out, spec, pt, dur, ready, fails, complete: full, file, dead: false, ranges: Arc::default() };
        e.changed();
        registry().insert(m.path.clone(), e);
    }
    let (needs, pdir) = {
        let mut reg = registry();
        let Some(e) = reg.get_mut(&m.path) else { return (found, false) };
        // only upwards: the build may have finished right after the search
        if let Some(f) = found.as_ref().filter(|_| !e.complete) {
            e.complete = true;
            e.file = Some(f.0.clone());
            e.changed();
        }
        (!e.complete && !e.dead, parts::parts_dir(&e.out))
    };
    // joined: parts gone (not during playback – a running stream still reads them)
    if full && !playing && pdir.is_dir() {
        let _ = std::fs::remove_dir_all(&pdir);
    }
    (found, needs)
}

/// Sources at the playhead that lack a part there. The same rule for choosing the next build
/// and for yielding a running one – otherwise the search picks the yielding medium again right away.
fn playhead_wants() -> HashSet<PathBuf> {
    // locks one after another, never both at once
    let at: Vec<(PathBuf, f64)> = focus().iter().filter_map(|(p, f)| f.at.map(|a| (p.clone(), a.0))).collect();
    let reg = registry();
    at.into_iter().filter(|(p, t)| reg.get(p).is_some_and(|e| e.needs(*t))).map(|(p, _)| p).collect()
}

/// Does another medium at the playhead have priority (the part is missing there, not here)?
fn other_media_first(src: &Path) -> bool {
    let w = playhead_wants();
    !w.contains(src) && !w.is_empty()
}

/// This many part attempts may fail in a row, then the build ends (disk full, source
/// gone) – otherwise each further part would try the whole encoder chain, thousands of starts for 2 h.
const MAX_FAILS_IN_ROW: u32 = 3;

/// Failed attempts in a row after a result; `None`: give up
fn fails_in_row(prev: u32, ok: bool) -> Option<u32> {
    if ok {
        Some(0)
    } else {
        Some(prev + 1).filter(|n| *n < MAX_FAILS_IN_ROW)
    }
}

/// Build parts of a source until all are there (then join) or another medium at the
/// playhead has priority. Replanned after each part (playhead jumped: continue there; the
/// running part is finished). A part that fails repeatedly is skipped – the original
/// stays there. `Ok(true)`: proxy finished; `Ok(false)`: interrupted (priority, stale).
fn build_parts(m: &MediaRef, out: &Path, spec: &ProxySpec, pt: &Parting, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<bool, String> {
    let pdir = parts::parts_dir(out);
    std::fs::create_dir_all(&pdir).map_err(|e| e.to_string())?;
    let _ = std::fs::write(pdir.join("quelle.txt"), m.path.to_string_lossy().as_bytes());
    let st = probe_start(&m.path);
    let name = m.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let (mut paused, mut in_row) = (false, 0);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen".into());
        }
        if other_media_first(&m.path) {
            return Ok(false);
        }
        // consideration for the player even before a part starts
        paused = pause_now(paused);
        if paused {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        let f = focus().get(&m.path).cloned().unwrap_or_default();
        let Some((next, done)) = with_mine(&m.path, out, |e| {
            let k = parts::next_part(pt, |i| e.ready[i as usize] || e.fails[i as usize] >= MAX_FAILS, &f);
            (k, e.ready.iter().filter(|r| **r).count())
        }) else {
            return Ok(false);
        };
        let Some(k) = next else { break };
        let n = pt.count.max(1) as f32;
        let base = done as f32 / n;
        let r = build_part(m, &pdir, pt, st, k, spec, &|v| progress(base + v / n), cancel);
        in_row = fails_in_row(in_row, r.is_ok()).ok_or_else(|| format!("{MAX_FAILS_IN_ROW} Stücke nacheinander gescheitert"))?;
        match r {
            Ok(_) => {
                with_mine(&m.path, out, |e| {
                    e.ready[k as usize] = true;
                    e.changed();
                });
            }
            Err(e) if e == "Abgebrochen" => return Err(e),
            Err(e) => {
                log::warn!(target: "proxy", "Stück {k} von „{name}“ gescheitert: {e}");
                with_mine(&m.path, out, |en| en.fails[k as usize] += 1);
            }
        }
    }
    if with_mine(&m.path, out, |e| e.ready.iter().all(|r| *r)) != Some(true) {
        return Err("Proxy-Stücke fehlgeschlagen".into());
    }
    merge(&pdir, pt, out, cancel)?;
    with_mine(&m.path, out, Entry::built);
    if !PLAYING.load(Ordering::Relaxed) {
        let _ = std::fs::remove_dir_all(&pdir);
    }
    Ok(true)
}

/// Proxies in the folder (finished files and part folders) and their space in bytes
fn folder_usage(dir: &Path) -> (u64, u64) {
    let (mut n, mut bytes) = (0, 0);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            n += 1;
            bytes += std::fs::read_dir(e.path()).into_iter().flatten().flatten().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum::<u64>();
        } else {
            n += u64::from(!e.file_name().to_string_lossy().ends_with(".part.mp4"));
            bytes += md.len();
        }
    }
    (n, bytes)
}

/// Is a proxy of height `proxy_h` the right source for a frame of height `need_h`? Its lines must be
/// enough (sharp), unless sharpness does not count (`loose`: playback, moving playhead).
pub fn proxy_sharp(loose: bool, need_h: u32, proxy_h: u32) -> bool {
    loose || need_h <= proxy_h * 6 / 5
}

impl App {
    fn proxy_want(&self) -> (u32, Rational) {
        (self.settings.proxy_h, self.project.settings.fps)
    }

    /// May the preview read `m` from a proxy (or its parts)? Never for transparent videos, even if
    /// an older version left a proxy in the cache: it has no alpha.
    fn proxy_allowed(&self, m: &MediaRef) -> bool {
        self.settings.use_proxies && m.kind == MediaKind::Video && !m.has_alpha
    }

    /// File for the preview: during playback the proxy (smooth), when stopped the original if
    /// the proxy's lines are not enough for the needed height `need_h` (sharp).
    /// Existing proxies come from the cache of the background search.
    pub fn preview_path(&self, m: &MediaRef, need_h: u32) -> PathBuf {
        self.preview_path_loose(m, need_h, self.playing)
    }

    /// Like [`Self::preview_path`]; `loose`: the proxy counts as sharp enough (playback, moving playhead)
    fn preview_path_loose(&self, m: &MediaRef, need_h: u32, loose: bool) -> PathBuf {
        if self.proxy_allowed(m) {
            let key = self.proxy_want();
            let found = match self.proxies.cached(m.id, key) {
                Some(c) => c,
                None => existing_proxy(m, key.0, key.1),
            };
            if let Some((p, h)) = found {
                if proxy_sharp(loose, need_h, h) {
                    return p;
                }
            }
        }
        m.path.clone()
    }

    /// Single frame at source time `at`: (file, time in it) – from a finished part, otherwise like
    /// [`Self::preview_path`]. `moving`: the playhead is being dragged – the proxy (fast seeking) even if
    /// its lines are not enough; at rest the sharp source as before.
    pub fn still_source(&self, m: &MediaRef, need_h: u32, at: Time, moving: bool) -> (PathBuf, Time) {
        let loose = self.playing || moving;
        if self.proxy_allowed(m) {
            let want = self.proxy_want();
            let part = registry().get(&m.path).filter(|e| e.want == want && !e.complete).and_then(|e| {
                let (k, rel) = parts::still_part(&e.pt?, at.as_secs_f64())?;
                let sharp = proxy_sharp(loose, need_h, e.spec.height);
                (sharp && e.ready.get(k as usize) == Some(&true)).then(|| (parts::parts_dir(&e.out).join(parts::part_name(k)), rel))
            });
            // empty part (past the video end): original
            if let Some((p, rel)) = part.filter(|(p, _)| std::fs::metadata(p).is_ok_and(|x| x.len() > 0)) {
                return (p, Time::from_secs_f64(rel));
            }
        }
        (self.preview_path_loose(m, need_h, loose), at)
    }

    /// Source of the scrub window of a single frame of size `w`×`h` (see `preview::ScrubSpec`): the
    /// proxy or a finished part whatever its sharpness (fast seeking, 0.5-s keyframes), at most at its
    /// own size; without one the original at half the height. Never a proxy for alpha media
    /// (`proxy_allowed`). `None` for non-videos and without a frame rate.
    pub fn window_source(&self, m: &MediaRef, (w, h): (u32, u32), at: Time) -> Option<crate::preview::ScrubSpec> {
        if m.kind != MediaKind::Video {
            return None;
        }
        let (path, at) = self.still_source(m, h, at, true);
        let (fps, max_h) = if path == m.path {
            (m.fps?, h / 2)
        } else {
            let want = self.proxy_want();
            let spec = proxy_spec(m, want.0, want.1);
            (spec.fps.or(m.fps)?, spec.height)
        };
        // the still size if the proxy is that sharp (window frames are then the exact ones), otherwise
        // at most half the height: ~1 MB per frame, a window run of up to 1 s fits the memory budget
        let max_h = if h <= max_h { h } else { max_h.min(h / 2).max(2) };
        let (w, h) = if h <= max_h { (w, h) } else { cl_media::fit_size(w, h, w, max_h) };
        Some(crate::preview::ScrubSpec { path, at, fps, w, h, graph: None })
    }

    /// Source of the stream of layer `id` from timeline place `from` (rules: [`parts::choose_stream`]).
    /// Goes into the stream key: a change runs through the replacement stream of the player core.
    pub fn stream_pick(&mut self, id: u128, c: &Clip, m: &MediaRef, from: Time) -> Pick {
        if !self.proxy_allowed(m) {
            return Pick::Whole;
        }
        let want = self.proxy_want();
        let p = c.source_time(Time(from.0.clamp(c.start.0, c.end().0))).as_secs_f64();
        let mut end = (c.source_in + c.source_span()).as_secs_f64();
        if m.duration > Time::ZERO {
            end = end.min(m.duration.as_secs_f64());
        }
        let margin = parts::SWITCH_MARGIN * c.speed.max(0.01) as f64;
        let prev = self.proxy_picks.get(&id).copied();
        let pick = registry().get(&m.path).filter(|e| e.want == want).map_or(Pick::Whole, |e| match e.pt {
            _ if e.complete => Pick::Full,
            Some(pt) => parts::choose_stream(prev, &e.ready, &pt, p, end, margin),
            None => Pick::Whole,
        });
        self.proxy_picks.insert(id, pick);
        pick
    }

    /// File of a stream and source time of its start (parts begin in the middle of the source).
    pub fn stream_source(&self, m: &MediaRef, need_h: u32, pick: Pick) -> (PathBuf, Time) {
        match pick {
            Pick::Original { .. } => (m.path.clone(), Time::ZERO),
            Pick::Parts { first, end } => {
                let at = registry().get(&m.path).and_then(|e| Some((parts::parts_dir(&e.out), e.pt?.start(first))));
                at.and_then(|(d, s)| parts::concat_list(&d, first, end).ok().map(|l| (l, Time::from_secs_f64(s))))
                    .unwrap_or_else(|| (m.path.clone(), Time::ZERO))
            }
            // file from the inventory (just finished: the search may not know it yet),
            // without file access in the UI thread
            Pick::Full => (registry().get(&m.path).and_then(|e| e.file.clone()).unwrap_or_else(|| self.preview_path(m, need_h)), Time::ZERO),
            Pick::Whole => (self.preview_path(m, need_h), Time::ZERO),
        }
    }

    /// Timeline: thin line at the clip's bottom edge – which parts of its source are already available as
    /// proxy parts; hides as soon as everything is there.
    pub fn proxy_line(&self, ctx: &egui::Context, p: &egui::Painter, c: &Clip, r: egui::Rect) {
        let ClipContent::Media { media, .. } = c.content else { return };
        if !self.settings.use_proxies {
            return;
        }
        let Some(m) = self.project.media(media) else { return };
        let want = self.proxy_want();
        let Some((ranges, complete)) = registry().get(&m.path).filter(|e| e.want == want).map(|e| (e.ranges.clone(), e.complete)) else {
            return;
        };
        let a = theme::fade(ctx, egui::Id::new(("proxy-line", c.id)), !complete, theme::MOTION);
        if a <= 0.0 {
            return;
        }
        let line = egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 2.0), r.max);
        p.rect_filled(line, 0.0, theme::ACCENT().gamma_multiply(0.25 * a));
        for (x0, x1) in parts::clip_coverage(&ranges, c.source_in.as_secs_f64(), c.source_span().as_secs_f64(), c.fx.reverse) {
            let seg = egui::Rect::from_x_y_ranges(line.left() + line.width() * x0..=line.left() + line.width() * x1, line.y_range());
            p.rect_filled(seg, 0.0, theme::ACCENT().gamma_multiply(a));
        }
    }

    /// Where the build should go first (playhead, visible and used ranges per source). Only renewed
    /// when playhead, view or project change; during playback at most every 250 ms.
    fn update_focus(&self, ctx: &egui::Context) {
        static LAST: Mutex<(u64, Option<Instant>)> = Mutex::new((0, None));
        let w = ctx.screen_rect().width();
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        (self.playhead.0, self.timeline.scroll.to_bits(), self.timeline.pps.to_bits(), w.to_bits(), self.project_epoch, self.project.media.len(), self.revision).hash(&mut hs);
        // only sources that can be built: a missing (NAS asleep) or failed
        // medium at the playhead would otherwise make the running build yield again and again
        let usable = |m: &MediaRef| m.kind == MediaKind::Video && self.presence.get(&m.path) && !self.proxy_failed.contains(&m.id);
        for m in &self.project.media {
            (m.id, usable(m)).hash(&mut hs);
        }
        let sig = hs.finish();
        {
            let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if l.0 == sig || (self.playing && l.1.is_some_and(|t| t.elapsed() < Duration::from_millis(250))) {
                return;
            }
            *l = (sig, Some(Instant::now()));
        }
        let paths: HashMap<MediaId, &Path> = self.project.media.iter().filter(|m| usable(m)).map(|m| (m.id, m.path.as_path())).collect();
        let v0 = self.timeline.scroll;
        let v1 = v0 + (w / self.timeline.pps.max(1e-3)) as f64;
        let mut map: HashMap<PathBuf, Focus> = HashMap::new();
        for c in self.project.timeline.tracks.iter().flat_map(|t| &t.clips) {
            let ClipContent::Media { media, .. } = c.content else { continue };
            let Some(p) = paths.get(&media) else { continue };
            let f = map.entry(p.to_path_buf()).or_default();
            let a = c.source_in.as_secs_f64();
            let b = a + c.source_span().as_secs_f64();
            f.used.push((a, b));
            let (s, e) = (c.start.as_secs_f64(), c.end().as_secs_f64());
            if s < v1 && v0 < e {
                let x = c.source_time(Time::from_secs_f64(s.max(v0))).as_secs_f64();
                let y = c.source_time(Time::from_secs_f64(e.min(v1))).as_secs_f64();
                f.visible.push((x.min(y), x.max(y)));
            }
            if c.contains(self.playhead) {
                let t = c.source_time(self.playhead).as_secs_f64();
                f.at = Some((t, if c.fx.reverse { t } else { b }));
            }
        }
        *focus() = map;
    }

    /// Create missing proxies in the background (one after another, one task per medium; in parts).
    /// Which are missing (and which parts are already there) is searched by a background thread; here only
    /// collect the result and start the next build.
    pub fn ensure_proxies(&mut self, ctx: &egui::Context) {
        let _s = crate::app::section("Proxys");
        set_play_state(self.playing, !self.settings.auto_quality || self.core.quality.level > 0);
        // keep in the pause: otherwise Play at the same place would choose the parts anew (other first
        // part = other key) and open a new stream instead of continuing
        if (!self.core.streaming() && self.core.mode() != crate::playcore::Mode::Pause) || self.proxy_picks.len() > 512 {
            self.proxy_picks.clear();
        }
        let key = self.proxy_want();
        if self.proxies.key != Some(key) {
            self.proxies.known.clear();
            self.proxies.key = Some(key);
            self.proxies.scanned = None;
        }
        if !self.settings.use_proxies || self.proxies.stopped {
            return;
        }
        self.update_focus(ctx);
        // search also during a build: a new medium at the playhead gets its entry, and
        // the running build yields priority (see `other_media_first`)
        let res = match &self.proxies.scan {
            Some(rx) => match rx.try_recv() {
                Ok(r) => r,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.proxies.scan = None;
                    return;
                }
            },
            None => {
                let n = self.project.media.len();
                if scan_due(self.proxies.scanned, n, self.revision, self.proxies.idle) {
                    self.proxies.scanned = Some((Instant::now(), n, self.revision));
                    // all videos (the player asks for proxies too), built only where it is worth it
                    let media: Vec<(MediaRef, bool)> = self
                        .project
                        .media
                        .iter()
                        // presence counts unchecked paths as present: never hand a URL to FFmpeg
                        .filter(|m| m.kind == MediaKind::Video && self.presence.get(&m.path) && !cl_media::is_url(&m.path))
                        .map(|m| (m.clone(), wants_proxy(m) && !self.proxy_failed.contains(&m.id)))
                        .collect();
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (c, playing) = (ctx.clone(), self.playing);
                    std::thread::spawn(move || {
                        // leftovers of the last run (also after a hard exit), once per session
                        static CLEANED: std::sync::Once = std::sync::Once::new();
                        let root = dir();
                        CLEANED.call_once(|| parts::cleanup(&root));
                        let mut found = Vec::new();
                        let mut missing = Vec::new();
                        for (m, want) in &media {
                            if *want {
                                let (p, needs) = refresh_entry(&root, m, key.0, key.1, playing);
                                found.push((m.id, p));
                                if needs {
                                    missing.push(m.id);
                                }
                            } else {
                                found.push((m.id, existing_proxy_in(&root, m, key.0, key.1)));
                            }
                        }
                        let _ = tx.send((found, missing));
                        c.request_repaint();
                    });
                    self.proxies.scan = Some(rx);
                }
                return;
            }
        };
        self.proxies.scan = None;
        let (found, missing) = res;
        self.proxies.known.extend(found);
        self.proxies.idle = missing.is_empty();
        (self.proxies.done, self.proxies.total) = round(self.proxies.done, missing.len());
        if !start_next(missing.len(), self.tasks.running("proxy").is_some(), ProxyState::paused()) {
            return;
        }
        let Some(m) = self.next_build(&missing, ctx).and_then(|id| self.project.media.iter().find(|m| m.id == id)).cloned() else {
            return;
        };
        let spec = proxy_spec(&m, key.0, key.1);
        // target and division are in the inventory (determined by the background search)
        let target = registry().get(&m.path).filter(|e| e.want == key).map(|e| (e.out.clone(), e.pt));
        let id = m.id;
        let label = format!("Proxys {} / {}", self.proxies.done + 1, self.proxies.total);
        self.tasks.spawn(ctx, "proxy", label, move |p, cancel| {
            let t0 = std::time::Instant::now();
            // in „…“: the log export redacts names with spaces only there
            let name = format!("„{}“", m.path.file_name().unwrap_or_default().to_string_lossy());
            let r = match target {
                Some((out, Some(pt))) => build_parts(&m, &out, &spec, &pt, p, cancel),
                Some((out, None)) => build(&m, &out, &spec, p, cancel).map(|_| true),
                None => build(&m, &proxy_path(&m, key.0, key.1), &spec, p, cancel).map(|_| true),
            };
            if r.is_err() {
                if let Some(e) = registry().get_mut(&m.path) {
                    e.dead = true;
                }
            }
            TaskDone::Proxy { media: id, ok: crate::applog::outcome("proxy", &name, t0, r).is_ok() }
        });
    }

    /// Which missing medium next: at the playhead only where a part is missing there (as when
    /// yielding, see [`other_media_first`]), then visible ones, then the rest.
    fn next_build(&self, missing: &[MediaId], ctx: &egui::Context) -> Option<MediaId> {
        let wants = playhead_wants();
        let at: HashSet<MediaId> = self.project.media.iter().filter(|m| wants.contains(&m.path)).map(|m| m.id).collect();
        let v0 = Time::from_secs_f64(self.timeline.scroll);
        let v1 = Time::from_secs_f64(self.timeline.scroll + (ctx.screen_rect().width() / self.timeline.pps.max(1e-3)) as f64);
        let vis: HashSet<MediaId> = self
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.start < v1 && v0 < c.end())
            .filter_map(|c| match c.content {
                ClipContent::Media { media, .. } => Some(media),
                _ => None,
            })
            .collect();
        next_proxy(missing, &at, &vis)
    }

    /// A proxy job has ended: search on right away. Only what finished or
    /// failed is counted – a build that yielded priority to a medium at the playhead comes back.
    pub fn proxy_finished(&mut self, media: MediaId, ok: bool) {
        let complete = self
            .project
            .media(media)
            .is_none_or(|m| registry().get(&m.path).is_none_or(|e| e.complete));
        if complete || !ok {
            self.proxies.done += 1;
        }
        // the cache stays until the search that follows right away (empty, the player would ask the disk)
        self.proxies.scanned = None;
    }

    /// End the proxy build for this project (Cancel in the task display)
    pub fn stop_proxies(&mut self) {
        self.proxies.stopped = true;
        if let Some(t) = self.tasks.running("proxy") {
            t.cancel();
        }
    }

    /// Settings → Performance: proxies
    pub fn proxy_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "Proxys");
        if widgets::toggle_row(
            ui,
            "Proxys für große Videos verwenden",
            &mut self.settings.use_proxies,
        ) {
            self.settings.save();
            self.invalidate_preview();
        }
        theme::row(ui, "Qualität", |ui| {
            let names: Vec<&str> = PROXY_HEIGHTS.iter().map(|p| p.1).collect();
            let mut i = PROXY_HEIGHTS.iter().position(|p| p.0 == self.settings.proxy_h).unwrap_or(0);
            if widgets::segmented(ui, &names, &mut i) {
                self.settings.proxy_h = PROXY_HEIGHTS[i].0;
                self.proxies.stopped = false;
                self.settings.save();
                self.proxy_failed.clear();
                self.invalidate_preview();
            }
        });
        // count the folder at most every 2 s (finished proxies and part folders)
        let (n, bytes) = match self.proxy_stats {
            Some((t, n, b)) if t.elapsed() < std::time::Duration::from_secs(2) => (n, b),
            _ => {
                let (n, b) = folder_usage(&dir());
                self.proxy_stats = Some((std::time::Instant::now(), n, b));
                (n, b)
            }
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(crate::tr(format!(
                    "{n} Proxys · {:.0} MB",
                    bytes as f64 / 1e6
                )))
                .small()
                .color(theme::TEXT_DIM()),
            );
            if n > 0
                && ui
                    .add(theme::ghost(format!("{}  Proxys löschen", icon::TRASH)))
                    .clicked()
            {
                // a running build notices this after its part (entry missing) and ends
                let _ = std::fs::remove_dir_all(dir());
                registry().clear();
                self.proxy_picks.clear();
                self.proxy_stats = None;
                self.proxies.known.clear();
                self.proxies.scanned = None;
                self.invalidate_preview();
            }
        });
    }
}

/// Test: set the inventory for `src` – source 100 s, parts of 10 s, finished per `ready` (one file
/// per finished part)
#[cfg(test)]
pub(crate) fn set_test_entry(src: &Path, out: PathBuf, want: (u32, Rational), ready: Vec<bool>, complete: bool) {
    let pt = Parting::new(100.0, want.1, PART_SECS).unwrap();
    let pdir = parts::parts_dir(&out);
    std::fs::create_dir_all(&pdir).unwrap();
    for k in (0..ready.len()).filter(|k| ready[*k]) {
        std::fs::write(pdir.join(parts::part_name(k as u32)), b"x").unwrap();
    }
    let spec = ProxySpec { height: want.0, fps: None, gop: 15 };
    let fails = vec![0; ready.len()];
    let file = complete.then(|| out.clone());
    let mut e = Entry { want, out, spec, pt: Some(pt), dur: 100.0, ready, fails, complete, file, dead: false, ranges: Arc::default() };
    e.changed();
    registry().insert(src.to_path_buf(), e);
}

/// Test: build real parts `0..n` of source `m` and set the inventory as in a running build
/// (proxy unfinished). Like the app: division by the proxy's frame rate.
#[cfg(test)]
pub(crate) fn test_build_parts(m: &MediaRef, out: PathBuf, want: (u32, Rational), n: u32) {
    let spec = proxy_spec(m, want.0, want.1);
    let fps = spec.fps.or(m.fps).unwrap_or(Rational::new(30, 1));
    let dur = m.duration.as_secs_f64();
    let pt = Parting::new(dur, fps, PART_SECS).unwrap();
    let dir = parts::parts_dir(&out);
    std::fs::create_dir_all(&dir).unwrap();
    let st = probe_start(&m.path);
    let mut ready = vec![false; pt.count as usize];
    for k in 0..n.min(pt.count) {
        build_part(m, &dir, &pt, st, k, &spec, &|_| {}, &AtomicBool::new(false)).unwrap();
        ready[k as usize] = true;
    }
    let fails = vec![0; ready.len()];
    let mut e = Entry { want, out, spec, pt: Some(pt), dur, ready, fails, complete: false, file: None, dead: false, ranges: Arc::default() };
    e.changed();
    registry().insert(m.path.clone(), e);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(w: u32, h: u32, fps: Rational, codec: &str) -> MediaRef {
        MediaRef {
            id: MediaId::new(),
            path: PathBuf::from("quelle.mp4"),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(60.0),
            width: w,
            height: h,
            fps: Some(fps),
            has_audio: false,
            audio_streams: 0,
            video_codec: Some(codec.into()),
            audio_codec: None,
            has_alpha: false,
        }
    }

    /// Held by tests that run FFmpeg through `run_encode`: `cancel_while_paused_leaves_nothing`
    /// pauses every running build (process-wide switch) while it runs.
    fn builds() -> std::sync::MutexGuard<'static, ()> {
        static L: Mutex<()> = Mutex::new(());
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Removes the source's registry entry when the test ends (also on failure)
    struct Forget(PathBuf);
    impl Drop for Forget {
        fn drop(&mut self) {
            registry().remove(&self.0);
        }
    }

    #[test]
    fn spec_follows_project_rate_and_keeps_short_gop() {
        let r = |n| Rational::new(n, 1);
        // 60 fps source in a 30 fps project → 30 fps, keyframe every 15 frames (0.5 s)
        let s = proxy_spec(&media(1920, 1080, r(60), "h264"), 540, r(30));
        assert_eq!(s, ProxySpec { height: 540, fps: Some(r(30)), gop: 15 });
        // in a 60 fps project it stays at 60
        assert_eq!(proxy_spec(&media(1920, 1080, r(60), "h264"), 540, r(60)), ProxySpec { height: 540, fps: None, gop: 30 });
        // a slower source is not scaled up; 29.97 in a 30 fps project stays
        let ntsc = Rational::new(30000, 1001);
        assert_eq!(proxy_spec(&media(3840, 2160, ntsc, "hevc"), 720, r(30)).fps, None);
        assert_eq!(proxy_spec(&media(3840, 2160, r(25), "hevc"), 720, r(30)).gop, 13);
        // 120 fps in a 60 fps project; never higher than the original
        let s = proxy_spec(&media(1280, 720, r(120), "h264"), 1080, r(60));
        assert_eq!((s.height, s.fps, s.gop), (720, Some(r(60)), 30));
        // broken values: no division by zero, GOP at least 1
        let mut m = media(1920, 1080, Rational::new(0, 0), "h264");
        assert_eq!(proxy_spec(&m, 540, Rational::new(0, 0)).fps, None);
        m.fps = Some(Rational::new(1, 1));
        assert_eq!(proxy_spec(&m, 540, r(30)).gop, 1);
    }

    #[test]
    fn existing_and_larger_proxies_stay_valid() {
        let m = media(1920, 1080, Rational::new(60, 1), "h264");
        let c = candidates(&m, 540, Rational::new(30, 1), 0xab);
        let names: Vec<&str> = c.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "00000000000000ab-540-30_1fps.mp4",
                "00000000000000ab-540.mp4",
                "00000000000000ab-720-30_1fps.mp4",
                "00000000000000ab-720.mp4",
                "00000000000000ab-1080-30_1fps.mp4",
                "00000000000000ab-1080.mp4",
            ]
        );
        // 1080 chosen: only 1080 (smaller proxies would be less sharp than wanted)
        assert_eq!(candidates(&m, 1080, Rational::new(30, 1), 1).len(), 2);
        // source not faster than the project: only the names without frame rate (as before)
        let m = media(3840, 2160, Rational::new(30, 1), "hevc");
        assert!(candidates(&m, 720, Rational::new(30, 1), 1).iter().all(|(n, _)| !n.contains("fps")));
    }

    #[test]
    fn encoder_chain_falls_back_to_software() {
        use Encoder::*;
        assert_eq!(encoder_chain(&[Software, Mf, Amf, Nvenc, Qsv], &[], "auto"), [Nvenc, Qsv, Amf, Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Nvenc, Qsv], &[Nvenc], "auto"), [Qsv, Software]);
        assert_eq!(encoder_chain(&[], &[], "auto"), [Software]);
        assert_eq!(encoder_chain(&[Software], &[Software], "auto"), [Software], "software always stays");
        // setting: chosen path first; failed or not found → like automatic or software
        assert_eq!(encoder_chain(&[Software, Nvenc, Mf], &[], "mf"), [Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Nvenc, Mf], &[Mf], "mf"), [Software]);
        assert_eq!(encoder_chain(&[Software, Mf], &[], "nvenc"), [Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Nvenc], &[], "software"), [Software]);
        let m = media(3840, 2160, Rational::new(60, 1), "hevc");
        assert_eq!(hw_decode(Nvenc, &m), Some("cuda"));
        assert_eq!(hw_decode(Software, &m), None);
        assert_eq!(hw_decode(Nvenc, &media(1920, 1080, Rational::new(60, 1), "h264")), None);
    }

    #[test]
    fn encode_args_set_size_rate_and_gop() {
        // parts (and the build in one go as one part over everything): same grid as a build in one go, exactly `per` frames, without B-frames
        // (parts of different encoders joined gave a duplicate frame at the seam)
        let pt = Parting::new(25.0, Rational::new(30, 1), 10.0).unwrap();
        for enc in Encoder::AUTO {
            let a = part_args(Path::new("q.mp4"), 1.5, &pt, 2, 540, 15, enc, None).join(" ");
            assert!(a.contains("-ss 19.500000 -copyts -i q.mp4"), "{a}");
            assert!(a.contains("fps=30/1:start_time=21.500000,setpts=PTS-STARTPTS,scale=-2:540"), "{a}");
            assert!(a.ends_with("-bf 0 -g 15 -frames:v 300 -r 30/1"), "{a}");
        }
        // Media Foundation: nv12, hardware only, bitrate by size and frame rate
        let a = part_args(Path::new("q.mp4"), 1.5, &pt, 2, 540, 15, Encoder::Mf, None).join(" ");
        assert!(a.contains("-c:v h264_mf -pix_fmt nv12 -hw_encoding 1 -b:v 2499k -bf 0"), "{a}");
        let a = part_args(Path::new("q.mp4"), 1.5, &pt, 2, 1080, 15, Encoder::Mf, None).join(" ");
        assert!(a.contains("-b:v 9998k"), "{a}");
    }

    #[test]
    fn pause_on_clicks_and_while_playback_falls_behind() {
        let ms = Duration::from_millis;
        // stopped: fresh click → pause; resume only after 0.8 s of rest
        assert!(want_pause(false, false, false, ms(100)));
        assert!(!want_pause(false, false, false, ms(400)));
        assert!(want_pause(true, false, false, ms(400)));
        assert!(!want_pause(true, false, false, ms(900)));
        // playback without lag: the build keeps running (also with single-frame jobs) …
        assert!(!want_pause(false, true, false, ms(0)));
        assert!(!want_pause(true, true, false, Duration::MAX));
        // … paused with lag
        assert!(want_pause(false, true, true, Duration::MAX));
        assert!(want_pause(true, true, true, ms(0)));
        // lag holds until stop (no back and forth with the automatic quality)
        assert!(latch_behind(false, true, true));
        assert!(latch_behind(true, true, false));
        assert!(!latch_behind(true, false, false));
        assert!(!latch_behind(false, true, false));
    }

    #[test]
    fn scan_runs_every_two_seconds_or_on_new_media_and_counts_the_round() {
        assert!(scan_due(None, 0, 0, false));
        let now = Some((Instant::now(), 5, 1));
        assert!(!scan_due(now, 5, 1, false));
        assert!(scan_due(now, 6, 1, false));
        assert!(scan_due(now, 6, 1, true));
        let ago = |s| Instant::now().checked_sub(Duration::from_secs(s)).map(|t| (t, 5, 1));
        assert!(scan_due(ago(3), 5, 1, false));
        // nothing to do: only every 30 s, unless the project changed (2 s) or media were added (at once)
        assert!(!scan_due(ago(3), 5, 1, true));
        assert!(scan_due(ago(3), 5, 2, true));
        assert!(scan_due(ago(31), 5, 1, true));
        // 3 finished, 7 missing → „Proxys 4 / 10“; nothing missing → round over
        assert_eq!(round(3, 7), (3, 10));
        assert_eq!(round(10, 0), (0, 0));
    }

    #[test]
    fn cached_proxy_only_for_the_same_setting() {
        let mut s = ProxyState::default();
        let (id, r) = (MediaId::new(), Rational::new(30, 1));
        s.key = Some((540, r));
        s.known.insert(id, Some((PathBuf::from("p.mp4"), 540)));
        assert_eq!(s.cached(id, (540, r)), Some(Some((PathBuf::from("p.mp4"), 540))));
        assert_eq!(s.cached(id, (720, r)), None, "other height: unknown");
        assert_eq!(s.cached(MediaId::new(), (540, r)), None);
        s.reset();
        assert_eq!(s.cached(id, (540, r)), Some(Some((PathBuf::from("p.mp4"), 540))), "stays across project change");
    }

    #[test]
    fn folder_usage_counts_parts() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a-540.mp4"), [0u8; 10]).unwrap();
        std::fs::write(d.path().join("b-540.part.mp4"), [0u8; 5]).unwrap();
        let t = d.path().join("c-540.teile");
        std::fs::create_dir(&t).unwrap();
        std::fs::write(t.join("0000.mp4"), [0u8; 7]).unwrap();
        std::fs::write(t.join("0001.mp4"), [0u8; 3]).unwrap();
        // two proxies (finished, in parts), all bytes
        assert_eq!(folder_usage(d.path()), (2, 25));
        assert_eq!(folder_usage(&d.path().join("fehlt")), (0, 0));
    }

    /// Older single-file proxies stay valid: no part build, stream from the finished file.
    #[test]
    fn old_single_file_proxy_is_used_not_rebuilt() {
        // the registry is shared with the app tests: under the home lock, entry removed at the end
        let home = crate::export_ui::TestHome::new();
        let d = &home.dir;
        let src = d.path().join("quelle.mp4");
        std::fs::write(&src, b"x").unwrap();
        let mut m = media(1920, 1080, Rational::new(30, 1), "h264");
        m.path = src;
        let _f = Forget(m.path.clone());
        let root = d.path().join("proxies");
        std::fs::create_dir_all(&root).unwrap();
        // name as before (without frame rate)
        std::fs::write(root.join(format!("{:016x}-540.mp4", media_hash(&m))), b"p").unwrap();
        let (found, needs) = refresh_entry(&root, &m, 540, Rational::new(30, 1), false);
        assert!(!needs, "nothing to build");
        assert_eq!(found.map(|f| f.1), Some(540));
        let reg = registry();
        let e = reg.get(&m.path).unwrap();
        assert!(e.complete && !e.needs(1.0));
        assert!(!parts::parts_dir(&e.out).exists());
    }

    /// Source with the frame number in the grey value (frame n: luma 19 + (n mod 32)·6, in video range 16–235), 60 fps.
    fn coded_source(dir: &Path, name: &str, secs: f64, extra_vf: &str, extra: &[&str]) -> MediaRef {
        let src = dir.join(name);
        let vf = format!("geq=lum='19+mod(N\\,32)*6':cb=128:cr=128{extra_vf}");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("color=c=black:s=320x180:r=60:d={secs}"), "-vf", &vf])
            .args(extra)
            .args(["-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-g", "600", "-threads", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        cl_media::probe(&src).unwrap()
    }

    fn code_of_luma(y: f32) -> u8 {
        (((y - 19.0) / 6.0).round().max(0.0) as u8) % 32
    }

    /// Frame numbers (mod 32) of a file, frame by frame (luma directly, without conversion)
    fn codes(path: &Path) -> Vec<u8> {
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(path)
            .args(["-vf", "scale=2:2:flags=area", "-pix_fmt", "yuv420p", "-f", "rawvideo", "-"])
            .output()
            .unwrap();
        o.stdout.chunks_exact(6).map(|f| code_of_luma(f[0] as f32)).collect()
    }

    /// Parts across several seams = a build in one go: no duplicate, no missing frame – even
    /// with start offset and varying frame rate. Joined, the same frames; a stream over
    /// the part list from a place in the middle of the second part delivers the matching frames.
    /// Where Media Foundation starts, the parts alternate between it and software.
    #[test]
    fn parts_join_frame_exact_and_merge_keeps_every_frame() {
        if !cl_media::test_tools() {
            return;
        }
        let _b = builds();
        let mix = if available_encoders().contains(&Encoder::Mf) { vec![Encoder::Mf, Encoder::Software] } else { vec![Encoder::Software] };
        let d = tempfile::tempdir().unwrap();
        let vfr = ",setpts='(N/60+0.006*sin(N*1.7))/TB'";
        let cases: [(&str, &str, &[&str]); 3] = [
            ("cfr.mp4", "", &[]),
            ("versatz.mp4", "", &["-output_ts_offset", "1.37"]),
            ("vfr.mp4", vfr, &["-fps_mode", "passthrough", "-video_track_timescale", "90000"]),
        ];
        for (name, vf, extra) in cases {
            let m = coded_source(d.path(), name, 4.5, vf, extra);
            let spec = proxy_spec(&m, 90, Rational::new(30, 1));
            let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), 1.0).unwrap();
            assert!(pt.count >= 4, "{name}: {pt:?}");
            let st = probe_start(&m.path);
            let pdir = d.path().join(format!("{name}.teile"));
            let no = AtomicBool::new(false);
            let mut joined = Vec::new();
            for k in 0..pt.count {
                let n = build_part_with(&m, &pdir, &pt, st, k, &spec, mix[k as usize % mix.len()], None, &|_| {}, &no).unwrap();
                let c = codes(&pdir.join(parts::part_name(k)));
                assert_eq!(c.len() as u64, n);
                if k + 1 < pt.count {
                    assert_eq!(n, pt.per as u64, "{name}: part {k} does not have exactly {} frames", pt.per);
                }
                joined.extend(c);
            }
            // comparison: the same way in one go (one "part" over everything)
            let whole = Parting { per: 1_000_000, count: 1, ..pt };
            let wdir = d.path().join(format!("{name}.ganz"));
            build_part_with(&m, &wdir, &whole, st, 0, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
            let reference = codes(&wdir.join(parts::part_name(0)));
            assert_eq!(joined, reference, "{name}: seams not frame-exact");
            if name == "cfr.mp4" {
                // 60 → 30 fps: every second source frame
                assert!(reference.windows(2).all(|w| (w[1] + 32 - w[0]) % 32 == 2), "{reference:?}");
            }
            // joining: the same frames
            let out = d.path().join(format!("{name}-ganz.mp4"));
            merge(&pdir, &pt, &out, &no).unwrap();
            assert_eq!(codes(&out), reference, "{name}: joined differently");
            assert!(!parts::tmp_of(&out).exists());
            // stream over parts 1.. from 1.5 s (0.5 s into part 1): frame 45 and following
            if name == "cfr.mp4" {
                let list = parts::concat_list(&pdir, 1, pt.count).unwrap();
                let s = cl_media::FrameStream::open(&list, Time::from_secs_f64(1.5 - pt.start(1)), 2, 2, Rational::new(30, 1)).unwrap();
                // RGBA (full range) back to luma
                let got: Vec<u8> = std::iter::from_fn(|| s.next_blocking()).map(|f| code_of_luma(16.0 + f.data[0] as f32 * 219.0 / 255.0)).collect();
                assert!(got.len() >= 80, "{}", got.len());
                assert_eq!(&got[..80], &reference[45..125], "stream across the seam");
            }
        }
    }

    /// Frame number of the single frame the player fetches at `t` from `path` (as for the original)
    fn still_code(path: &Path, t: f64) -> u8 {
        let (f, _) = cl_media::decode_frame_opts(path, Time::from_secs_f64(t), 2, 2, None, false, None).unwrap();
        code_of_luma(16.0 + f.data[0] as f32 * 219.0 / 255.0)
    }

    /// Video track begins 0.1 s after the sound: part, joined proxy and build in one go
    /// show the same frame as the original at the same source time (`-ss` counts from the file start).
    #[test]
    fn parts_and_proxy_match_the_original_when_video_starts_late() {
        if !cl_media::test_tools() {
            return;
        }
        let _b = builds();
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("spaet.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-itsoffset", "0.1", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=60:d=4", "-f", "lavfi", "-i", "sine=d=4.1"])
            .args(["-vf", "geq=lum='19+mod(N\\,32)*6':cb=128:cr=128", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-g", "600"])
            .args(["-c:a", "aac", "-threads", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        let spec = proxy_spec(&m, 90, Rational::new(30, 1));
        let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), 1.0).unwrap();
        let pdir = d.path().join("s.teile");
        let no = AtomicBool::new(false);
        let st = probe_start(&src);
        for k in 0..pt.count {
            build_part_with(&m, &pdir, &pt, st, k, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
        }
        let merged = d.path().join("s-ganz.mp4");
        merge(&pdir, &pt, &merged, &no).unwrap();
        let whole = d.path().join("s-zug.mp4");
        build_with(&m, &whole, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
        // times on the grid of both frame rates (30 fps in the proxy, 60 fps in the original)
        for t in [0.0, 1.0, 1.5, 2.0 + 1.0 / 30.0, 3.0] {
            let want = still_code(&src, t);
            let (k, rel) = parts::still_part(&pt, t).unwrap();
            assert_eq!(still_code(&pdir.join(parts::part_name(k)), rel), want, "part at {t} s");
            assert_eq!(still_code(&merged, t), want, "joined at {t} s");
            assert_eq!(still_code(&whole, t), want, "in one go at {t} s");
        }
    }

    /// Video shorter than the file (sound longer): the last part has no frame, the build still finishes.
    #[test]
    fn part_behind_video_end_is_empty_and_merge_still_works() {
        if !cl_media::test_tools() {
            return;
        }
        let _b = builds();
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("kurz.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=2", "-f", "lavfi", "-i", "sine=d=3.5"])
            .args(["-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-threads", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        let spec = proxy_spec(&m, 90, Rational::new(30, 1));
        let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), 1.0).unwrap();
        assert!(pt.count >= 3, "{pt:?}");
        let pdir = d.path().join("k.teile");
        let no = AtomicBool::new(false);
        // temp files of a second window (other process) stay untouched
        std::fs::create_dir_all(&pdir).unwrap();
        let foreign = [pdir.join("0000.part.mp4"), pdir.join("0000.1.part.mp4")];
        for f in &foreign {
            std::fs::write(f, b"fremd").unwrap();
        }
        let n: Vec<u64> = (0..pt.count).map(|k| build_part_with(&m, &pdir, &pt, 0.0, k, &spec, Encoder::Software, None, &|_| {}, &no).unwrap()).collect();
        assert!(foreign.iter().all(|f| std::fs::read(f).unwrap() == b"fremd"), "foreign temp file overwritten");
        assert_eq!(*n.last().unwrap(), 0, "{n:?}");
        assert_eq!(std::fs::metadata(pdir.join(parts::part_name(pt.count - 1))).unwrap().len(), 0);
        let out = d.path().join("k.mp4");
        merge(&pdir, &pt, &out, &no).unwrap();
        assert_eq!(codes(&out).len() as u64, n.iter().sum::<u64>());
        assert_eq!(n.iter().sum::<u64>(), 60);
    }

    /// Restart in the middle of a build: an aborted part and `.part` files left by a hard exit are
    /// cleaned up, only what is missing is built (a finished part never again), one file at the end.
    #[test]
    fn resume_after_abort_builds_only_missing_parts() {
        if !cl_media::test_tools() {
            return;
        }
        let home = crate::export_ui::TestHome::new();
        let _b = builds();
        let d = &home.dir;
        let m = coded_source(d.path(), "q.mp4", 4.0, "", &[]);
        let _f = Forget(m.path.clone());
        let spec = proxy_spec(&m, 90, Rational::new(30, 1));
        let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), 1.0).unwrap();
        assert_eq!(pt.count, 4);
        let root = d.path().join("proxies");
        let out = root.join("q-90.mp4");
        let pdir = parts::parts_dir(&out);
        let no = AtomicBool::new(false);
        // first run: parts 0 and 2 finished, part 1 aborted, part 3 "hard ended"
        for k in [0, 2] {
            build_part_with(&m, &pdir, &pt, 0.0, k, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
        }
        // cancelled from the first progress call: `run_encode` checks the flag again in that same round,
        // also when FFmpeg has already ended, so the part is always aborted (no timing involved)
        let cancel = AtomicBool::new(false);
        let r = build_part_with(&m, &pdir, &pt, 0.0, 1, &spec, Encoder::Software, None, &|_| cancel.store(true, Ordering::Relaxed), &cancel);
        assert_eq!(r.unwrap_err(), "Abgebrochen");
        std::fs::write(pdir.join("0003.part.mp4"), b"halb").unwrap();
        // marker: replace part 2 with a copy of part 0 – if it were rebuilt, it would be correct again
        std::fs::copy(pdir.join(parts::part_name(0)), pdir.join(parts::part_name(2))).unwrap();
        // "restart": clean up, read inventory, continue building
        parts::cleanup(&root);
        assert!(!pdir.join("0003.part.mp4").exists() && !parts::tmp_of(&pdir.join(parts::part_name(1))).exists());
        let ready = parts::scan_ready(&pdir, pt.count);
        assert_eq!(ready, [true, false, true, false]);
        let mut e = Entry {
            want: (90, Rational::new(30, 1)),
            out: out.clone(),
            spec,
            pt: Some(pt),
            dur: m.duration.as_secs_f64(),
            fails: vec![0; 4],
            ready,
            complete: false,
            file: None,
            dead: false,
            ranges: Arc::default(),
        };
        e.changed();
        registry().insert(m.path.clone(), e);
        assert_eq!(build_parts(&m, &out, &spec, &pt, &|_| {}, &no), Ok(true));
        let c = codes(&out);
        assert_eq!(c.len(), 120);
        // part 2 stayed the copy of part 0 (not rebuilt), 1 and 3 are correct
        assert_eq!(&c[60..90], &c[0..30], "finished part rebuilt");
        assert!(c[30..60].iter().enumerate().all(|(i, v)| *v == ((30 + i) * 2 % 32) as u8), "{c:?}");
        assert!(c[90..].iter().enumerate().all(|(i, v)| *v == ((90 + i) * 2 % 32) as u8), "{c:?}");
        assert!(registry().get(&m.path).unwrap().complete);
        assert!(!pdir.exists(), "parts deleted after joining");
    }

    /// Two videos stacked at the playhead: once the first one's playhead part is finished, the
    /// second gets its turn, and only then does the first one's build yield (before, the search always chose the
    /// first again, which yielded again right away). If the second is missing (NAS asleep) or failed,
    /// nobody yields.
    #[test]
    fn second_media_at_playhead_gets_its_part() {
        let mut app = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        let mk = |name: &str| {
            let mut m = media(3840, 2160, Rational::new(30, 1), "h264");
            m.path = app.dir().join(name);
            m.duration = Time::from_secs_f64(100.0);
            m
        };
        let (x, y) = (mk("x.mp4"), mk("y.mp4"));
        for m in [&x, &y] {
            let mut t = cl_core::Track::new(cl_core::TrackKind::Video);
            t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(100.0)));
            app.project.timeline.tracks.push(t);
            app.project.media.push(m.clone());
        }
        app.playhead = Time::from_secs_f64(42.0);
        let ready = |k: Option<usize>| (0..10).map(|i| Some(i) == k).collect::<Vec<_>>();
        let set = |x_ready: Option<usize>, y_ready: Option<usize>| {
            let want = (540, Rational::new(30, 1));
            set_test_entry(&x.path, app.dir().join("x-540.mp4"), want, ready(x_ready), false);
            set_test_entry(&y.path, app.dir().join("y-540.mp4"), want, ready(y_ready), false);
        };
        let missing = [x.id, y.id];
        // none finished: media order, nobody yields
        set(None, None);
        app.update_focus(&ctx);
        assert_eq!(app.next_build(&missing, &ctx), Some(x.id));
        assert!(!other_media_first(&x.path) && !other_media_first(&y.path));
        // the first one's playhead part (4) is finished: the second's turn, the first yields
        set(Some(4), None);
        assert_eq!(app.next_build(&missing, &ctx), Some(y.id));
        assert!(other_media_first(&x.path) && !other_media_first(&y.path));
        // both finished: normal order, nobody yields
        set(Some(4), Some(4));
        assert_eq!(app.next_build(&missing, &ctx), Some(x.id));
        assert!(!other_media_first(&x.path));
        // second missing per check or failed: no yielding
        set(Some(4), None);
        app.presence.set_all([(y.path.clone(), false)].into(), 2);
        app.update_focus(&ctx);
        assert!(!other_media_first(&x.path), "missing medium");
        app.presence.set_all([(y.path.clone(), true)].into(), 2);
        app.proxy_failed.insert(y.id);
        app.update_focus(&ctx);
        assert!(!other_media_first(&x.path), "failed medium");
        app.proxy_failed.clear();
        app.update_focus(&ctx);
        assert!(other_media_first(&x.path));
        // edit with the playhead standing: the second video moves away – the focus follows
        app.project.timeline.tracks[1].clips[0].start = Time::from_secs_f64(200.0);
        app.revision += 1;
        app.update_focus(&ctx);
        assert!(!other_media_first(&x.path), "focus knows the edit");
        assert_eq!(app.next_build(&missing, &ctx), Some(x.id));
        registry().remove(&x.path);
        registry().remove(&y.path);
    }

    /// Source gone or disk full: after 3 failed attempts in a row the build ends instead of
    /// trying each part twice with the whole encoder chain.
    #[test]
    fn build_gives_up_after_three_failures_in_a_row() {
        assert_eq!(fails_in_row(0, false), Some(1));
        assert_eq!(fails_in_row(1, false), Some(2));
        assert_eq!(fails_in_row(2, false), None);
        assert_eq!(fails_in_row(2, true), Some(0), "a success resets");
        if !cl_media::test_tools() {
            return;
        }
        let home = crate::export_ui::TestHome::new();
        let _b = builds();
        let d = &home.dir;
        let mut m = media(1920, 1080, Rational::new(30, 1), "h264");
        m.path = d.path().join("weg.mp4");
        let out = d.path().join("weg-540.mp4");
        let want = (540, Rational::new(30, 1));
        set_test_entry(&m.path, out.clone(), want, vec![false; 10], false);
        let pt = Parting::new(100.0, want.1, PART_SECS).unwrap();
        let spec = proxy_spec(&m, 540, want.1);
        let r = build_parts(&m, &out, &spec, &pt, &|_| {}, &AtomicBool::new(false));
        assert!(r.is_err(), "{r:?}");
        let fails: u32 = registry().remove(&m.path).unwrap().fails.iter().map(|f| u32::from(*f)).sum();
        assert_eq!(fails, 2, "third failure ends the build");
    }

    /// Click test K5: Pause and Play at the same place opened a new stream because the
    /// source choice per layer was discarded in the pause and afterwards made anew with another first part
    /// (new key). In the pause it stays; when stopped it is freed.
    #[test]
    fn stream_picks_survive_a_pause() {
        let mut app = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        app.proxy_picks.insert(7, Pick::Parts { first: 2, end: 9 });
        let rev = app.revision;
        app.core.play(Time::ZERO, rev, 0.0);
        let tick = |app: &mut crate::app::App, now: f64| {
            let t = crate::playcore::Tick { now, playhead: app.core.pos(), revision: app.revision, audio: crate::playcore::AudioIn::Pending, end: Time::from_secs_f64(60.0), fps: 30.0, loop_range: None, recording: false, max_quality_level: 0 };
            app.core.tick(&t);
        };
        tick(&mut app, 0.1);
        assert_eq!(app.core.mode(), crate::playcore::Mode::Spielen);
        let rev = app.revision;
        app.core.pause(rev, true, true);
        app.ensure_proxies(&ctx);
        assert_eq!(app.proxy_picks.get(&7), Some(&Pick::Parts { first: 2, end: 9 }), "pause: choice stays");
        app.core.stop();
        app.ensure_proxies(&ctx);
        assert!(app.proxy_picks.is_empty(), "stopped: freed");
    }

    /// Finished proxy for the stream: file from the inventory, not searched on disk (the search here
    /// would not find it at all); after the build the cache stays until the next search.
    #[test]
    fn full_proxy_stream_comes_from_the_registry() {
        let mut app = crate::saving::tests::test_app();
        let mut m = media(3840, 2160, Rational::new(30, 1), "h264");
        m.path = app.dir().join("v.mp4");
        app.project.media.push(m.clone());
        let want = (app.settings.proxy_h, app.project.settings.fps);
        let out = app.dir().join("woanders").join("v-540.mp4");
        set_test_entry(&m.path, out.clone(), want, vec![true; 10], true);
        assert_eq!(app.stream_source(&m, 540, Pick::Full), (out, Time::ZERO));
        app.proxies.key = Some(want);
        app.proxies.known.insert(m.id, None);
        app.proxy_finished(m.id, true);
        assert_eq!(app.proxies.cached(m.id, want), Some(None), "no look on disk until the search");
        registry().remove(&m.path);
    }

    #[test]
    fn transparent_videos_get_no_proxy() {
        let r = |n| Rational::new(n, 1);
        for (w, h, fps, codec) in [(3840, 2160, r(60), "prores"), (1920, 1080, r(30), "vp9"), (1280, 720, r(30), "qtrle")] {
            let mut m = media(w, h, fps, codec);
            m.has_alpha = true;
            assert!(!wants_proxy(&m), "{codec} {w}x{h} with alpha");
            m.has_alpha = false;
            assert_eq!(wants_proxy(&m), codec != "qtrle", "{codec} {w}x{h} opaque: as before");
        }
    }

    /// A proxy (whole or in parts) left in the cache for a transparent video is never read: it has
    /// no alpha. The same media without alpha does use it (the guard checks the setup).
    #[test]
    fn transparent_video_ignores_existing_proxy() {
        let mut app = crate::saving::tests::test_app();
        app.settings.use_proxies = true;
        let mut m = media(3840, 2160, Rational::new(30, 1), "prores");
        m.path = app.dir().join("alpha.mov");
        let _f = Forget(m.path.clone());
        app.project.media.push(m.clone());
        let want = (app.settings.proxy_h, app.project.settings.fps);
        let out = app.dir().join("alpha-540.mp4");
        std::fs::write(&out, b"x").unwrap();
        set_test_entry(&m.path, out.clone(), want, vec![true; 10], true);
        app.proxies.key = Some(want);
        app.proxies.known.insert(m.id, Some((out.clone(), 540)));
        let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(10.0));
        let pick = app.stream_pick(1, &c, &m, Time::ZERO);
        assert_eq!(app.stream_source(&m, 540, pick).0, out, "opaque: proxy");
        assert_eq!(app.still_source(&m, 540, Time::ZERO, false).0, out, "opaque: proxy");
        // display larger than the proxy: the scrub window reads the proxy at its size, the exact frame
        // at rest the sharp original
        let win = app.window_source(&m, (1920, 1080), Time::ZERO).unwrap();
        assert_eq!((win.path.clone(), win.h), (out.clone(), 540), "window: proxy at its own size");
        assert_eq!(app.window_source(&m, (1280, 720), Time::ZERO).unwrap().h, 360, "at most half the height");
        assert_eq!(app.window_source(&m, (960, 540), Time::ZERO).unwrap().h, 540, "window = still size: reusable");
        assert_eq!(app.still_source(&m, 1080, Time::ZERO, true).0, out, "moving: proxy");
        assert_eq!(app.still_source(&m, 1080, Time::ZERO, false).0, m.path, "rest: original");
        m.has_alpha = true;
        let pick = app.stream_pick(1, &c, &m, Time::ZERO);
        assert_eq!(pick, Pick::Whole);
        assert_eq!(app.stream_source(&m, 540, pick).0, m.path, "alpha: original stream");
        assert_eq!(app.still_source(&m, 540, Time::ZERO, false).0, m.path, "alpha: original single frame");
        assert_eq!(app.still_source(&m, 1080, Time::ZERO, true).0, m.path, "alpha: original while moving too");
        let win = app.window_source(&m, (1920, 1080), Time::ZERO).unwrap();
        assert_eq!((win.path.clone(), win.h), (m.path.clone(), 540), "alpha: window from the original at half height");
    }

    #[test]
    fn proxy_is_sharp_enough_or_moving() {
        assert!(proxy_sharp(false, 648, 540), "up to 6/5 of the proxy lines at rest");
        assert!(!proxy_sharp(false, 720, 540), "rest, larger display: original");
        assert!(proxy_sharp(true, 2160, 540), "moving or playing: proxy");
    }

    #[test]
    fn playhead_media_comes_first() {
        let ids: Vec<MediaId> = (0..4).map(|_| MediaId::new()).collect();
        let set = |v: &[usize]| v.iter().map(|i| ids[*i]).collect::<HashSet<_>>();
        assert_eq!(next_proxy(&ids, &set(&[]), &set(&[])), Some(ids[0]));
        assert_eq!(next_proxy(&ids, &set(&[]), &set(&[2, 3])), Some(ids[2]));
        assert_eq!(next_proxy(&ids, &set(&[3]), &set(&[1, 2])), Some(ids[3]));
        assert_eq!(next_proxy(&[], &set(&[1]), &set(&[])), None);
    }

    fn make_source(dir: &Path, s: &str) -> MediaRef {
        let src = dir.join("big.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", s, "-pix_fmt", "yuv420p", "-threads", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        cl_media::probe(&src).unwrap()
    }

    /// Software path (CI has no GPU): correct size, frame rate, keyframe interval,
    /// playable.
    #[test]
    fn builds_small_proxy() {
        if !cl_media::test_tools() {
            return;
        }
        // touches the registry: home lock first, then the build lock (same order everywhere)
        let home = crate::export_ui::TestHome::new();
        let _b = builds();
        let d = &home.dir;
        let m = make_source(d.path(), "testsrc2=s=2560x1440:r=60:d=2");
        assert!(wants_proxy(&m));
        let out = d.path().join("p.mp4");
        let spec = proxy_spec(&m, 540, Rational::new(30, 1));
        build_with(&m, &out, &spec, Encoder::Software, None, &|_| {}, &AtomicBool::new(false)).unwrap();
        let p = cl_media::probe(&out).unwrap();
        assert_eq!((p.width, p.height), (960, 540));
        assert!((p.fps.unwrap().as_f64() - 30.0).abs() < 0.01);
        assert!((p.duration.as_secs_f64() - 2.0).abs() < 0.2);
        // keyframes: 0, 0.5, 1.0, 1.5 s
        let o = cl_media::ffprobe_command()
            .unwrap()
            .args(["-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey", "-show_entries", "frame=pts_time", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .unwrap();
        let keys = String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(keys, 4, "{}", String::from_utf8_lossy(&o.stdout));
        // playable: stream delivers all frames
        let s = cl_media::FrameStream::open(&out, Time::ZERO, 96, 54, Rational::new(30, 1)).unwrap();
        assert!(std::iter::from_fn(|| s.next_blocking()).count() >= 58);
        // whole path with encoder chain (hardware if present, otherwise software); in the inventory without
        // division (duration unknown) it then counts as finished
        let out2 = d.path().join("q.mp4");
        set_test_entry(&m.path, out2.clone(), (540, Rational::new(30, 1)), Vec::new(), false);
        registry().get_mut(&m.path).unwrap().pt = None;
        build(&m, &out2, &spec, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(cl_media::probe(&out2).unwrap().height, 540);
        let e = registry().remove(&m.path).unwrap();
        assert!(e.complete && e.file.as_ref() == Some(&out2) && !e.needs(0.0));
        assert!(!parts::tmp_of(&out2).exists());
    }

    /// Cancel during the pause too: no half file is left behind.
    #[test]
    fn cancel_while_paused_leaves_nothing() {
        if !cl_media::test_tools() {
            return;
        }
        // the pause below is process-wide: keep other proxy builds (here and under the home lock) out
        let _home = crate::export_ui::TestHome::new();
        let _b = builds();
        let d = tempfile::tempdir().unwrap();
        let m = make_source(d.path(), "testsrc2=s=1920x1080:r=60:d=4");
        let out = d.path().join("p.mp4");
        let spec = proxy_spec(&m, 540, Rational::new(30, 1));
        let cancel = AtomicBool::new(false);
        // paused before the start: the first round of `run_encode` suspends FFmpeg, then calls
        // progress, which cancels; the cancel wins in that same round even if FFmpeg had ended.
        // Deterministic, unlike a player click (pause only within 300 ms of it).
        struct Unpause;
        impl Drop for Unpause {
            fn drop(&mut self) {
                ProxyState::set_paused(false);
            }
        }
        let _u = Unpause;
        ProxyState::set_paused(true);
        let r = build_with(&m, &out, &spec, Encoder::Software, None, &|_| cancel.store(true, Ordering::Relaxed), &cancel);
        assert_eq!(r.unwrap_err(), "Abgebrochen");
        assert!(!out.exists() && !parts::tmp_of(&out).exists());
    }

    fn median(v: &mut [f64]) -> f64 {
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
    }

    /// Measurement test (manual): proxy in parts on a real source (default: 23 min 1080p60 original).
    /// `cargo test --release -p cl-app --bin cutline -- --ignored --nocapture measure_parts`
    /// (`CUTLINE_PROXY_SRC=<file>`; `CUTLINE_PROXY_FULL=1`: whole build without and with simulated playback)
    #[test]
    #[ignore = "measurement only: needs FFmpeg and a long video (CUTLINE_PROXY_SRC or %USERPROFILE%/Downloads/videoplayback.mp4), prints timings"]
    fn measure_parts() {
        let src = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Downloads/videoplayback.mp4");
        let src = std::env::var_os("CUTLINE_PROXY_SRC").map(PathBuf::from).unwrap_or(src);
        if !src.is_file() {
            return;
        }
        let m = cl_media::probe(&src).unwrap();
        let spec = proxy_spec(&m, 540, Rational::new(30, 1));
        let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), PART_SECS).unwrap();
        let d = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);
        let secs = |t: Instant| t.elapsed().as_secs_f64();
        println!("Quelle {} ({:.0} s, {} Stücke à {} Bilder)", src.display(), m.duration.as_secs_f64(), pt.count, pt.per);
        let picks = [0, pt.count / 3, pt.count / 2, pt.count * 3 / 4, pt.count - 2];
        let mut built: Option<PathBuf> = None;
        for enc in encoder_chain(available_encoders(), &[], "auto") {
            for hw in decode_attempts(enc, &m) {
                let dir = d.path().join(format!("{enc:?}-{hw:?}"));
                let t = Instant::now();
                // as in the build: start time of the source first
                let st = probe_start(&src);
                let first = build_part_with(&m, &dir, &pt, st, 0, &spec, enc, hw, &|_| {}, &no).map(|_| secs(t));
                let Ok(first) = first else {
                    println!("{enc:?} hw={hw:?}: Fehler {first:?}");
                    continue;
                };
                let mut more: Vec<f64> = picks[1..]
                    .iter()
                    .filter_map(|k| {
                        let t = Instant::now();
                        build_part_with(&m, &dir, &pt, st, *k, &spec, enc, hw, &|_| {}, &no).ok().map(|_| secs(t))
                    })
                    .collect();
                let best = more.iter().copied().fold(f64::MAX, f64::min);
                let bytes: u64 = picks.iter().filter_map(|k| std::fs::metadata(dir.join(parts::part_name(*k))).ok()).map(|x| x.len()).sum();
                println!(
                    "{enc:?} hw={hw:?}: erstes Stück {first:.2} s (mit ffprobe), weitere Median {:.2} s bestes {best:.2} s = {:.1} s je Minute Quelle, {:.1} MB/min",
                    median(&mut more),
                    median(&mut more.clone()) / PART_SECS * 60.0,
                    bytes as f64 / 1e6 / (picks.len() as f64 * PART_SECS / 60.0)
                );
                built.get_or_insert(dir);
            }
        }
        let Some(dir) = built else { return };
        // single frame and stream start: part against original at the same places
        let still = |path: &Path, at: f64| {
            let t = Instant::now();
            cl_media::decode_frame_opts(path, Time::from_secs_f64(at), 960, 540, None, false, None).unwrap();
            secs(t) * 1e3
        };
        let (mut sp, mut so, mut ap, mut ao) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (i, k) in picks.iter().enumerate() {
            for off in [1.3, 6.7] {
                let at = pt.start(*k) + off + i as f64 * 0.37;
                let (kk, rel) = parts::still_part(&pt, at).unwrap();
                sp.push(still(&dir.join(parts::part_name(kk)), rel));
                so.push(still(&src, at));
                let first = |p: &Path, at: f64, hw: bool| {
                    let t = Instant::now();
                    let s = cl_media::FrameStream::open_opts(p, Time::from_secs_f64(at), 960, 540, Rational::new(30, 1), None, if hw { cl_media::Hw::On } else { cl_media::Hw::Off }).unwrap();
                    s.next_blocking().unwrap();
                    secs(t) * 1e3
                };
                let list = parts::concat_list(&dir, *k, k + 1).unwrap();
                ap.push(first(&list, at - pt.start(*k), false));
                ao.push(first(&src, at, crate::preview::stream_hw(&m, &src)));
            }
        }
        let best = |v: &[f64]| v.iter().copied().fold(f64::MAX, f64::min);
        println!(
            "Einzelbild: Stück Median {:.0} ms bestes {:.0} ms, Original Median {:.0} ms bestes {:.0} ms",
            median(&mut sp.clone()),
            best(&sp),
            median(&mut so.clone()),
            best(&so)
        );
        println!(
            "Stromstart: Stücke Median {:.0} ms bestes {:.0} ms, Original Median {:.0} ms bestes {:.0} ms",
            median(&mut ap.clone()),
            best(&ap),
            median(&mut ao.clone()),
            best(&ao)
        );
        // restart in the middle of a build: abort a part (FFmpeg ended), a ".part" of a hard exit lies around
        let rdir = d.path().join("neustart.teile");
        for k in 0..3 {
            build_part_with(&m, &rdir, &pt, 0.0, k, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let t = Instant::now();
        let r = build_part_with(&m, &rdir, &pt, 0.0, 3, &spec, Encoder::Software, None, &|_| {
            if t.elapsed() > Duration::from_millis(300) {
                cancel.store(true, Ordering::Relaxed)
            }
        }, &cancel);
        std::fs::write(rdir.join("0004.part.mp4"), b"halb").unwrap();
        let before: Vec<_> = (0..3).map(|k| std::fs::metadata(rdir.join(parts::part_name(k))).unwrap().modified().unwrap()).collect();
        parts::cleanup(d.path());
        let ready = parts::scan_ready(&rdir, pt.count);
        let next = parts::next_part(&pt, |i| ready[i as usize], &Focus::default());
        let after: Vec<_> = (0..3).map(|k| std::fs::metadata(rdir.join(parts::part_name(k))).unwrap().modified().unwrap()).collect();
        println!(
            "Neustart: Abbruch {:?}, fertig {:?}, .part übrig {}, weiter mit Stück {next:?}, fertige unverändert {}",
            r.err(),
            (0..pt.count).filter(|k| ready[*k as usize]).collect::<Vec<_>>(),
            std::fs::read_dir(&rdir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".part.mp4")),
            before == after
        );
        if std::env::var_os("CUTLINE_PROXY_FULL").is_none() {
            return;
        }
        // whole build: without playback and with continuously running playback (stream from the original in real time)
        for playing in [false, true] {
            let root = d.path().join(format!("voll-{playing}"));
            let out = root.join("p.mp4");
            let mut e = Entry {
                want: (540, Rational::new(30, 1)),
                out: out.clone(),
                spec,
                pt: Some(pt),
                dur: m.duration.as_secs_f64(),
                ready: vec![false; pt.count as usize],
                fails: vec![0; pt.count as usize],
                complete: false,
                file: None,
                dead: false,
                ranges: Arc::default(),
            };
            e.changed();
            registry().insert(m.path.clone(), e);
            let stop = Arc::new(AtomicBool::new(false));
            let player = playing.then(|| {
                let (src, stop, m) = (src.clone(), stop.clone(), m.clone());
                set_play_state(true, false);
                std::thread::spawn(move || {
                    let mut at = 0.0;
                    let (mut frames, mut late) = (0u64, 0u64);
                    while !stop.load(Ordering::Relaxed) {
                        let s = cl_media::FrameStream::open_opts(&src, Time::from_secs_f64(at), 960, 540, Rational::new(30, 1), None, crate::preview::stream_decode(&m, &src)).unwrap();
                        let t0 = Instant::now();
                        while let Some(_f) = s.next_blocking() {
                            frames += 1;
                            let due = frames as f64 / 30.0;
                            let now = t0.elapsed().as_secs_f64();
                            if now < due {
                                std::thread::sleep(Duration::from_secs_f64(due - now));
                            } else if now - due > 0.1 {
                                late += 1;
                            }
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                        at = 0.0;
                    }
                    (frames, late)
                })
            });
            let t = Instant::now();
            let r = build_parts(&m, &out, &spec, &pt, &|_| {}, &no);
            let total = secs(t);
            stop.store(true, Ordering::Relaxed);
            set_play_state(false, false);
            let pl = player.map(|h| h.join().unwrap());
            println!(
                "Gesamtbau {}: {total:.0} s ({:.1} s je Minute Quelle) {r:?}{}",
                if playing { "bei laufender Wiedergabe" } else { "ohne Wiedergabe" },
                total / (m.duration.as_secs_f64() / 60.0),
                pl.map(|(f, l)| format!(", Wiedergabe {f} Bilder, davon {l} über 100 ms zu spät")).unwrap_or_default()
            );
        }
    }

    /// Measurement test (manual): proxy for the 23 min 1080p60 original with each encoder.
    /// `cargo test --release -p cl-app --bin cutline -- --ignored --nocapture measure_proxy_build` (`CUTLINE_PROXY_SRC=<file>`)
    #[test]
    #[ignore = "measurement only: needs FFmpeg and a long video (CUTLINE_PROXY_SRC or %USERPROFILE%/Downloads/videoplayback.mp4), prints timings"]
    fn measure_proxy_build() {
        let src = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Downloads/videoplayback.mp4");
        let src = std::env::var_os("CUTLINE_PROXY_SRC").map(PathBuf::from).unwrap_or(src);
        if !src.is_file() {
            return;
        }
        let m = cl_media::probe(&src).unwrap();
        let d = tempfile::tempdir().unwrap();
        let spec = proxy_spec(&m, 540, Rational::new(30, 1));
        let dur = m.duration.as_secs_f64();
        for enc in encoder_chain(available_encoders(), &[], "auto") {
            for hw in decode_attempts(enc, &m) {
                let out = d.path().join(format!("{enc:?}-{hw:?}.mp4"));
                match build_with(&m, &out, &spec, enc, hw, &|_| {}, &AtomicBool::new(false)) {
                    Ok(r) => {
                        let mb = std::fs::metadata(&out).unwrap().len() as f64 / 1e6;
                        println!(
                            "{enc:?} hw={hw:?}: {:.1} s, {:.1}× Echtzeit, {:.1} MB/min ({:.0} MB)",
                            r.secs,
                            dur / r.secs,
                            mb / (dur / 60.0),
                            mb
                        );
                    }
                    Err(e) => println!("{enc:?} hw={hw:?}: Fehler {e}"),
                }
            }
        }
    }
}
