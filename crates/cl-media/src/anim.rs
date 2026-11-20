//! Animated images (GIF, APNG) as an image clip: frame times and loop from the file header.
//!
//! An animated image plays once in the image clip and then stays on the last frame,
//! unless the file is stored as an endless loop (APNG `num_plays = 0`, GIF loop count 0)
//! – then it repeats over the whole clip length. Preview and export both use this data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cl_core::Time;

#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    /// Start of each frame (ascending, first = 0)
    pub starts: Vec<Time>,
    pub duration: Time,
    pub looping: bool,
}

impl Animation {
    /// Seek time (`-ss`) for the frame visible `t` after clip start. Lies 1 ms before the
    /// frame start because from `-ss` FFmpeg only delivers frames that don't start earlier.
    pub fn seek_time(&self, t: Time) -> Time {
        let t = if self.looping && self.duration.0 > 0 {
            Time(t.0.rem_euclid(self.duration.0))
        } else {
            t
        };
        let i = self.starts.partition_point(|s| *s <= t).saturating_sub(1);
        Time((self.starts.get(i).map_or(0, |s| s.0) - 1000).max(0))
    }
}

/// Animated image? Detected by content (GIF header or PNG with `acTL`), not by extension.
/// `None` for still images and everything else. The result is cached per path.
// shortcut: the cache knows no file changes, upgrade if images get replaced while running
pub fn animation(path: &Path) -> Option<Animation> {
    static CACHE: Mutex<Option<HashMap<PathBuf, Option<Animation>>>> = Mutex::new(None);
    if let Some(a) = CACHE.lock().ok()?.get_or_insert_with(HashMap::new).get(path) {
        return a.clone();
    }
    let a = std::fs::read(path).ok().and_then(|b| parse(&b));
    CACHE
        .lock()
        .ok()?
        .get_or_insert_with(HashMap::new)
        .insert(path.to_path_buf(), a.clone());
    a
}

/// All frames of an animated image in `w`×`h` (RGBA, order as `starts`) with one
/// FFmpeg run: the preview keeps them in memory instead of starting one FFmpeg per frame. `None` if
/// it is not an animated image, it would exceed `max_bytes` or FFmpeg does not deliver all frames.
pub fn decode_all(path: &Path, w: u32, h: u32, max_bytes: usize) -> Option<Vec<crate::RgbaFrame>> {
    let a = animation(path)?;
    let len = (w as usize) * (h as usize) * 4;
    if len == 0 || a.starts.len().saturating_mul(len) > max_bytes {
        return None;
    }
    let out = decode_all_cmd(path, w, h, a.starts.len())?
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let frames: Vec<crate::RgbaFrame> = out
        .stdout
        .chunks_exact(len)
        .take(a.starts.len())
        .map(|c| crate::RgbaFrame { width: w, height: h, data: c.to_vec() })
        .collect();
    (frames.len() == a.starts.len()).then_some(frames)
}

