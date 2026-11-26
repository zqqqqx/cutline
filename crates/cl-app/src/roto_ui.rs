//! Object cut-out in the app: marks on the clip (`Clip::roto`), the marking tool in the player
//! with a live mask from a worker thread, and the tracking task that writes the mask file.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use cl_analyze::roto::{Grid, Prompt};
use cl_core::*;
use eframe::egui::{self, Color32, Rect, Stroke};

use crate::app::App;
use crate::player_edit::Region;
use crate::pro::Pick;
use crate::tasks::TaskDone;
use crate::theme;

/// Part of every mask name: a new model or export gives new files
const MODEL_TAG: &str = "sam2.1-tiny-512 v1";

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}
const FNV0: u64 = 0xcbf29ce484222325;

fn is_video(c: &Clip) -> bool {
    matches!(c.content, ClipContent::Media { kind: MediaKind::Video, .. })
}

/// Half the cores: tracking runs beside playback and editing
fn threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get() / 2).max(1)
}

/// Tracker grid: the clip's source range at the source rate, or the one frame of an image.
pub(crate) fn roto_grid(c: &Clip, m: &MediaRef) -> Grid {
    if !is_video(c) {
        return Grid { start: 0.0, fps: 25.0, n: 1 };
    }
    let fps = m.fps.map(|f| f.as_f64()).filter(|f| *f > 0.0).unwrap_or(30.0);
    let (a, b) = (c.source_time(c.start), c.source_time(c.end()));
    Grid::new(a.min(b).as_secs_f64(), a.max(b).as_secs_f64(), fps)
}

pub(crate) fn prompt_of(k: &RotoKey) -> Prompt {
    Prompt { points: k.points.iter().map(|p| (p.x, p.y, p.remove)).collect(), rect: k.rect }
}

/// What tracking the clip's marks needs: grid, marked frame, mask file, work folder, range.
pub(crate) struct RotoPlan {
    pub grid: Grid,
    pub key: usize,
    pub prompt: Prompt,
    pub out: PathBuf,
    pub work: PathBuf,
    pub span: Option<MatteSpan>,
}

/// Plan for the clip's marks; `None` without usable marks. The mask file is named by everything
/// its content depends on (FNV: stable across Rust versions): same marks, same file.
pub(crate) fn roto_plan(c: &Clip, m: &MediaRef, generated: &Path, cache: &Path) -> Option<RotoPlan> {
    let k = c.roto.as_ref()?.keys.first()?;
    let prompt = prompt_of(k);
    prompt.usable().then_some(())?;
    let grid = roto_grid(c, m);
    let key = grid.index(k.t.as_secs_f64());
    let mut h = fnv(FNV0, MODEL_TAG.as_bytes());
    h = fnv(h, m.path.to_string_lossy().as_bytes());
    if let Ok(md) = std::fs::metadata(&m.path) {
        h = fnv(h, &md.len().to_le_bytes());
        let secs = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        h = fnv(h, &secs.to_le_bytes());
    }
    for v in [grid.start.to_bits(), grid.fps.to_bits(), grid.n as u64, key as u64] {
        h = fnv(h, &v.to_le_bytes());
    }
    h = fnv(h, serde_json::to_string(k).unwrap_or_default().as_bytes());
    let name = format!("{h:016x}");
    let span = is_video(c).then(|| MatteSpan { start: Time::from_secs_f64(grid.start), end: Time::from_secs_f64(grid.end()) });
    Some(RotoPlan { grid, key, prompt, out: generated.join("roto").join(format!("{name}.mkv")), work: cache.join("roto").join(name), span })
}

/// A marking gesture in source fractions
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Mark {
    /// (x, y, background)
    Point(f32, f32, bool),
    Rect([f32; 4]),
}

