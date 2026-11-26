//! Object cut-out with SAM 2.1 tiny (Meta, Apache-2.0) in the 512 px fp16 ONNX export by
//! Diffusion Studio (five graphs). Clicks mark an object on one frame ([`Live`]); [`track`]
//! follows it from there forwards and backwards through a source range and [`join`]s a mask video
//! in the format of [`crate::matte`] (gray, FFV1, frame 0 = start of the range).
//!
//! Tracking runs in sections (from the marked frame forwards, and backwards) appended to raw files
//! in a work folder; every [`SAVE_EVERY`] frames the tracker's memory goes to a state file, so a
//! cancelled run resumes there with the same result.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use ort::session::Session;
use ort::value::Tensor;

use crate::Models;

/// Folder of the model in the model folder (catalog entry `sam2`)
pub const MODEL_DIR: &str = "sam2.1-tiny-512";
pub const GRAPHS: [&str; 5] = ["vision_encoder", "mask_decoder", "memory_encoder", "memory_attention", "pointer_tpos"];
/// Frame memories (the marked frame + 6 recent) and object pointers of the export
const MEM_BLOCKS: usize = 7;
const PTRS: usize = 16;
/// Frames between two saves of the tracker state (cancel and resume)
pub const SAVE_EVERY: usize = 30;
/// Longest edge of the written mask
const OUT_MAX: u32 = 1280;

type R<T> = Result<T, String>;

fn oe<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// What the user marked on the base frame, as fractions of the source image.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prompt {
    /// (x, y, background)
    pub points: Vec<(f32, f32, bool)>,
    /// left, top, right, bottom
    pub rect: Option<[f32; 4]>,
}

impl Prompt {
    /// SAM needs an object point or a frame; background points alone mark nothing.
    pub fn usable(&self) -> bool {
        self.rect.is_some() || self.points.iter().any(|p| !p.2)
    }
    /// Points in model pixels with SAM's labels: frame corners 2/3 first, then 1 = object, 0 = background
    /// (the frame is stretched to the square input, so fractions map directly).
    fn model(&self, s: usize) -> Vec<(f32, f32, i32)> {
        let k = s as f32;
        let mut v = vec![];
        if let Some([l, t, r, b]) = self.rect {
            v.push((l.min(r) * k, t.min(b) * k, 2));
            v.push((l.max(r) * k, t.max(b) * k, 3));
        }
        v.extend(self.points.iter().map(|&(x, y, bg)| (x * k, y * k, if bg { 0 } else { 1 })));
        v
    }
}

/// Frames of a source range at the source rate: frame `i` shows `start + i / fps`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub start: f64,
    pub fps: f64,
    pub n: usize,
}

impl Grid {
    /// Range `start..end` (seconds); `start` is moved back onto a source frame.
    pub fn new(start: f64, end: f64, fps: f64) -> Grid {
        let fps = if fps > 0.0 { fps } else { 30.0 };
        let start = ((start * fps + 1e-3).floor() / fps).max(0.0);
        let n = (((end - start) * fps) - 1e-6).ceil().max(1.0) as usize;
        Grid { start, fps, n }
    }
    pub fn time(&self, i: usize) -> f64 {
        self.start + i as f64 / self.fps
    }
    /// End of the last frame (the range the mask file covers)
    pub fn end(&self) -> f64 {
        self.time(self.n)
    }
    /// Frame showing source time `t`
    pub fn index(&self, t: f64) -> usize {
        (((t - self.start) * self.fps + 1e-3).floor().max(0.0) as usize).min(self.n - 1)
    }
}

/// Part of the tracking: from the marked frame `key` forwards (written: `key`, `key + 1`, …) or
/// backwards (written: `key - 1`, `key - 2`, …; the marked frame only sets up the memory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub key: usize,
    pub back: bool,
    /// Frames written
    pub len: usize,
}

impl Section {
    fn name(&self) -> &'static str {
        if self.back { "back" } else { "fwd" }
    }
    /// Grid frame of the `i`-th written frame
    pub fn frame(&self, i: usize) -> usize {
        if self.back { self.key - 1 - i } else { self.key + i }
    }
}

/// Sections for one marked frame: forwards to the end, backwards to the start (if there is room).
pub fn plan(grid: &Grid, key: usize) -> Vec<Section> {
    let key = key.min(grid.n - 1);
    let mut v = vec![Section { key, back: false, len: grid.n - key }];
    if key > 0 {
        v.push(Section { key, back: true, len: key });
    }
    v
}

/// Mask logits (model pixels) as bytes: 128 = undecided, 16 steps per logit unit.
fn quantize(hi: &[f32]) -> Vec<u8> {
    hi.iter().map(|l| (128.0 + 16.0 * l).round().clamp(0.0, 255.0) as u8).collect()
}