/// FFmpeg for [`decode_all`]: at most `frames` frames – if FFmpeg counts more than our own reader
/// (unusual file), the output would otherwise grow past the checked memory limit.
fn decode_all_cmd(path: &Path, w: u32, h: u32, frames: usize) -> Option<std::process::Command> {
    let mut cmd = crate::ffmpeg_command().ok()?;
    cmd.args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        // like the single frame (`decode_frame`): same scaling, every frame exactly once
        .args(["-vf", &format!("scale={w}:{h}:flags=bilinear,format=rgba"), "-fps_mode", "passthrough"])
        .args(["-frames:v", &frames.to_string()])
        .args(["-an", "-sn", "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
    Some(cmd)
}

/// Index of the frame hit by seek time `seek` (from [`Animation::seek_time`])
pub fn frame_index(a: &Animation, seek: Time) -> usize {
    a.starts.partition_point(|s| s.0 <= seek.0 + 1000).saturating_sub(1)
}

fn parse(b: &[u8]) -> Option<Animation> {
    let (delays, looping) = if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        apng(b)?
    } else if b.starts_with(b"GIF8") {
        gif(b)?
    } else {
        return None;
    };
    if delays.len() < 2 {
        return None;
    }
    let mut starts = Vec::with_capacity(delays.len());
    let mut t = 0i64;
    for d in delays {
        starts.push(Time(t));
        t += d;
    }
    Some(Animation { starts, duration: Time(t), looping })
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Frame durations (µs) and loop from `acTL`/`fcTL`, like FFmpeg's APNG reader
/// (delay 0 → 1/15 s).
fn apng(b: &[u8]) -> Option<(Vec<i64>, bool)> {
    let (mut i, mut delays, mut plays) = (8, Vec::new(), None);
    while i + 8 <= b.len() {
        let len = be32(b, i)? as usize;
        let data = b.get(i + 8..i + 8 + len)?;
        match &b[i + 4..i + 8] {
            b"acTL" => plays = Some(be32(data, 4)?),
            b"fcTL" => {
                let num = u16::from_be_bytes([*data.get(20)?, *data.get(21)?]) as i64;
                let den = match u16::from_be_bytes([*data.get(22)?, *data.get(23)?]) {
                    0 => 100,
                    d => d as i64,
                };
                delays.push(if num == 0 { 1_000_000 / 15 } else { num * 1_000_000 / den });
            }
            b"IEND" => break,
            _ => {}
        }
        i += 12 + len;
    }
    Some((delays, plays? == 0))
}

/// Frame durations (µs) and loop (NETSCAPE block with count 0) of a GIF, like FFmpeg's GIF
/// reader (delay below 2/100 s → 1/10 s).
fn gif(b: &[u8]) -> Option<(Vec<i64>, bool)> {
    let skip_blocks = |mut i: usize| -> Option<usize> {
        loop {
            let n = *b.get(i)? as usize;
            i += 1 + n;
            if n == 0 {
                return Some(i);
            }
        }
    };
    let flags = *b.get(10)?;
    let mut i = 13 + if flags & 0x80 != 0 { 3 << ((flags & 7) + 1) } else { 0 };
    let (mut delays, mut delay, mut looping) = (Vec::new(), 0i64, false);
    loop {
        match *b.get(i)? {
            0x21 => {
                let label = *b.get(i + 1)?;
                let first = b.get(i + 3..i + 3 + *b.get(i + 2)? as usize)?;
                if label == 0xF9 && first.len() >= 3 {
                    delay = u16::from_le_bytes([first[1], first[2]]) as i64;
                } else if label == 0xFF && first == b"NETSCAPE2.0" {
                    let sub = b.get(i + 3 + first.len()..i + 3 + first.len() + 4)?;
                    looping = sub[0] >= 3 && sub[1] == 1 && sub[2] == 0 && sub[3] == 0;
                }
                i = skip_blocks(i + 2)?;
            }
            0x2C => {
                let f = *b.get(i + 9)?;
                i += 10 + if f & 0x80 != 0 { 3 << ((f & 7) + 1) } else { 0 };
                i = skip_blocks(i + 1)?;
                delays.push(if delay < 2 { 10 } else { delay } * 10_000);
                delay = 0;
            }
            _ => break,
        }
    }
    Some((delays, looping))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review finding: the memory limit only applied to the frames we counted, but FFmpeg
    /// delivered all of them. Now at most as many as were checked.
    #[test]
    fn decode_all_reads_at_most_the_counted_frames() {
        let Some(cmd) = decode_all_cmd(Path::new("a.gif"), 8, 8, 7) else { return };
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.windows(2).any(|w| w[0] == "-frames:v" && w[1] == "7"), "{args:?}");
    }

    fn ffmpeg(args: &[&str]) {
        let st = crate::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y"])
            .args(args)
            .status()
            .unwrap();
        assert!(st.success());
    }

    #[test]
    fn reads_frames_and_loop_flag_of_apng_and_gif() {
        if !crate::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n).to_string_lossy().into_owned();
        let src = "testsrc2=size=32x32:rate=10:duration=1";
        ffmpeg(&["-f", "lavfi", "-i", src, "-plays", "0", "-f", "apng", &p("loop.png")]);
        ffmpeg(&["-f", "lavfi", "-i", src, "-plays", "1", "-f", "apng", &p("once.png")]);
        ffmpeg(&["-f", "lavfi", "-i", src, "-loop", "0", &p("loop.gif")]);
        ffmpeg(&["-f", "lavfi", "-i", src, "-loop", "-1", &p("once.gif")]);
        ffmpeg(&["-f", "lavfi", "-i", src, "-frames:v", "1", &p("still.png")]);
        for (f, looping) in [("loop.png", true), ("once.png", false), ("loop.gif", true), ("once.gif", false)] {
            let a = animation(Path::new(&p(f))).unwrap_or_else(|| panic!("{f}"));
            assert_eq!(a.looping, looping, "{f}");
            assert_eq!(a.starts.len(), 10, "{f}");
            assert!((a.duration.as_secs_f64() - 1.0).abs() < 0.02, "{f}: {:?}", a.duration);
            // animated images are images (image clip with free length), not videos
            assert_eq!(crate::probe(Path::new(&p(f))).unwrap().kind, cl_core::MediaKind::Image, "{f}");
        }
        assert!(animation(Path::new(&p("still.png"))).is_none());
        assert!(animation(Path::new(&p("fehlt.gif"))).is_none());
    }

    /// All frames at once (preview in memory) = the same frames as one by one with `-ss`.
    #[test]
    fn decode_all_matches_single_frames() {
        if !crate::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = "testsrc2=size=48x32:rate=10:duration=0.8";
        for (n, args) in [("a.png", vec!["-plays", "0", "-f", "apng"]), ("a.gif", vec!["-loop", "0"])] {
            let f = dir.path().join(n);
            let fs = f.to_string_lossy().into_owned();
            let mut all = vec!["-f", "lavfi", "-i", src];
            all.extend(args);
            all.push(&fs);
            ffmpeg(&all);
            let a = animation(&f).unwrap();
            let frames = decode_all(&f, 24, 16, 1 << 20).unwrap_or_else(|| panic!("{n}"));
            assert_eq!(frames.len(), a.starts.len());
            for (i, s) in a.starts.iter().enumerate() {
                let seek = a.seek_time(*s);
                assert_eq!(frame_index(&a, seek), i);
                let one = crate::decode_frame(&f, seek, 24, 16).unwrap();
                assert!(one.data == frames[i].data, "{n}: Bild {i} weicht ab");
            }
            // too large: not in memory
            assert!(decode_all(&f, 24, 16, 100).is_none());
        }
        assert!(decode_all(&dir.path().join("fehlt.gif"), 24, 16, 1 << 20).is_none());
    }

    /// The preview decodes with `-ss seek_time` exactly the frame the export shows.
    #[test]
    fn seek_time_hits_the_frame_with_ffmpeg() {
        if !crate::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let band = "color=red:size=64x64:rate=10:duration=1,format=rgba,geq=r=255:g=0:b=0:a='if(between(X-T*48,0,15),255,0)'";
        for (name, plays, ext) in [("loop", "0", "apng"), ("once", "1", "apng"), ("once", "-1", "gif")] {
            let f = dir.path().join(format!("{name}.{ext}"));
            let fs = f.to_string_lossy().into_owned();
            let mut args = vec!["-f", "lavfi", "-i", band];
            if ext == "gif" {
                args.extend(["-vf", "split[a][b];[a]palettegen=reserve_transparent=1[p];[b][p]paletteuse", "-loop", plays]);
            } else {
                args.extend(["-plays", plays, "-f", "apng"]);
            }
            args.push(&fs);
            ffmpeg(&args);
            let a = animation(&f).unwrap();
            // left edge of the stripe in row 32
            let left = |t: f64| {
                let fr = crate::decode_frame(&f, a.seek_time(Time::from_secs_f64(t)), 64, 64).unwrap();
                (0..64).find(|x| fr.data[((32 * 64 + x) * 4 + 3) as usize] > 128)
            };
            for k in [0, 1, 5, 9] {
                assert_eq!(left(k as f64 * 0.1 + 0.05), Some((k as f64 * 4.8).ceil() as u32), "{name}.{ext} Bild {k}");
            }
            let want = if name == "loop" { 24 } else { 44 };
            assert_eq!(left(3.55), Some(want), "{name}.{ext} at 3.55 s");
        }
    }

    #[test]
    fn seek_time_loops_or_holds_last_frame() {
        let starts = (0..10).map(|k| Time::from_millis(k * 100)).collect();
        let mut a = Animation { starts, duration: Time::from_millis(1000), looping: false };
        assert_eq!(a.seek_time(Time::from_millis(250)), Time::from_millis(199));
        assert_eq!(a.seek_time(Time::from_millis(3500)), Time::from_millis(899));
        assert_eq!(a.seek_time(Time::ZERO), Time::ZERO);
        a.looping = true;
        assert_eq!(a.seek_time(Time::from_millis(3500)), Time::from_millis(499));
        // truncated files are not an animated image
        assert!(parse(b"GIF89a").is_none());
        assert!(parse(b"\x89PNG\r\n\x1a\n\0\0").is_none());
    }
}