/// The clip after a marking gesture at source time `at` (one undo step): the mark goes to the frame
/// there (one marked frame: marking another frame starts there anew), `fx.cutout` names the mask
/// file the marks will give, before it exists.
pub(crate) fn roto_mark(c: &Clip, m: &MediaRef, at: Time, mark: Mark, generated: &Path, cache: &Path) -> Clip {
    let mut c = c.clone();
    let grid = roto_grid(&c, m);
    let i = grid.index(at.as_secs_f64());
    let roto = c.roto.get_or_insert_with(Roto::default);
    if roto.keys.first().is_none_or(|k| grid.index(k.t.as_secs_f64()) != i) {
        roto.keys = vec![RotoKey { t: Time::from_secs_f64(grid.time(i)), ..Default::default() }];
    }
    match mark {
        Mark::Point(x, y, remove) => roto.keys[0].points.push(RotoPoint { x, y, remove }),
        Mark::Rect(r) => roto.keys[0].rect = Some(r),
    }
    let plan = roto_plan(&c, m, generated, cache);
    c.fx.cutout = plan.as_ref().map(|p| p.out.to_string_lossy().into_owned());
    c.fx.cutout_span = plan.and_then(|p| p.span);
    c
}

/// Graphics card runtime of the model folder, if both libraries match the catalog's checksums: then
/// the runtime is loaded from there (unless one is loaded already). Files changed since the download
/// are not used (status line instead).
pub(crate) fn check_gpu_runtime(m: &cl_analyze::Models) {
    let (Some(dir), Some(info)) = (m.gpu_runtime(), cl_analyze::CATALOG.iter().find(|c| c.id == "directml")) else { return };
    let intact = info.files.iter().filter(|(f, _)| f.ends_with(".dll")).all(|(f, url)| {
        let want = url.rsplit_once("#sha256=").map(|x| x.1);
        std::fs::read(m.dir.join(f)).is_ok_and(|d| want == Some(cl_update::sha256_hex(&d).as_str()))
    });
    if intact {
        cl_analyze::use_gpu_runtime(dir);
    } else {
        cl_analyze::note_runtime("Dateien beschädigt".into());
    }
}

/// Display fractions (mirrored as shown) → source fractions
fn unflip(c: &Clip, (u, v): (f32, f32)) -> (f32, f32) {
    (if c.fx.flip_h { 1.0 - u } else { u }, if c.fx.flip_v { 1.0 - v } else { v })
}

// ------------------------------------------------------------------ live mask

struct Want {
    tag: u64,
    src: PathBuf,
    grid: Grid,
    index: usize,
    prompt: Prompt,
}

enum Got {
    Ready,
    Mask { tag: u64, size: usize, mask: Option<Vec<u8>> },
    Failed(String),
}

/// The marking tool's model in a worker thread (loaded and warmed up on entering the tool). The
/// UI asks for the mask of the current marks; the worker answers the newest question only.
pub struct RotoLive {
    pub clip: ClipId,
    tx: Sender<Want>,
    rx: Receiver<Got>,
    sent: u64,
    ready: bool,
    /// (question, mask texture) shown
    shown: Option<(u64, Option<egui::TextureHandle>)>,
}