/// Bilinear upscaling of the quantized `s`×`s` logits to `w`×`h`, cut at 0 (as SAM's own
/// post-processing): 255 = object.
pub fn upscale(q: &[u8], s: usize, w: usize, h: usize) -> Vec<u8> {
    let mut o = vec![0u8; w * h];
    let (sx, sy) = (s as f32 / w as f32, s as f32 / h as f32);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (s - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(s - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (s - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(s - 1);
            let v = |yy: usize, xx: usize| q[yy * s + xx] as f32;
            let top = v(y0, x0) * (1.0 - tx) + v(y0, x1) * tx;
            let bot = v(y1, x0) * (1.0 - tx) + v(y1, x1) * tx;
            o[y * w + x] = if top * (1.0 - ty) + bot * ty > 128.0 { 255 } else { 0 };
        }
    }
    o
}

// ------------------------------------------------------------------ model

/// The five graphs and the export's constants.
pub struct Sam {
    venc: Session,
    dec: Session,
    menc: Session,
    matt: Session,
    ptpos: Session,
    /// Input size (512)
    pub s: usize,
    /// Temporal position table, 7×64
    tpos: Vec<f32>,
    /// Runs on the graphics card
    pub gpu: bool,
}

struct Feats {
    f0: Vec<f32>,
    f1: Vec<f32>,
    f2: Vec<f32>,
    f2nm: Vec<f32>,
    pos: Vec<f32>,
}

/// Tracker memory: marked frame (tokens, positions, pointer), recent frames and pointers with
/// their step from the marked frame, and the next step.
#[derive(Debug, Clone, PartialEq, Default)]
struct Bank {
    cond: (Vec<f32>, Vec<f32>, Vec<f32>),
    recent: Vec<(usize, Vec<f32>, Vec<f32>)>,
    ptrs: Vec<(usize, Vec<f32>)>,
    n: usize,
}

fn out_f32(res: &ort::session::SessionOutputs, name: &str) -> R<Vec<f32>> {
    let (_, d) = res[name].try_extract_tensor::<f32>().map_err(oe)?;
    Ok(d.to_vec())
}

/// [C, HW] -> [HW, C]
fn chw_to_seq(x: &[f32], c: usize) -> Vec<f32> {
    let hw = x.len() / c;
    let mut o = vec![0f32; x.len()];
    for ci in 0..c {
        for i in 0..hw {
            o[i * c + ci] = x[ci * hw + i];
        }
    }
    o
}

fn json_number(s: &str, key: &str) -> Option<f64> {
    let k = &s[s.find(&format!("\"{key}\""))? + key.len() + 2..];
    k.trim_start_matches([' ', ':']).split([',', '}']).next()?.trim().parse().ok()
}

fn json_table(s: &str, key: &str) -> Option<Vec<f32>> {
    let body = &s[s.find(&format!("\"{key}\""))?..];
    let body = &body[body.find("[[")? + 2..body.find("]]")?];
    Some(body.split([',', '[', ']']).filter_map(|t| t.trim().parse().ok()).collect())
}

impl Sam {
    /// Load the graphs: on the graphics card when the runtime has DirectML (falls back to the
    /// CPU if a session cannot be built there), else on the CPU with `threads`.
    pub fn load(models: &Models, threads: usize) -> R<Sam> {
        let dir = models.sam2().ok_or("Objekt-Modell fehlt (Einstellungen → Modelle)")?;
        let consts = std::fs::read_to_string(dir.join("constants.json")).map_err(oe)?;
        let s = json_number(&consts, "image_size").ok_or("constants.json: image_size")? as usize;
        let tpos = json_table(&consts, "memory_temporal_positional_encoding").filter(|t| t.len() == MEM_BLOCKS * 64).ok_or("constants.json: memory table")?;
        let env = crate::ort_env(models)?;
        #[allow(unused_mut)]
        let mut gpu = crate::gpu_runtime_loaded();
        let mut sessions = vec![];
        for name in GRAPHS {
            let path = dir.join("onnx").join(format!("{name}.onnx"));
            #[allow(unused_mut)]
            let mut sess = None;
            #[cfg(windows)]
            if gpu {
                // DirectML wants sequential execution and no memory pattern
                let dml = (|| -> R<Session> {
                    Session::builder(env)
                        .map_err(oe)?
                        .with_memory_pattern(false)
                        .map_err(oe)?
                        .with_parallel_execution(false)
                        .map_err(oe)?
                        .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
                        .map_err(oe)?
                        .commit_from_file(&path)
                        .map_err(oe)
                })();
                match dml {
                    Ok(x) => sess = Some(x),
                    Err(e) => {
                        crate::note_runtime(format!("DirectML: {e}"));
                        gpu = false;
                    }
                }
            }
            let sess = match sess {
                Some(x) => x,
                None => Session::builder(env)
                    .map_err(oe)?
                    .with_intra_threads(threads.max(1))
                    .map_err(oe)?
                    .commit_from_file(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?,
            };
            sessions.push(sess);
        }
        let mut it = sessions.into_iter();
        let mut next = || it.next().expect("five graphs");
        let (venc, dec, menc, matt, ptpos) = (next(), next(), next(), next(), next());
        Ok(Sam { venc, dec, menc, matt, ptpos, s, tpos, gpu })
    }

    fn embed(&mut self, rgb: &[u8]) -> R<Feats> {
        let s = self.s;
        let (mean, std) = ([0.485f32, 0.456, 0.406], [0.229f32, 0.224, 0.225]);
        let mut px = vec![0f32; 3 * s * s];
        for (i, p) in rgb.chunks_exact(3).enumerate() {
            for c in 0..3 {
                px[c * s * s + i] = (p[c] as f32 / 255.0 - mean[c]) / std[c];
            }
        }
        let v = self.venc.run(ort::inputs!["pixel_values" => Tensor::from_array(([1usize, 3, s, s], px)).map_err(oe)?]).map_err(oe)?;
        Ok(Feats {
            f0: out_f32(&v, "feats0")?,
            f1: out_f32(&v, "feats1")?,
            f2: out_f32(&v, "feats2")?,
            f2nm: out_f32(&v, "feats2_no_mem")?,
            pos: out_f32(&v, "vision_pos_embed")?,
        })
    }

    /// Mask decoder: (high-res logits, object score, object pointer)
    fn decode(&mut self, f: &Feats, f2: &[f32], pts: &[(f32, f32, i32)]) -> R<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        let (s, n) = (self.s, pts.len());
        let r = self
            .dec
            .run(ort::inputs![
                "feats0" => Tensor::from_array(([1usize, 32, s / 4, s / 4], f.f0.clone())).map_err(oe)?,
                "feats1" => Tensor::from_array(([1usize, 64, s / 8, s / 8], f.f1.clone())).map_err(oe)?,
                "feats2_cond" => Tensor::from_array(([1usize, 256, s / 16, s / 16], f2.to_vec())).map_err(oe)?,
                "input_points" => Tensor::from_array(([1usize, 1, n, 2], pts.iter().flat_map(|p| [p.0, p.1]).collect::<Vec<f32>>())).map_err(oe)?,
                "input_labels" => Tensor::from_array(([1usize, 1, n], pts.iter().map(|p| p.2).collect::<Vec<i32>>())).map_err(oe)?,
            ])
            .map_err(oe)?;
        Ok((out_f32(&r, "high_res_mask")?, out_f32(&r, "object_score_logits")?, out_f32(&r, "object_pointer")?))
    }

    /// Memory of a frame: (tokens, positions)
    fn memory(&mut self, f2: &[f32], hi: &[f32], score: &[f32], prompted: bool) -> R<(Vec<f32>, Vec<f32>)> {
        let s = self.s;
        let r = self
            .menc
            .run(ort::inputs![
                "feats2" => Tensor::from_array(([1usize, 256, s / 16, s / 16], f2.to_vec())).map_err(oe)?,
                "high_res_mask" => Tensor::from_array(([1usize, 1, s, s], hi.to_vec())).map_err(oe)?,
                "object_score_logits" => Tensor::from_array(([1usize, 1], score.to_vec())).map_err(oe)?,
                "binarize" => Tensor::from_array((Vec::<usize>::new(), vec![if prompted { 1.0f32 } else { 0.0 }])).map_err(oe)?,
            ])
            .map_err(oe)?;
        Ok((out_f32(&r, "memory_tokens")?, out_f32(&r, "memory_pos")?))
    }

    /// Features of the current frame conditioned on the memory (step `b.n` from the marked frame).
    fn attend(&mut self, f: &Feats, b: &Bank) -> R<Vec<f32>> {
        let hw = (self.s / 16) * (self.s / 16);
        let n = b.n;
        // the marked frame (temporal row 6), recent frames k steps back (row k - 1), padded to 7
        // by repeating the newest; then 16 pointers as 4 tokens of 64 each
        let (ct, cp, cptr) = &b.cond;
        let mut blocks: Vec<(&[f32], &[f32], usize)> = vec![(ct, cp, 6)];
        for (fi, t, p) in &b.recent {
            blocks.push((t, p, n - fi - 1));
        }
        while blocks.len() < MEM_BLOCKS {
            blocks.push(*blocks.last().expect("not empty"));
        }
        let mut diffs = vec![n as f32];
        let mut plist: Vec<&[f32]> = vec![cptr];
        for (fi, p) in b.ptrs.iter().rev() {
            diffs.push((n - fi) as f32);
            plist.push(p);
        }
        while plist.len() < PTRS {
            plist.push(plist.last().copied().expect("not empty"));
            diffs.push(*diffs.last().expect("not empty"));
        }
        // SAM 2 normalizes the pointer distance by max_obj_ptrs_in_encoder - 1
        let norm = (PTRS - 1) as f32;
        let pr = self
            .ptpos
            .run(ort::inputs!["normalized_diffs" => Tensor::from_array(([PTRS], diffs.iter().map(|d| d / norm).collect::<Vec<f32>>())).map_err(oe)?])
            .map_err(oe)?;
        let ppos = out_f32(&pr, "pointer_pos")?;
        drop(pr);
        let tot = MEM_BLOCKS * hw + PTRS * 4;
        let (mut mem, mut mpos) = (Vec::with_capacity(tot * 64), Vec::with_capacity(tot * 64));
        for (t, p, row) in &blocks {
            mem.extend_from_slice(t);
            let tr = &self.tpos[row * 64..row * 64 + 64];
            mpos.extend(p.chunks_exact(64).flat_map(|c| c.iter().zip(tr).map(|(a, b)| a + b)));
        }
        for (k, p) in plist.iter().enumerate() {
            mem.extend_from_slice(p);
            for _ in 0..4 {
                mpos.extend_from_slice(&ppos[k * 64..k * 64 + 64]);
            }
        }
        let r = self
            .matt
            .run(ort::inputs![
                "current_vision_features" => Tensor::from_array(([hw, 1usize, 256], chw_to_seq(&f.f2, 256))).map_err(oe)?,
                "current_vision_position_embeddings" => Tensor::from_array(([hw, 1usize, 256], chw_to_seq(&f.pos, 256))).map_err(oe)?,
                "memory" => Tensor::from_array(([tot, 1usize, 64], mem)).map_err(oe)?,
                "memory_pos" => Tensor::from_array(([tot, 1usize, 64], mpos)).map_err(oe)?,
            ])
            .map_err(oe)?;
        out_f32(&r, "conditioned_feats")
    }

    /// One frame: the marked one (no memory yet: `prompt` builds it) or the next tracked one.
    /// Returns the quantized mask.
    fn step(&mut self, rgb: &[u8], bank: &mut Option<Bank>, prompt: &[(f32, f32, i32)]) -> R<Vec<u8>> {
        let f = self.embed(rgb)?;
        match bank {
            None => {
                let (hi, score, ptr) = self.decode(&f, &f.f2nm, prompt)?;
                let (t, p) = self.memory(&f.f2, &hi, &score, true)?;
                *bank = Some(Bank { cond: (t, p, ptr), n: 1, ..Default::default() });
                Ok(quantize(&hi))
            }
            Some(b) => {
                let f2 = self.attend(&f, b)?;
                let (hi, score, ptr) = self.decode(&f, &f2, &[(0.0, 0.0, -1)])?;
                let (t, p) = self.memory(&f.f2, &hi, &score, false)?;
                let n = b.n;
                b.recent.push((n, t, p));
                if b.recent.len() > MEM_BLOCKS - 1 {
                    b.recent.remove(0);
                }
                b.ptrs.push((n, ptr));
                if b.ptrs.len() > PTRS - 1 {
                    b.ptrs.remove(0);
                }
                b.n += 1;
                Ok(quantize(&hi))
            }
        }
    }
}

// ------------------------------------------------------------------ marking

/// Marking in the player: the frame is read in once, every change of the marks only runs the
/// decoder (milliseconds).
pub struct Live {
    sam: Sam,
    frame: Option<(u64, Feats)>,
}

impl Live {
    /// Load and warm up (the first call on the graphics card builds its shaders and takes seconds).
    pub fn new(models: &Models, threads: usize) -> R<Live> {
        let mut sam = Sam::load(models, threads)?;
        let s = sam.s;
        let f = sam.embed(&vec![128u8; s * s * 3])?;
        sam.decode(&f, &f.f2nm, &[(s as f32 / 2.0, s as f32 / 2.0, 1)])?;
        Ok(Live { sam, frame: None })
    }
    pub fn size(&self) -> usize {
        self.sam.s
    }
    pub fn gpu(&self) -> bool {
        self.sam.gpu
    }
    /// Read in the frame `rgb` (`size`² RGB) unless `key` is already in.
    pub fn set_frame(&mut self, key: u64, rgb: &[u8]) -> R<()> {
        if self.frame.as_ref().is_none_or(|f| f.0 != key) {
            self.frame = None;
            self.frame = Some((key, self.sam.embed(rgb)?));
        }
        Ok(())
    }
    /// Mask of the marks on the read-in frame (`size`², 128 = edge, above = object); `None` without
    /// a frame or usable marks. The same computation as the marked frame in [`track`].
    pub fn mask(&mut self, p: &Prompt) -> R<Option<Vec<u8>>> {
        let Some((_, f)) = &self.frame else { return Ok(None) };
        if !p.usable() {
            return Ok(None);
        }
        let (hi, _, _) = self.sam.decode(f, &f.f2nm, &p.model(self.sam.s))?;
        Ok(Some(quantize(&hi)))
    }
}

/// Source frames `from .. from + count` of the grid as `s`×`s` RGB (stretched as the model wants).
/// Seeks a quarter frame before the frame: the one before is dropped, the wanted one is kept.
/// shortcut: assumes a constant frame rate (variable-rate sources may shift by a frame)
pub fn frames(src: &Path, grid: &Grid, from: usize, count: usize, s: usize, mut each: impl FnMut(usize, &[u8]) -> R<()>) -> R<usize> {
    let ss = (grid.time(from) - 0.25 / grid.fps).max(0.0);
    let mut child = cl_media::ffmpeg_command()
        .map_err(oe)?
        .args(["-v", "error", "-nostdin", "-ss", &format!("{ss:.6}")])
        .arg("-i")
        .arg(src)
        .args(["-an", "-frames:v", &count.to_string(), "-vf", &format!("scale={s}:{s}:flags=bilinear,format=rgb24"), "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(oe)?;
    let mut out = child.stdout.take().expect("piped");
    let mut buf = vec![0u8; s * s * 3];
    let mut n = 0;
    let mut r = Ok(());
    while n < count && out.read_exact(&mut buf).is_ok() {
        r = each(from + n, &buf);
        if r.is_err() {
            break;
        }
        n += 1;
    }
    let _ = child.kill();
    let _ = child.wait();
    r.map(|()| n)
}

// ------------------------------------------------------------------ tracking

/// A tracking job: source, range, marked frame, folders.
pub struct Job {
    pub src: PathBuf,
    pub grid: Grid,
    pub key: usize,
    pub prompt: Prompt,
    /// Section and state files (cache)
    pub work: PathBuf,
    /// Finished mask video
    pub out: PathBuf,
    /// Source size and frame rate (`num/den`) for the mask video
    pub size: (u32, u32),
    pub rate: String,
}

/// State file: frames written so far, then the tracker memory.
fn save_state(path: &Path, done: usize, b: &Bank) -> R<()> {
    let mut v: Vec<u8> = vec![];
    let mut num = |x: usize| v.extend_from_slice(&(x as u64).to_le_bytes());
    num(done);
    num(b.n);
    num(b.recent.len());
    num(b.ptrs.len());
    let arr = |v: &mut Vec<u8>, a: &[f32]| {
        v.extend_from_slice(&(a.len() as u64).to_le_bytes());
        a.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
    };
    arr(&mut v, &b.cond.0);
    arr(&mut v, &b.cond.1);
    arr(&mut v, &b.cond.2);
    for (i, t, p) in &b.recent {
        v.extend_from_slice(&(*i as u64).to_le_bytes());
        arr(&mut v, t);
        arr(&mut v, p);
    }
    for (i, p) in &b.ptrs {
        v.extend_from_slice(&(*i as u64).to_le_bytes());
        arr(&mut v, p);
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, v).map_err(oe)?;
    std::fs::rename(&tmp, path).map_err(oe)
}

fn load_state(path: &Path) -> Option<(usize, Bank)> {
    let d = std::fs::read(path).ok()?;
    let mut at = 0usize;
    let mut num = || -> Option<usize> {
        let b = d.get(at..at + 8)?;
        at += 8;
        Some(u64::from_le_bytes(b.try_into().ok()?) as usize)
    };
    let (done, n, nr, np) = (num()?, num()?, num()?, num()?);
    let arr = |at: &mut usize| -> Option<Vec<f32>> {
        let len = u64::from_le_bytes(d.get(*at..*at + 8)?.try_into().ok()?) as usize;
        *at += 8;
        let bytes = d.get(*at..*at + len.checked_mul(4)?)?;
        *at += len * 4;
        Some(bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    };
    let idx = |at: &mut usize| -> Option<usize> {
        let v = u64::from_le_bytes(d.get(*at..*at + 8)?.try_into().ok()?) as usize;
        *at += 8;
        Some(v)
    };
    let cond = (arr(&mut at)?, arr(&mut at)?, arr(&mut at)?);
    let mut recent = vec![];
    for _ in 0..nr.min(MEM_BLOCKS) {
        recent.push((idx(&mut at)?, arr(&mut at)?, arr(&mut at)?));
    }
    let mut ptrs = vec![];
    for _ in 0..np.min(PTRS) {
        ptrs.push((idx(&mut at)?, arr(&mut at)?));
    }
    (at == d.len()).then_some((done, Bank { cond, recent, ptrs, n }))
}

/// Section file and state of an earlier run: the frames that the state covers (the file is cut
/// back to them; frames written after the last save are computed again). `None` = start anew.
fn resume(raw: &Path, state: &Path, frame: usize) -> Option<(usize, Bank)> {
    let (done, bank) = load_state(state)?;
    let f = std::fs::OpenOptions::new().write(true).open(raw).ok()?;
    let have = f.metadata().ok()?.len() as usize / frame;
    (have >= done).then_some(())?;
    f.set_len((done * frame) as u64).ok()?;
    Some((done, bank))
}

/// Track the object over the job's range (see the module text), then [`join`] the mask video.
/// `pause`: wait while it says so (playback). Resumes from the work folder of an earlier run.
pub fn track(
    models: &Models,
    job: &Job,
    threads: usize,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
    pause: &dyn Fn() -> bool,
) -> R<()> {
    if !job.prompt.usable() {
        return Err("Nichts markiert".into());
    }
    std::fs::create_dir_all(&job.work).map_err(oe)?;
    let sections = plan(&job.grid, job.key);
    let total: usize = sections.iter().map(|s| s.len).sum();
    let mut model: Option<Sam> = None;
    let mut counted = 0;
    for sec in &sections {
        let raw = job.work.join(format!("{}.raw", sec.name()));
        let state = job.work.join(format!("{}.state", sec.name()));
        if model.is_none() {
            model = Some(Sam::load(models, threads)?);
        }
        let sam = model.as_mut().expect("loaded");
        let s = sam.s;
        let frame = s * s;
        let (mut done, mut bank) = match resume(&raw, &state, frame) {
            Some((d, b)) => (d, Some(b)),
            None => {
                let _ = std::fs::remove_file(&raw);
                (0, None)
            }
        };
        if done >= sec.len {
            counted += sec.len;
            continue;
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&raw).map_err(oe)?;
        let prompt = job.prompt.model(s);
        if bank.is_none() {
            let mut first = None;
            frames(&job.src, &job.grid, sec.key, 1, s, |_, rgb| {
                first = Some(sam.step(rgb, &mut bank, &prompt)?);
                Ok(())
            })?;
            let m = first.ok_or("Bild nicht lesbar")?;
            if !sec.back {
                file.write_all(&m).map_err(oe)?;
                done = 1;
            }
            save_state(&state, done, bank.as_ref().expect("set"))?;
        }
        let mut last: Option<Vec<u8>> = None;
        while done < sec.len {
            let count = SAVE_EVERY.min(sec.len - done);
            // forwards: the next frames in order; backwards: the block before, read forwards, used reversed
            let from = if sec.back { sec.frame(done + count - 1) } else { sec.frame(done) };
            let mut block: Vec<Vec<u8>> = vec![];
            frames(&job.src, &job.grid, from, count, s, |_, rgb| {
                block.push(rgb.to_vec());
                Ok(())
            })?;
            if sec.back {
                block.reverse();
            }
            for i in 0..count {
                while pause() && !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err("Abgebrochen".into());
                }
                // the source ended early (rounding at its end): the last mask holds
                let m = match block.get(i) {
                    Some(rgb) => sam.step(rgb, &mut bank, &prompt)?,
                    None => last.clone().unwrap_or_else(|| vec![0; frame]),
                };
                file.write_all(&m).map_err(oe)?;
                last = Some(m);
                progress(((counted + done + i + 1) as f32 / total as f32 * 0.97).min(0.97));
            }
            done += count;
            file.flush().map_err(oe)?;
            save_state(&state, done, bank.as_ref().expect("set"))?;
        }
        counted += sec.len;
    }
    let s = model.as_ref().map(|m| m.s).ok_or("Modell nicht geladen")?;
    join(&job.work, &job.grid, job.key, s, job.size, &job.rate, &job.out)?;
    progress(1.0);
    Ok(())
}

/// Mask video from the section files: frame `i` from the backwards file before the marked frame,
/// from the forwards file from it on; `s`² logits upscaled to the source size (at most
/// [`OUT_MAX`]), FFV1 at `rate`. Written next to `out` and renamed when complete.
pub fn join(work: &Path, grid: &Grid, key: usize, s: usize, size: (u32, u32), rate: &str, out: &Path) -> R<()> {
    let frame = s * s;
    let read = |name: &str| std::fs::read(work.join(format!("{name}.raw"))).unwrap_or_default();
    let (fwd, back) = (read("fwd"), read("back"));
    let lim = OUT_MAX.min(size.0.max(size.1)).max(2);
    let (w, h) = cl_media::fit_size(size.0, size.1, lim, lim);
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(oe)?;
    }
    let tmp = out.with_extension("part.mkv");
    let mut enc = cl_media::ffmpeg_command()
        .map_err(oe)?
        .args(["-v", "error", "-nostdin", "-y", "-f", "rawvideo", "-pix_fmt", "gray", "-s", &format!("{w}x{h}"), "-r", rate, "-i", "-", "-c:v", "ffv1", "-g", "1"])
        .arg(&tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(oe)?;
    let mut writer = enc.stdin.take().expect("piped");
    let result = (|| -> R<()> {
        for i in 0..grid.n {
            let (data, j) = if i < key { (&back, key - 1 - i) } else { (&fwd, i - key) };
            let q = data.get(j * frame..(j + 1) * frame).ok_or("Abschnitt unvollständig")?;
            writer.write_all(&upscale(q, s, w as usize, h as usize)).map_err(oe)?;
        }
        Ok(())
    })();
    drop(writer);
    let ok = enc.wait().map(|s| s.success()).unwrap_or(false);
    crate::matte::finish(&tmp, out, result, ok && grid.n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn grid_at_29_97_fps_starts_on_a_source_frame() {
        let fps = 30000.0 / 1001.0;
        // 2.0 s is between frames 59 (1.968 s) and 60 (2.002 s): the range starts on frame 59
        let g = Grid::new(2.0, 4.0, fps);
        assert!((g.start - 59.0 / fps).abs() < 1e-9, "{}", g.start);
        assert_eq!(g.n, 61, "frames 59..120 cover 2.0..4.0");
        assert!(g.end() >= 4.0 && g.end() - 4.0 < 1.0 / fps);
        assert_eq!((g.index(g.time(7)), g.index(g.time(7) + 0.5 / fps)), (7, 7));
        assert_eq!((g.index(-1.0), g.index(100.0)), (0, g.n - 1));
        // a frame time exactly on the grid stays there (no drift by rounding)
        let g = Grid::new(59.0 / fps, 61.0 / fps, fps);
        assert_eq!((g.n, g.index(60.0 / fps)), (2, 1));
    }

    #[test]
    fn plan_tracks_forwards_and_backwards_from_the_marked_frame() {
        let g = Grid { start: 0.0, fps: 30.0, n: 100 };
        let v = plan(&g, 40);
        assert_eq!(v, vec![Section { key: 40, back: false, len: 60 }, Section { key: 40, back: true, len: 40 }]);
        assert_eq!((v[0].frame(0), v[0].frame(59), v[1].frame(0), v[1].frame(39)), (40, 99, 39, 0));
        // marked on the first frame: only forwards; beyond the end: the last frame
        assert_eq!(plan(&g, 0), vec![Section { key: 0, back: false, len: 100 }]);
        assert_eq!(plan(&g, 500)[0], Section { key: 99, back: false, len: 1 });
        // a still image: one frame
        assert_eq!(plan(&Grid { start: 0.0, fps: 30.0, n: 1 }, 0), vec![Section { key: 0, back: false, len: 1 }]);
    }

    #[test]
    fn prompt_puts_the_frame_first_and_needs_an_object_mark() {
        let p = Prompt { points: vec![(0.5, 0.25, false), (0.1, 0.1, true)], rect: Some([0.8, 0.6, 0.2, 0.4]) };
        assert_eq!(p.model(512), vec![(102.4, 204.8, 2), (409.6, 307.2, 3), (256.0, 128.0, 1), (51.2, 51.2, 0)]);
        assert!(p.usable());
        assert!(!Prompt { points: vec![(0.1, 0.1, true)], rect: None }.usable());
        assert!(!Prompt::default().usable());
    }

    #[test]
    fn upscale_cuts_at_the_zero_logit() {
        // left half object (logit +2 → 160), right half background (-2 → 96)
        let s = 4;
        let q: Vec<u8> = (0..16).map(|i| if i % 4 < 2 { 160 } else { 96 }).collect();
        assert_eq!(quantize(&[2.0, -2.0, 0.0, 100.0, -100.0]), vec![160, 96, 128, 255, 0]);
        let o = upscale(&q, s, 8, 2);
        assert_eq!(&o[..8], &[255, 255, 255, 255, 0, 0, 0, 0]);
    }

    fn bank(seed: f32) -> Bank {
        let v = |n: usize| (0..n).map(|i| seed + i as f32 * 0.5).collect::<Vec<f32>>();
        Bank { cond: (v(6), v(6), v(4)), recent: vec![(1, v(6), v(6)), (2, v(6), v(6))], ptrs: vec![(1, v(4))], n: 3 }
    }

    /// State file round trip; resuming cuts the section file back to the saved frames, a broken
    /// or missing state or too short a file starts anew.
    #[test]
    fn section_file_appends_cuts_back_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let (raw, state) = (dir.path().join("fwd.raw"), dir.path().join("fwd.state"));
        let b = bank(1.25);
        save_state(&state, 2, &b).unwrap();
        assert_eq!(load_state(&state), Some((2, b.clone())));
        // three frames of 4 bytes written, the state covers two: the third goes
        std::fs::write(&raw, [1u8; 12]).unwrap();
        assert_eq!(resume(&raw, &state, 4), Some((2, b.clone())));
        assert_eq!(std::fs::metadata(&raw).unwrap().len(), 8);
        // fewer frames than the state claims, or no file: anew
        std::fs::write(&raw, [1u8; 4]).unwrap();
        assert_eq!(resume(&raw, &state, 4), None);
        assert_eq!(resume(&dir.path().join("none.raw"), &state, 4), None);
        // broken state
        let mut d = std::fs::read(&state).unwrap();
        d.pop();
        std::fs::write(&state, d).unwrap();
        assert_eq!(load_state(&state), None);
    }

    /// `join` from made-up sections: backwards file before the marked frame (stored newest first),
    /// forwards from it; every frame lands at its place in the mask video.
    #[test]
    fn join_puts_the_sections_in_time_order() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (s, n, key) = (8usize, 4usize, 2usize);
        // frame i: object in the left i/n part (logit +4): the share of white tells the index
        let mask = |i: usize| -> Vec<u8> { (0..s * s).map(|p| if (p % s) * n < i * s { 192 } else { 64 }).collect() };
        let fwd: Vec<u8> = (key..n).flat_map(mask).collect();
        let back: Vec<u8> = (0..key).rev().flat_map(mask).collect();
        std::fs::write(dir.path().join("fwd.raw"), fwd).unwrap();
        std::fs::write(dir.path().join("back.raw"), back).unwrap();
        let out = dir.path().join("m.mkv");
        let g = Grid { start: 0.0, fps: 25.0, n };
        join(dir.path(), &g, key, s, (96, 48), "25", &out).unwrap();
        assert!(!dir.path().join("m.part.mkv").exists());
        let m = cl_media::probe(&out).unwrap();
        assert_eq!((m.width, m.height), (96, 48));
        let mut shares = vec![];
        crate::gray_frames(&out, 96, 48, 25.0, 0.0, 10.0, |_, f| {
            shares.push((f.iter().filter(|v| **v > 127).count() as f64 / f.len() as f64 * n as f64).round() as usize);
            true
        })
        .unwrap();
        assert_eq!(shares, (0..n).collect::<Vec<_>>());
        // a missing frame: no file, no part file
        std::fs::write(dir.path().join("fwd.raw"), [0u8; 3]).unwrap();
        let bad = dir.path().join("bad.mkv");
        assert!(join(dir.path(), &g, key, s, (96, 48), "25", &bad).is_err());
        assert!(!bad.exists() && !dir.path().join("bad.part.mkv").exists());
    }

    /// Synthetic clip shared by the model tests (made once per test binary): a block figure of two
    /// colours walks across a background that changes colour, 160×90, 25 fps, 2 s.
    fn clip() -> &'static Path {
        static CLIP: std::sync::OnceLock<(tempfile::TempDir, PathBuf)> = std::sync::OnceLock::new();
        &CLIP.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("walk.mp4");
            let st = cl_media::ffmpeg_command()
                .unwrap()
                .args([
                    "-v", "error", "-y", "-f", "lavfi", "-i",
                    "color=c=0x3060a0:s=160x90:r=25:d=2,hue=h=t*60[bg];\
                     color=c=0xe0c020:s=24x34:r=25:d=2,drawbox=x=0:y=14:w=24:h=20:color=0xc02020:t=fill[fg];\
                     [bg][fg]overlay=x='20+t*50':y=30:shortest=1",
                    "-pix_fmt", "yuv420p",
                ])
                .arg(&p)
                .status()
                .unwrap();
            assert!(st.success());
            (dir, p)
        })
        .1
    }

    /// Ground truth of the figure at time t (pixels of the 160×90 clip)
    fn figure(t: f64) -> (usize, usize, usize, usize) {
        let x = (20.0 + t * 50.0) as usize;
        (x, 30, x + 24, 64)
    }

    fn iou(mask: &[u8], w: usize, b: (usize, usize, usize, usize)) -> f64 {
        let (mut i, mut u) = (0, 0);
        for (k, v) in mask.iter().enumerate() {
            let (x, y) = (k % w, k / w);
            let inside = x >= b.0 && x < b.2 && y >= b.1 && y < b.3;
            i += (inside && *v > 127) as usize;
            u += (inside || *v > 127) as usize;
        }
        i as f64 / u.max(1) as f64
    }

    fn sam_models() -> Option<Models> {
        let m = testutil::models()?;
        if m.sam2().is_none() || m.ort_dll().is_none() || !cl_media::test_tools() {
            eprintln!("skipped (object model or onnxruntime missing)");
            return None;
        }
        // a model folder with the graphics card runtime runs these tests on DirectML
        if let Some(d) = m.gpu_runtime() {
            crate::use_gpu_runtime(d);
        }
        Some(m)
    }

    fn job(dir: &Path, key_t: f64) -> Job {
        let grid = Grid::new(0.0, 2.0, 25.0);
        let key = grid.index(key_t);
        let (x0, y0, x1, y1) = figure(grid.time(key));
        let c = ((x0 + x1) as f32 / 2.0 / 160.0, (y0 + y1) as f32 / 2.0 / 90.0);
        Job {
            src: clip().to_path_buf(),
            grid,
            key,
            prompt: Prompt { points: vec![(c.0, c.1, false), (0.05, 0.1, true)], rect: None },
            work: dir.join("work"),
            out: dir.join("mask.mkv"),
            size: (160, 90),
            rate: "25".into(),
        }
    }

    fn mask_frames(p: &Path) -> Vec<Vec<u8>> {
        let mut v = vec![];
        crate::gray_frames(p, 160, 90, 25.0, 0.0, 10.0, |_, f| {
            v.push(f.to_vec());
            true
        })
        .unwrap();
        v
    }

    /// A click on the figure marks it (the live mask is the first tracked frame); tracking from the
    /// middle follows it through the whole clip, forwards and backwards.
    #[test]
    fn click_marks_the_figure_and_tracking_follows_it() {
        let Some(models) = sam_models() else { return };
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get() / 2).max(1);
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path(), 1.0);
        let t0 = std::time::Instant::now();
        let mut live = Live::new(&models, threads).unwrap();
        println!("load + warm-up {:.2} s, gpu {}", t0.elapsed().as_secs_f64(), live.gpu());
        let s = live.size();
        let mut rgb = vec![];
        frames(&j.src, &j.grid, j.key, 1, s, |_, f| {
            rgb = f.to_vec();
            Ok(())
        })
        .unwrap();
        live.set_frame(1, &rgb).unwrap();
        let t0 = std::time::Instant::now();
        let q = live.mask(&j.prompt).unwrap().expect("a mask");
        println!("click → mask {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
        let first = upscale(&q, s, 160, 90);
        let cover = iou(&first, 160, figure(j.grid.time(j.key)));
        assert!(cover > 0.9, "click covers the figure: IoU {cover:.3}");
        assert!(live.mask(&Prompt { points: vec![(0.5, 0.5, true)], rect: None }).unwrap().is_none());
        drop(live);
        let t0 = std::time::Instant::now();
        track(&models, &j, threads, &|_| {}, &AtomicBool::new(false), &|| false).unwrap();
        let secs = t0.elapsed().as_secs_f64();
        println!("tracked {} frames in {secs:.1} s ({:.0} ms per frame)", j.grid.n, secs * 1000.0 / j.grid.n as f64);
        let frames = mask_frames(&j.out);
        assert_eq!(frames.len(), j.grid.n);
        assert_eq!(frames[j.key], first, "live mask = the marked frame of the track");
        let low: Vec<(usize, f64)> =
            frames.iter().enumerate().map(|(i, f)| (i, iou(f, 160, figure(j.grid.time(i))))).filter(|(_, v)| *v < 0.8).collect();
        assert!(low.is_empty(), "frames losing the figure: {low:?}");
    }

    /// Cancelled in the middle and started again: the same mask video as in one go.
    #[test]
    fn cancel_and_resume_give_the_same_mask() {
        let Some(models) = sam_models() else { return };
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get() / 2).max(1);
        let dir = tempfile::tempdir().unwrap();
        let whole = job(&dir.path().join("a"), 0.0);
        track(&models, &whole, threads, &|_| {}, &AtomicBool::new(false), &|| false).unwrap();
        let parts = job(&dir.path().join("b"), 0.0);
        let stop = AtomicBool::new(false);
        // cancel inside the second block (frames 31–49)
        let r = track(&models, &parts, threads, &|p| stop.store(p > 0.7, Ordering::Relaxed), &stop, &|| false);
        assert_eq!(r, Err("Abgebrochen".into()));
        assert!(!parts.out.exists());
        let saved = load_state(&parts.work.join("fwd.state")).expect("state saved").0;
        assert!(saved >= SAVE_EVERY, "resumes after the first block: {saved}");
        track(&models, &parts, threads, &|_| {}, &AtomicBool::new(false), &|| false).unwrap();
        let (a, b) = (mask_frames(&whole.out), mask_frames(&parts.out));
        assert_eq!(a.len(), b.len());
        let diff: usize = a.iter().zip(&b).map(|(x, y)| x.iter().zip(y).filter(|(p, q)| p != q).count()).sum();
        // the same computation on the same inputs; a few edge pixels may differ by thread scheduling
        assert!(diff * 1000 < a.len() * 160 * 90, "{diff} pixels differ");
    }
}