impl RotoLive {
    fn start(ctx: &egui::Context, clip: ClipId, models: cl_analyze::Models) -> Self {
        let (tx, wrx) = channel::<Want>();
        let (wtx, rx) = channel::<Got>();
        let ctx2 = ctx.clone();
        std::thread::Builder::new()
            .name("cl-roto".into())
            .spawn(move || {
                let mut live = match cl_analyze::roto::Live::new(&models, threads()) {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = wtx.send(Got::Failed(e));
                        ctx2.request_repaint();
                        return;
                    }
                };
                let _ = wtx.send(Got::Ready);
                ctx2.request_repaint();
                while let Ok(mut w) = wrx.recv() {
                    while let Ok(n) = wrx.try_recv() {
                        w = n;
                    }
                    let s = live.size();
                    let key = fnv(fnv(FNV0, w.src.to_string_lossy().as_bytes()), &[w.grid.start.to_bits(), w.index as u64].map(u64::to_le_bytes).concat());
                    let mut rgb = None;
                    let got = cl_analyze::roto::frames(&w.src, &w.grid, w.index, 1, s, |_, f| {
                        rgb = Some(f.to_vec());
                        Ok(())
                    })
                    .and_then(|_| rgb.ok_or_else(|| "Bild nicht lesbar".to_string()))
                    .and_then(|f| live.set_frame(key, &f))
                    .and_then(|()| live.mask(&w.prompt));
                    let _ = wtx.send(match got {
                        Ok(mask) => Got::Mask { tag: w.tag, size: s, mask },
                        Err(e) => Got::Failed(e),
                    });
                    ctx2.request_repaint();
                }
            })
            .expect("roto thread");
        Self { clip, tx, rx, sent: 0, ready: false, shown: None }
    }

    /// Answers from the worker: texture of the newest mask; an error ends the tool with a message.
    fn poll(&mut self, ctx: &egui::Context) -> Option<String> {
        while let Ok(g) = self.rx.try_recv() {
            match g {
                Got::Ready => self.ready = true,
                Got::Failed(e) => return Some(e),
                Got::Mask { tag, size, mask } => {
                    let tex = mask.map(|q| {
                        let a = theme::ACCENT();
                        let px: Vec<Color32> = q.iter().map(|v| if *v > 128 { Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 110) } else { Color32::TRANSPARENT }).collect();
                        let img = egui::ColorImage { size: [size, size], pixels: px };
                        ctx.load_texture("roto-live", img, egui::TextureOptions::LINEAR)
                    });
                    self.shown = Some((tag, tex));
                }
            }
        }
        None
    }

    fn want(&mut self, w: Want) {
        if self.ready && w.tag != self.sent {
            self.sent = w.tag;
            let _ = self.tx.send(w);
        }
    }
}

/// Mask texture over the clip as shown (rotation, mirroring)
fn paint_mask(painter: &egui::Painter, r: &Region, c: &Clip, tex: egui::TextureId) {
    let (u0, u1) = if c.fx.flip_h { (1.0, 0.0) } else { (0.0, 1.0) };
    let (v0, v1) = if c.fx.flip_v { (1.0, 0.0) } else { (0.0, 1.0) };
    let mut mesh = egui::Mesh::with_texture(tex);
    mesh.add_rect_with_uv(Rect::from_center_size(r.center, r.size), Rect::from_min_max(egui::pos2(u0, v0), egui::pos2(u1, v1)), Color32::WHITE);
    mesh.rotate(egui::emath::Rot2::from_angle(r.angle), r.center);
    painter.add(mesh);
}

impl App {
    /// Enter the marking tool for `clip` (model loads and warms up in the background).
    pub(crate) fn roto_enter(&mut self, ctx: &egui::Context, clip: ClipId) {
        if self.roto_live.as_ref().is_none_or(|l| l.clip != clip) {
            self.roto_live = Some(RotoLive::start(ctx, clip, self.models()));
        }
        self.pick = Some(Pick::Roto(clip));
        self.roto_tool = Some(clip);
    }

    /// Leave the tool: the worker ends and tracking starts.
    pub(crate) fn roto_leave(&mut self, ctx: &egui::Context) {
        let clip = self.roto_tool.take().or(match self.pick {
            Some(Pick::Roto(c)) => Some(c),
            _ => None,
        });
        if matches!(self.pick, Some(Pick::Roto(_))) {
            self.pick = None;
        }
        self.roto_live = None;
        if let Some(c) = clip {
            self.start_roto(ctx, c);
        }
    }

    /// However the tool ends – Esc (the global shortcut clears `pick` before the player sees the
    /// key), another tool, another clip selected, another inspector tab – tracking starts as on
    /// leaving. Runs every frame and right where Esc clears the tool.
    pub(crate) fn roto_watch(&mut self, ctx: &egui::Context) {
        if let Some(Pick::Roto(c)) = self.pick {
            if self.selected != Some(c) || self.clip_tab != crate::app::ClipTab::Smart {
                log::info!(target: "objekt", "Markieren beendet (Auswahl oder Reiter gewechselt)");
                self.roto_leave(ctx);
            }
            return;
        }
        if self.roto_tool.is_some() {
            log::info!(target: "objekt", "Markieren beendet");
            self.roto_leave(ctx);
        }
    }

    /// Track the clip's marks in the background unless the mask is there or being made. A clip
    /// trimmed since gets its new mask name first (one undo step).
    pub(crate) fn start_roto(&mut self, ctx: &egui::Context, clip: ClipId) {
        let skip = |why: &str| log::info!(target: "objekt", "Verfolgen nicht gestartet: {why}");
        let Some(c) = self.clip_view(clip) else { return skip("Clip nicht mehr da") };
        let ClipContent::Media { media, .. } = c.content else { return skip("kein Medienclip") };
        let Some(m) = self.project.media(media).cloned() else { return skip("Medium fehlt im Projekt") };
        let (gen, cache) = (crate::paths::generated_dir(), crate::paths::data_dir().join("cache"));
        // no object point or frame: nothing to track (the inspector shows no mask status then)
        let Some(plan) = roto_plan(&c, &m, &gen, &cache) else { return skip("nichts markiert") };
        if self.tasks.running(&format!("roto:{clip}")).is_some() {
            return skip("läuft schon");
        }
        let out = plan.out.to_string_lossy().into_owned();
        if c.fx.cutout.as_deref() != Some(out.as_str()) || c.fx.cutout_span != plan.span {
            let mut c = c.clone();
            (c.fx.cutout, c.fx.cutout_span) = (Some(out), plan.span);
            self.live = None;
            self.apply(EditOp::ReplaceClip { clip: c });
        }
        if plan.out.is_file() {
            self.masks_changed();
            return skip("Maske schon vorhanden");
        }
        // model or runtime missing: message and the model settings instead of a task that fails later
        if !self.need_models(&["onnxruntime", "sam2"]) {
            return skip("Modell fehlt");
        }
        let models = self.models();
        log::info!(target: "objekt", "Verfolgen: {} Bilder ab Bild {}", plan.grid.n, plan.key);
        let job = cl_analyze::roto::Job {
            src: m.path.clone(),
            grid: plan.grid,
            key: plan.key,
            prompt: plan.prompt,
            work: plan.work,
            out: plan.out,
            size: (m.width, m.height),
            rate: m.fps.filter(|f| f.num > 0 && f.den > 0 && is_video(&c)).map_or("25".into(), |f| format!("{}/{}", f.num, f.den)),
        };
        self.tasks.spawn(ctx, format!("roto:{clip}"), "Objekt wird verfolgt …", move |p, cancel| {
            match cl_analyze::roto::track(&models, &job, threads(), p, cancel, &crate::proxy::playing) {
                Ok(()) => TaskDone::Run(Box::new(|app: &mut App| {
                    app.masks_changed();
                    app.toast("Objekt freigestellt");
                })),
                Err(e) => TaskDone::Failed(format!("Objekt: {e}")),
            }
        });
    }

    /// The tool in the player: mask preview under the pointer, click adds, Alt or right click
    /// marks background, dragging draws a frame. Every gesture is one undo step.
    pub(crate) fn roto_pick(&mut self, ui: &egui::Ui, resp: &egui::Response, regions: &[Region], clip: ClipId) {
        let ctx = ui.ctx().clone();
        if let Some(e) = self.roto_live.as_mut().and_then(|l| l.poll(&ctx)) {
            (self.roto_live, self.pick, self.roto_tool) = (None, None, None);
            self.toast_err(format!("Objekt: {e}"));
            return;
        }
        let (Some(c), Some(reg)) = (self.clip_view(clip), regions.iter().find(|r| r.clip == clip).copied()) else { return };
        let ClipContent::Media { media, .. } = c.content else { return };
        let Some(m) = self.project.media(media).cloned() else { return };
        let painter = ui.painter_at(resp.rect);
        let at = c.source_time(self.playhead.clamp(c.start, c.end()));
        let alt = ui.input(|i| i.modifiers.alt);
        let inside = |p: egui::Pos2| {
            let (u, v) = reg.local(p);
            ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then(|| unflip(&c, (u, v)))
        };
        let (gen, cache) = (crate::paths::generated_dir(), crate::paths::data_dir().join("cache"));
        let mut mark = None;
        if resp.drag_started_by(egui::PointerButton::Primary) {
            self.pick_origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        }
        let cur = ui.input(|i| i.pointer.latest_pos());
        if let (Some(a), Some(b)) = (self.pick_origin, cur) {
            if resp.dragged_by(egui::PointerButton::Primary) || resp.drag_stopped() {
                let r = Rect::from_two_pos(a, b);
                painter.rect_stroke(r, 2.0, Stroke::new(1.5_f32, theme::ACCENT()), egui::StrokeKind::Inside);
            }
            if resp.drag_stopped() {
                self.pick_origin = None;
                let (u0, v0) = unflip(&c, reg.local(a));
                let (u1, v1) = unflip(&c, reg.local(b));
                let cl = |v: f32| v.clamp(0.0, 1.0);
                mark = Some(Mark::Rect([cl(u0.min(u1)), cl(v0.min(v1)), cl(u0.max(u1)), cl(v0.max(v1))]));
            }
        }
        let click = resp.interact_pointer_pos().and_then(&inside);
        if resp.clicked() {
            mark = click.map(|(x, y)| Mark::Point(x, y, alt));
        } else if resp.secondary_clicked() {
            mark = click.map(|(x, y)| Mark::Point(x, y, true));
        }
        let mut shown = c.clone();
        if let Some(mk) = mark {
            if let Some(t) = self.tasks.running(&format!("roto:{clip}")) {
                t.cancel();
            }
            shown = roto_mark(&c, &m, at, mk, &gen, &cache);
            self.live = None;
            self.apply(EditOp::ReplaceClip { clip: shown.clone() });
        }
        // live mask: the marks on this frame plus the point under the pointer
        let grid = roto_grid(&shown, &m);
        let index = grid.index(at.as_secs_f64());
        let mut prompt = shown
            .roto
            .as_ref()
            .and_then(|r| r.keys.first())
            .filter(|k| grid.index(k.t.as_secs_f64()) == index)
            .map(prompt_of)
            .unwrap_or_default();
        if let Some((x, y)) = resp.hover_pos().filter(|_| self.pick_origin.is_none()).and_then(&inside) {
            prompt.points.push((x, y, alt));
        }
        let tag = fnv(fnv(FNV0, m.path.to_string_lossy().as_bytes()), format!("{:?}{index}{prompt:?}", grid.start).as_bytes()) | 1;
        if let Some(l) = self.roto_live.as_mut() {
            l.want(Want { tag, src: m.path.clone(), grid, index, prompt });
            if let Some((_, Some(tex))) = &l.shown {
                paint_mask(&painter, &reg, &shown, tex.id());
            }
        }
        // marked points: object filled, background hollow
        if let Some(k) = shown.roto.as_ref().and_then(|r| r.keys.first()).filter(|k| grid.index(k.t.as_secs_f64()) == index) {
            let flip = |x: f32, y: f32| unflip(&shown, (x, y));
            let screen = |(u, v): (f32, f32)| reg.corner(2.0 * u - 1.0, 2.0 * v - 1.0);
            for p in &k.points {
                let at = screen(flip(p.x, p.y));
                if p.remove {
                    painter.circle_stroke(at, 4.0, Stroke::new(2.0_f32, theme::DANGER_TEXT()));
                } else {
                    painter.circle_filled(at, 4.0, theme::ACCENT());
                }
            }
            if let Some([l, t, r, b]) = k.rect {
                let pts = [(l, t), (r, t), (r, b), (l, b), (l, t)].map(|(x, y)| screen(flip(x, y)));
                painter.add(egui::Shape::line(pts.to_vec(), Stroke::new(1.5_f32, theme::ACCENT())));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(dir: &Path) -> (Clip, MediaRef) {
        let path = dir.join("v.mp4");
        std::fs::write(&path, b"video").unwrap();
        let m = MediaRef {
            id: MediaId::new(),
            path,
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(10.0),
            width: 160,
            height: 90,
            fps: Some(Rational::new(30000, 1001)),
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
            has_alpha: false,
        };
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::from_secs_f64(5.0), Time::from_secs_f64(2.0));
        c.source_in = Time::from_secs_f64(2.0);
        (c, m)
    }

    /// Gestures add to the marked frame of the playhead; the mask name follows the marks and
    /// stays the same for the same marks; marking another frame starts there; background
    /// points alone name no mask.
    #[test]
    fn marks_name_a_stable_mask_file_and_start_over_on_another_frame() {
        let dir = tempfile::tempdir().unwrap();
        let (c, m) = video(dir.path());
        let (gen, cache) = (dir.path().join("gen"), dir.path().join("cache"));
        let at = Time::from_secs_f64(2.5);
        let bg = roto_mark(&c, &m, at, Mark::Point(0.9, 0.9, true), &gen, &cache);
        assert_eq!((bg.fx.cutout.as_ref(), bg.fx.cutout_span), (None, None), "background only: nothing to cut out");
        let one = roto_mark(&bg, &m, at, Mark::Point(0.5, 0.5, false), &gen, &cache);
        let k = &one.roto.as_ref().unwrap().keys;
        assert_eq!((k.len(), k[0].points.len()), (1, 2));
        let out = one.fx.cutout.clone().expect("a mask name");
        assert!(out.starts_with(&gen.join("roto").to_string_lossy().into_owned()) && out.ends_with(".mkv"), "{out}");
        // the same marks again (also on a copy of the clip): the same name
        assert_eq!(roto_mark(&bg, &m, at, Mark::Point(0.5, 0.5, false), &gen, &cache).fx.cutout, Some(out.clone()));
        // a frame changes it; the range is the clip's source range on the 29.97 grid
        let framed = roto_mark(&one, &m, at, Mark::Rect([0.1, 0.1, 0.6, 0.7]), &gen, &cache);
        assert_ne!(framed.fx.cutout, Some(out));
        let span = framed.fx.cutout_span.unwrap();
        assert!(span.start <= Time::from_secs_f64(2.0) && span.end >= Time::from_secs_f64(4.0) && span.end < Time::from_secs_f64(4.04), "{span:?}");
        // another frame of the clip: marks start there anew
        let other = roto_mark(&framed, &m, Time::from_secs_f64(3.5), Mark::Point(0.2, 0.2, false), &gen, &cache);
        let k = &other.roto.as_ref().unwrap().keys;
        assert_eq!((k.len(), k[0].points.len(), k[0].rect), (1, 1, None));
        let grid = roto_grid(&other, &m);
        assert_eq!(grid.index(k[0].t.as_secs_f64()), grid.index(3.5));
        // a changed source file: a new mask
        std::fs::write(&m.path, b"another video").unwrap();
        assert_ne!(roto_mark(&bg, &m, at, Mark::Point(0.5, 0.5, false), &gen, &cache).fx.cutout, one.fx.cutout);
    }

    /// Bug (window test): after Esc in the marking tool tracking never started and nothing said
    /// why – the global Esc shortcut cleared the tool before the player saw the key. Driven like the
    /// window: whole app frames, Esc as a key event. Leaving by another clip, another tab or another
    /// tool starts it too. Without the model the user gets the "needs" message instead of nothing.
    #[test]
    fn leaving_the_tool_starts_tracking_or_says_why() {
        if !cl_media::test_tools() {
            return;
        }
        let ctx = egui::Context::default();
        let mut t = crate::saving::tests::test_app_with(ctx.clone());
        let dir = t.dir().to_path_buf();
        let v = dir.join("v.mp4");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=160x90:r=25:d=1", "-pix_fmt", "yuv420p"])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let m = cl_media::probe(&v).unwrap();
        let mut p = Project::new("Objekt");
        let mut tr = Track::new(TrackKind::Video);
        let (a, b) = (
            Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(0.5)),
            Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::from_secs_f64(0.5), Time::from_secs_f64(0.5)),
        );
        let (ida, idb) = (a.id, b.id);
        tr.main = true;
        tr.clips = vec![a, b];
        p.timeline.tracks.push(tr);
        p.media.push(m.clone());
        t.set_doc(p);
        t.screen = crate::app::Screen::Editor;
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))), events, ..Default::default() };
            let _ = ctx.run(input, |ctx| eframe::App::update(app, ctx, &mut frame));
        };
        let esc = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        // the tool on clip `id`, one object point marked as a click does (the worker plays no part)
        let enter = |app: &mut App, id: ClipId| {
            (app.selected, app.clip_tab) = (Some(id), crate::app::ClipTab::Smart);
            app.roto_enter(&ctx, id);
            app.roto_live = None;
            let c = app.clip_view(id).unwrap();
            let marked = roto_mark(&c, &m, c.source_time(c.start), Mark::Point(0.5, 0.5, false), &crate::paths::generated_dir(), &dir);
            app.apply(EditOp::ReplaceClip { clip: marked });
            app.toasts.clear();
            app.settings_open = false;
        };
        // started, or the model is missing and the user is told so
        let started = |app: &App, id: ClipId| {
            app.tasks.running(&format!("roto:{id}")).is_some() || app.toasts.iter().any(|t| t.0.contains("SAM 2.1 Tiny"))
        };
        let tool = |app: &App| matches!(app.pick, Some(Pick::Roto(_)));
        enter(&mut t, ida);
        run(&mut t, vec![]);
        assert!(tool(&t) && !started(&t, ida), "the tool stays open while marking");
        run(&mut t, vec![esc.clone()]);
        assert!(!tool(&t), "Esc leaves the tool");
        assert!(started(&t, ida), "Esc: no task and no message; toasts {:?}", t.toasts);
        // another clip selected
        enter(&mut t, idb);
        t.selected = Some(ida);
        run(&mut t, vec![]);
        assert!(!tool(&t) && started(&t, idb), "selecting another clip; toasts {:?}", t.toasts);
        // another inspector tab
        enter(&mut t, idb);
        t.clip_tab = crate::app::ClipTab::Basic;
        run(&mut t, vec![]);
        assert!(!tool(&t) && started(&t, idb), "another tab; toasts {:?}", t.toasts);
        // another tool takes over: it stays, tracking starts
        enter(&mut t, idb);
        t.pick = Some(Pick::Chroma(idb));
        run(&mut t, vec![]);
        assert!(t.pick == Some(Pick::Chroma(idb)) && started(&t, idb), "another tool; toasts {:?}", t.toasts);
    }

    /// Graphics card files that do not match the catalog are not used: a status line instead.
    #[test]
    fn damaged_gpu_runtime_is_not_used() {
        if !cfg!(windows) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("directml")).unwrap();
        for f in ["onnxruntime.dll", "DirectML.dll"] {
            std::fs::write(dir.path().join("directml").join(f), b"not a library").unwrap();
        }
        let m = cl_analyze::Models::new(dir.path());
        assert!(m.installed("directml"));
        check_gpu_runtime(&m);
        assert_eq!(cl_analyze::runtime_note().as_deref(), Some("Dateien beschädigt"));
        assert!(!cl_analyze::gpu_runtime_loaded());
    }

    /// A still image: one frame, no range (the export holds its one mask frame).
    #[test]
    fn still_image_marks_one_frame_without_range() {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, mut m) = video(dir.path());
        (m.kind, c.content) = (MediaKind::Image, ClipContent::Media { media: m.id, kind: MediaKind::Image });
        let c = roto_mark(&c, &m, Time::from_secs_f64(3.0), Mark::Point(0.5, 0.5, false), dir.path(), dir.path());
        assert!(c.fx.cutout.is_some() && c.fx.cutout_span.is_none());
        let p = roto_plan(&c, &m, dir.path(), dir.path()).unwrap();
        assert_eq!((p.grid.n, p.key), (1, 0));
    }
}
