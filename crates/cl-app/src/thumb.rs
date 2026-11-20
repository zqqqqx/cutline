//! Thumbnail tool: frame of a position at 1280×720 with a cut-out person, outline and text.
//! The settings produce a small helper project that the export renders as PNG –
//! preview and file come from the same path. The open project stays unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cl_core::*;
use eframe::egui::{self, vec2, Sense};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::tasks::TaskDone;
use crate::theme;
use crate::widgets::{self, Slider};

pub const W: u32 = 1280;
pub const H: u32 = 720;
/// Distance of the text from the frame edge
pub const MARGIN: f32 = 48.0;
const FPS: u32 = 30;

/// Font family → (file, index), like `App::font_files`
pub type Fonts = HashMap<String, (PathBuf, u32)>;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Bg {
    Original,
    Blur,
    Dark,
    Color,
}

const BG_NAMES: [(Bg, &str); 4] =
    [(Bg::Original, "Original"), (Bg::Blur, "Weichgezeichnet"), (Bg::Dark, "Abgedunkelt"), (Bg::Color, "Farbe")];

/// Settings without text and position; kept as last used values.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Opts {
    pub person: bool,
    pub bg: Bg,
    pub bg_color: [u8; 3],
    pub outline: bool,
    pub outline_color: [u8; 3],
    pub outline_width: f32,
    pub template: String,
    pub size: f32,
    /// Grid cell 0…8, row by row from top left
    pub pos: u8,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            person: false,
            bg: Bg::Blur,
            bg_color: [24, 120, 255],
            outline: true,
            outline_color: [255, 255, 255],
            outline_width: 0.4,
            template: "thumbnail".into(),
            size: 120.0,
            pos: 6,
        }
    }
}

/// What is rendered: project state (`App::revision`), position, settings, text
#[derive(Clone, PartialEq, Debug)]
pub struct Want {
    pub rev: u64,
    pub at: Time,
    pub opts: Opts,
    pub text: String,
}

/// Text templates, „Thumbnail“ first
pub fn templates() -> Vec<&'static cl_fx::text::TextTemplate> {
    let mut v: Vec<_> = cl_fx::text::TEMPLATES.iter().collect();
    v.sort_by_key(|t| t.id != "thumbnail" && t.category != "Thumbnail");
    v
}

fn template(id: &str) -> &'static cl_fx::text::TextTemplate {
    let all = templates();
    all.iter().find(|t| t.id == id).copied().unwrap_or(all[0])
}

/// Size of the still: fitted into 1280×720 (even edges)
fn contain(w: u32, h: u32) -> (u32, u32) {
    let k = (W as f64 / w.max(1) as f64).min(H as f64 / h.max(1) as f64);
    let even = |v: u32| ((v as f64 * k).round() as u32 + 1).max(2) & !1;
    (even(w), even(h))
}

/// Settings → helper project 1280×720, two frames long. Layers from the bottom: fill (other
/// aspect ratio) or color area, image, person, text. `frame`: still as video, `matte`:
/// its mask (without one no person), `color`: color area for the „Farbe“ background,
/// `measure`: width and height of a text in pixels.
pub fn project(
    o: &Opts,
    text: &str,
    frame: &MediaRef,
    matte: Option<&Path>,
    color: Option<&MediaRef>,
    measure: &dyn Fn(&TextStyle) -> (f32, f32),
) -> Project {
    let mut p = Project::new("Thumbnail");
    (p.settings.width, p.settings.height) = (W, H);
    p.settings.fps = Rational::new(FPS, 1);
    let len = Time::from_secs_f64(2.0 / FPS as f64);
    let clip = |m: &MediaRef, fill: bool| {
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: m.kind }, Time::ZERO, len);
        crate::shorts::fit_clip(&mut c, m, W as f32, H as f32, fill);
        c
    };
    let person = matte.filter(|_| o.person);
    let mut look = ClipFx::default();
    match o.bg {
        Bg::Blur if person.is_some() => look.effects.push(FxRef::new("blur")),
        Bg::Dark if person.is_some() => (look.adjust.brightness, look.adjust.saturation) = (-1.0, -0.3),
        _ => {}
    }
    let mut layers: Vec<(TrackKind, Clip)> = Vec::new();
    match color.filter(|_| person.is_some() && o.bg == Bg::Color) {
        Some(c) => layers.push((TrackKind::Video, clip(c, true))),
        None => {
            // other aspect ratio: filling, blurred copy against borders
            if (frame.width as f32 / frame.height.max(1) as f32 - W as f32 / H as f32).abs() > 0.01 {
                let mut c = clip(frame, true);
                c.fx = look.clone();
                if !c.fx.effects.iter().any(|e| e.id == "blur") {
                    c.fx.effects.push(FxRef::new("blur"));
                }
                layers.push((TrackKind::Video, c));
            }
            let mut c = clip(frame, false);
            c.fx = look;
            layers.push((TrackKind::Video, c));
        }
    }
    if let Some(m) = person {
        let mut c = clip(frame, false);
        c.fx.cutout = Some(m.to_string_lossy().into_owned());
        c.fx.person = o.outline.then_some(PersonFx::Outline {
            color: o.outline_color,
            width: o.outline_width,
            transparent: true,
        });
        layers.push((TrackKind::Video, c));
    }
    if !text.trim().is_empty() {
        let mut st = template(&o.template).make();
        st.text = text.trim_end().to_string();
        st.size = o.size;
        st.auto_wrap = false;
        let (col, row) = ((o.pos % 3) as f32 - 1.0, (o.pos / 3).min(2) as f32 - 1.0);
        st.align = col as i8;
        let (tw, th) = measure(&st);
        // too large for the frame: smaller instead of cut off
        let k = ((W as f32 - 2.0 * MARGIN) / tw.max(1.0)).min((H as f32 - 2.0 * MARGIN) / th.max(1.0)).min(1.0);
        st.size *= k;
        let mut c = Clip::new(ClipContent::Text(st), Time::ZERO, len);
        c.transform.x = col * (W as f32 / 2.0 - MARGIN - tw * k / 2.0);
        c.transform.y = row * (H as f32 / 2.0 - MARGIN - th * k / 2.0);
        layers.push((TrackKind::Text, c));
    }
    p.media = [Some(frame), color].into_iter().flatten().cloned().collect();
    for (kind, c) in layers {
        let mut t = Track::new(kind);
        t.clips.push(c);
        p.timeline.tracks.push(t);
    }
    p
}

/// Width and height of a text as in the export (font via `fonts`, otherwise system font)
fn measure(fonts: &Fonts, st: &TextStyle) -> (f32, f32) {
    let (data, idx) = font_data(fonts, st.font_family.as_deref());
    measure_in(&data, idx, st)
}

/// Font file (content, index) of a family as in the export, otherwise the system font
pub fn font_data(fonts: &Fonts, family: Option<&str>) -> (Vec<u8>, u32) {
    let font = family.and_then(|f| fonts.get(f).cloned()).or_else(|| cl_export::default_font().map(|p| (p, 0)));
    let data = font.as_ref().and_then(|f| std::fs::read(&f.0).ok()).unwrap_or_default();
    (data, font.map_or(0, |f| f.1))
}

/// Like [`measure`] with an already read font
pub fn measure_in(data: &[u8], idx: u32, st: &TextStyle) -> (f32, f32) {
    let txt = if st.uppercase { st.text.to_uppercase() } else { st.text.clone() };
    let lines: Vec<&str> = txt.split('\n').collect();
    let tw = lines
        .iter()
        .map(|l| cl_fonts::text_width(data, idx, st.size, l).unwrap_or(st.size * 0.6 * l.chars().count() as f32))
        .fold(0.0f32, f32::max);
    let border = st.outline.map_or(0.0, |o| 2.0 * o.0);
    (tw + border, st.size * 1.3 * st.line_spacing.max(0.5) * lines.len() as f32 + border)
}

/// Range `t` … `t + 2 frames` of `p` without audio as video in `w`×`h`
pub fn export_frames(p: &Project, t: Time, (w, h): (u32, u32), fonts: &Fonts, out: &Path) -> std::result::Result<(), String> {
    let mut s = cl_export::ExportSettings::from_project(p);
    (s.width, s.height) = (w, h);
    s.include_audio = false;
    s.chapters = false;
    s.quality = cl_export::Quality::Higher;
    let frame = Time::from_secs_f64(1.0 / p.settings.fps.as_f64());
    s.range = Some((t, t + frame + frame));
    let resolver = |fam: Option<&str>| fam.and_then(|f| fonts.get(f).cloned()).or_else(|| cl_export::default_font().map(|p| (p, 0)));
    let job = cl_export::build(p, &s, &resolver, out).map_err(|e| e.to_string())?;
    cl_export::run(&job, Arc::new(AtomicBool::new(false)), |_| {}).map_err(|e| e.to_string())
}

/// First frame of a video as PNG
pub fn first_png(video: &Path, out: &Path) -> std::result::Result<(), String> {
    let ok = cl_media::ffmpeg_command()
        .map_err(|e| e.to_string())?
        .args(["-v", "error", "-y", "-i"])
        .arg(video)
        .args(["-frames:v", "1"])
        .arg(out)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(()).ok_or_else(|| "Thumbnail konnte nicht gespeichert werden".into())
}

/// Thumbnail for `w` from `src` to `out` (PNG). Still and mask per position of the current
/// project state stay in `dir`, so that changes to text or outline only render the
/// helper project; those of older states are removed.
pub fn render(
    src: &Project,
    w: &Want,
    fonts: &Fonts,
    models: Option<&cl_analyze::Models>,
    dir: &Path,
    out: &Path,
    cancel: &AtomicBool,
) -> std::result::Result<(), String> {
    // Preview and saving share the files in the work folder: one after another
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let frame = dir.join(format!("frame-{}-{}.mp4", w.rev, w.at.0));
    if !frame.is_file() {
        let keep = [format!("frame-{}-", w.rev), format!("matte-{}-", w.rev)];
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if (n.starts_with("frame-") || n.starts_with("matte-")) && !keep.iter().any(|k| n.starts_with(k)) {
                let _ = std::fs::remove_file(e.path());
            }
        }
        export_frames(src, w.at, contain(src.settings.width, src.settings.height), fonts, &frame)?;
    }
    let frame = cl_media::probe(&frame).map_err(|e| e.to_string())?;
    let matte = dir.join(format!("matte-{}-{}.mkv", w.rev, w.at.0));
    let matte = match models.filter(|_| w.opts.person) {
        Some(_) if matte.is_file() => Some(matte),
        // shortcut: RVM sees the still only twice (recurrent model without warm-up); with
        // unclean edges repeat the frame more often
        Some(m) => cl_analyze::matte::matte(m, &frame.path, &matte, &|_| {}, cancel).map(|_| Some(matte))?,
        None => None,
    };
    let color = match w.opts.bg {
        Bg::Color if matte.is_some() => {
            let [r, g, b] = w.opts.bg_color;
            let path = dir.join(format!("color-{r:02x}{g:02x}{b:02x}.png"));
            image::RgbImage::from_pixel(16, 16, image::Rgb([r, g, b])).save(&path).map_err(|e| e.to_string())?;
            Some(cl_media::probe(&path).map_err(|e| e.to_string())?)
        }
        _ => None,
    };
    let p = project(&w.opts, &w.text, &frame, matte.as_deref(), color.as_ref(), &|st| measure(fonts, st));
    let video = dir.join(format!("thumb-{}.mp4", std::process::id()));
    let r = export_frames(&p, Time::ZERO, (W, H), fonts, &video).and_then(|_| first_png(&video, out));
    let _ = std::fs::remove_file(&video);
    r
}

/// Open „Thumbnail“ window
pub struct ThumbWin {
    /// Position in seconds
    pub at: f32,
    pub text: String,
    /// Work folder (still, mask, preview); disappears with the last task
    dir: Arc<tempfile::TempDir>,
    tex: Option<egui::TextureHandle>,
    /// last requested
    asked: Option<Want>,
    /// last seen state and since when (debounce)
    seen: Option<(Want, f64)>,
}

impl App {
    fn thumb_want(&self) -> Option<Want> {
        let w = self.thumb.as_ref()?;
        Some(Want {
            rev: self.revision,
            at: Time::from_secs_f64(w.at as f64).snap_to_frame(self.project.settings.fps),
            opts: self.thumb_opts.clone(),
            text: w.text.clone(),
        })
    }

    fn thumb_models(&self) -> Option<cl_analyze::Models> {
        let m = self.models();
        (m.installed("onnxruntime") && m.installed("rvm")).then_some(m)
    }

    pub fn thumb_open(&mut self) {
        let Ok(dir) = tempfile::Builder::new().prefix("cutline-thumb").tempdir() else {
            self.toast_err("Thumbnail konnte nicht gespeichert werden");
            return;
        };
        self.thumb = Some(ThumbWin {
            at: self.playhead.as_secs_f64() as f32,
            text: String::new(),
            dir: Arc::new(dir),
            tex: None,
            asked: None,
            seen: None,
        });
    }

    fn thumb_close(&mut self) {
        self.thumb = None;
        if let Some(t) = self.tasks.running("thumb") {
            t.cancel();
        }
        self.save_last_used();
    }

    /// Render the thumbnail with the current settings to `out` (in the background)
    pub fn thumb_save(&mut self, ctx: &egui::Context, out: PathBuf) {
        let (Some(want), Some(w)) = (self.thumb_want(), self.thumb.as_ref()) else { return };
        let (src, fonts, models, dir) = (self.project.clone(), self.font_files(), self.thumb_models(), w.dir.clone());
        self.tasks.spawn(ctx, "thumb:save", "Thumbnail wird erstellt …", move |_, cancel| {
            match render(&src, &want, &fonts, models.as_ref(), dir.path(), &out, cancel) {
                Ok(()) => TaskDone::Run(Box::new(move |app: &mut App| {
                    app.toast(format!("Thumbnail gespeichert: {}", out.display()));
                    crate::home::reveal(&out);
                })),
                Err(e) => TaskDone::Failed(e),
            }
        });
    }

    /// Re-render the preview when the state has changed and has been quiet for 0.25 s; the last
    /// finished frame stays until then.
    fn thumb_preview(&mut self, ctx: &egui::Context) {
        let Some(want) = self.thumb_want() else { return };
        let now = ctx.input(|i| i.time);
        let Some(w) = self.thumb.as_mut() else { return };
        if w.seen.as_ref().is_none_or(|s| s.0 != want) {
            w.seen = Some((want.clone(), now));
        }
        let quiet = now - w.seen.as_ref().map_or(now, |s| s.1);
        if w.asked.as_ref() == Some(&want) || self.tasks.running("thumb").is_some() {
            return;
        }
        if quiet < 0.25 && w.asked.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(0.25 - quiet));
            return;
        }
        w.asked = Some(want.clone());
        let dir = w.dir.clone();
        let (src, fonts, models, ctx2) = (self.project.clone(), self.font_files(), self.thumb_models(), ctx.clone());
        self.tasks.spawn(ctx, "thumb", "Thumbnail-Vorschau …", move |_, cancel| {
            let t0 = std::time::Instant::now();
            let png = dir.path().join("preview.png");
            let img = render(&src, &want, &fonts, models.as_ref(), dir.path(), &png, cancel)
                .and_then(|_| image::open(&png).map_err(|e| e.to_string()));
            let ms = t0.elapsed().as_millis();
            match img {
                Ok(img) => {
                    let img = img.to_rgba8();
                    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], &img);
                    TaskDone::Run(Box::new(move |app: &mut App| {
                        if let Some(w) = app.thumb.as_mut() {
                            log::info!(target: "thumbnail", "Vorschau in {ms} ms");
                            w.tex = Some(ctx2.load_texture("thumb", ci, egui::TextureOptions::LINEAR));
                        }
                    }))
                }
                Err(e) => TaskDone::Failed(e),
            }
        });
    }

    pub fn thumb_window(&mut self, ctx: &egui::Context) {
        if self.thumb.is_none() {
            return;
        }
        self.thumb_preview(ctx);
        let (esc, enter) = crate::dialog::keys(ctx);
        let mut open = !esc;
        let mut save = false;
        let models = self.thumb_models().is_some();
        let dur = (self.project.duration().as_secs_f64() - 1.0 / self.project.settings.fps.as_f64()).max(0.0) as f32;
        let busy = self.tasks.running("thumb").is_some();
        let saving = self.tasks.running("thumb:save").is_some();
        let Some(mut w) = self.thumb.take() else { return };
        let o = &mut self.thumb_opts;
        crate::dialog::window(ctx, crate::tr("Thumbnail"), "thumb")
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                if crate::dialog::title(ui, "Thumbnail") {
                    open = false;
                }
                ui.horizontal_top(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(640.0, 360.0), Sense::hover());
                    match &w.tex {
                        Some(t) => {
                            ui.painter().image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                        }
                        None => {
                            ui.painter().rect_filled(r, theme::R_CARD, theme::BG_INPUT());
                        }
                    }
                    if busy {
                        widgets::busy_line(ui.painter(), egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 2.0), r.max));
                    }
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.set_width(300.0);
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            ui.label(egui::RichText::new(crate::tr("Bild")).strong());
                            theme::row(ui, "Stelle", |ui| Slider::new(&mut w.at, 0.0..=dur).suffix(" s").decimals(1).show(ui));
                            theme::section(ui, "Person freistellen");
                            ui.add_enabled_ui(models, |ui| {
                                let mut on = o.person && models;
                                widgets::toggle_row(ui, "Person freistellen", &mut on);
                                o.person = on;
                            })
                            .response
                            .on_disabled_hover_text(crate::tr(format!(
                                "Benötigt: {} (Einstellungen → Modelle)",
                                cl_analyze::CATALOG.iter().find(|c| c.id == "rvm").map_or("", |c| c.name)
                            )));
                            if o.person && models {
                                theme::row(ui, "Hintergrund", |ui| {
                                    let name = BG_NAMES.iter().find(|b| b.0 == o.bg).map_or("", |b| b.1);
                                    egui::ComboBox::from_id_salt("thumb-bg")
                                        .selected_text(crate::tr(name))
                                        .width((ui.available_width() - 12.0).max(60.0))
                                        .show_ui(ui, |ui| {
                                            for (b, n) in BG_NAMES {
                                                ui.selectable_value(&mut o.bg, b, crate::tr(n));
                                            }
                                        });
                                });
                                if o.bg == Bg::Color {
                                    color_row(ui, "Farbe", &mut o.bg_color);
                                }
                                widgets::toggle_row(ui, "Kontur", &mut o.outline);
                                if o.outline {
                                    color_row(ui, "Farbe", &mut o.outline_color);
                                    theme::row(ui, "Stärke", |ui| Slider::new(&mut o.outline_width, 0.0..=1.0).percent().default(0.4).show(ui));
                                }
                            }
                            theme::section(ui, "Text");
                            ui.add(egui::TextEdit::multiline(&mut w.text).desired_rows(2).desired_width(f32::INFINITY));
                            theme::row(ui, "Vorlage", |ui| {
                                egui::ComboBox::from_id_salt("thumb-tpl")
                                    .selected_text(crate::tr(template(&o.template).name))
                                    .width((ui.available_width() - 12.0).max(60.0))
                                    .show_ui(ui, |ui| {
                                        for t in templates() {
                                            if ui.selectable_label(o.template == t.id, crate::tr(t.name)).clicked() {
                                                o.template = t.id.into();
                                            }
                                        }
                                    });
                            });
                            theme::row(ui, "Größe", |ui| Slider::new(&mut o.size, 40.0..=240.0).default(120.0).decimals(0).show(ui));
                            theme::row(ui, "Lage", |ui| grid(ui, &mut o.pos));
                        });
                    });
                });
                let (main, cancel) = crate::dialog::buttons(ui, theme::primary("PNG speichern"), !saving);
                save = main || (enter && !saving);
                if cancel {
                    open = false;
                }
            });
        self.thumb = Some(w);
        if save {
            let base = self
                .project_path
                .as_ref()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(crate::paths::data_dir);
            if let Some(out) = crate::dialog()
                .set_directory(&base)
                .set_file_name(format!("{} – Thumbnail.png", self.project.name))
                .add_filter("PNG", &["png"])
                .save_file()
            {
                self.thumb_save(ctx, out);
                open = false;
            }
        }
        if !open {
            self.thumb_close();
        }
    }
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [u8; 3]) {
    theme::row(ui, label, |ui| {
        let mut rgba = [c[0], c[1], c[2], 255];
        if widgets::color_button(ui, &mut rgba) {
            *c = [rgba[0], rgba[1], rgba[2]];
        }
    });
}

/// 3×3 grid for the text position
fn grid(ui: &mut egui::Ui, pos: &mut u8) {
    let (cell, gap) = (vec2(22.0, 13.0), 3.0);
    let (r, resp) = ui.allocate_exact_size(vec2(3.0 * cell.x + 2.0 * gap, 3.0 * cell.y + 2.0 * gap), Sense::click());
    for i in 0..9u8 {
        let min = r.min + vec2((i % 3) as f32 * (cell.x + gap), (i / 3) as f32 * (cell.y + gap));
        let c = egui::Rect::from_min_size(min, cell);
        if resp.clicked() && resp.interact_pointer_pos().is_some_and(|p| c.expand(gap / 2.0).contains(p)) {
            *pos = i;
        }
        let fill = if *pos == i { theme::ACCENT() } else { theme::BG_RAISED() };
        ui.painter().rect_filled(c, theme::R_CTRL, fill);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(w: u32, h: u32, kind: MediaKind) -> MediaRef {
        MediaRef {
            id: MediaId::new(),
            path: PathBuf::from("bild.mp4"),
            kind,
            duration: Time::from_secs_f64(0.07),
            width: w,
            height: h,
            fps: Some(Rational::new(30, 1)),
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        }
    }

    /// rough text size like the export without a font file
    fn rough(st: &TextStyle) -> (f32, f32) {
        let lines: Vec<&str> = st.text.split('\n').collect();
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32 * st.size * 0.6;
        (w, st.size * 1.3 * lines.len() as f32)
    }

    fn kinds(p: &Project) -> Vec<&'static str> {
        p.timeline
            .tracks
            .iter()
            .map(|t| {
                let c = &t.clips[0];
                match &c.content {
                    ClipContent::Text(_) => "text",
                    ClipContent::Media { kind: MediaKind::Image, .. } => "farbe",
                    _ if c.fx.cutout.is_some() => "person",
                    _ if c.transform.scale > 1.01 => "fuellung",
                    _ => "bild",
                }
            })
            .collect()
    }

    #[test]
    fn helper_project_layers() {
        let wide = media(1280, 720, MediaKind::Video);
        let tall = media(406, 720, MediaKind::Video);
        let color = media(16, 16, MediaKind::Image);
        let matte = Path::new("maske.mkv");
        let o = Opts::default();
        let p = project(&o, "", &wide, None, None, &rough);
        assert_eq!((p.settings.width, p.settings.height), (W, H));
        assert_eq!(kinds(&p), ["bild"], "text empty, person off → only the image");
        assert_eq!(kinds(&project(&o, "  \n", &wide, Some(matte), None, &rough)), ["bild"], "person off despite the mask");
        // other aspect ratio: blurred fill under the fitted image
        let p = project(&o, "Hallo", &tall, None, None, &rough);
        assert_eq!(kinds(&p), ["fuellung", "bild", "text"]);
        assert!(p.timeline.tracks[0].clips[0].fx.effects.iter().any(|e| e.id == "blur"));
        assert!(p.media.iter().any(|m| m.id == tall.id));
        // Person with outline above the blurred image
        let on = Opts { person: true, ..o.clone() };
        let p = project(&on, "Hallo", &wide, Some(matte), Some(&color), &rough);
        assert_eq!(kinds(&p), ["bild", "person", "text"]);
        assert!(p.timeline.tracks[0].clips[0].fx.effects.iter().any(|e| e.id == "blur"));
        assert!(matches!(p.timeline.tracks[1].clips[0].fx.person, Some(PersonFx::Outline { transparent: true, .. })));
        // without outline: person without the person effect (transparent)
        let p = project(&Opts { outline: false, ..on.clone() }, "", &wide, Some(matte), None, &rough);
        assert!(p.timeline.tracks[1].clips[0].fx.person.is_none());
        // model missing (no mask): no cutout layer
        assert_eq!(kinds(&project(&on, "", &wide, None, None, &rough)), ["bild"]);
        // „Farbe“ background: color area instead of the image, also with another aspect ratio
        let c = Opts { bg: Bg::Color, ..on.clone() };
        assert_eq!(kinds(&project(&c, "", &tall, Some(matte), Some(&color), &rough)), ["farbe", "person"]);
        // darkened: image darker, the person above unchanged
        let p = project(&Opts { bg: Bg::Dark, ..on }, "", &wide, Some(matte), None, &rough);
        assert!(p.timeline.tracks[0].clips[0].fx.adjust.brightness < 0.0);
        assert!(p.timeline.tracks[1].clips[0].fx.adjust.is_identity());
    }

    #[test]
    fn text_stays_inside_the_frame_for_every_grid_cell() {
        let wide = media(1280, 720, MediaKind::Video);
        for text in ["KRASS?!", "Zwei\nZeilen", "Ein sehr langer Titel, der nicht in eine Zeile passt"] {
            for pos in 0..9u8 {
                let o = Opts { pos, size: 200.0, ..Default::default() };
                let p = project(&o, text, &wide, None, None, &rough);
                let c = &p.timeline.tracks.last().unwrap().clips[0];
                let ClipContent::Text(st) = &c.content else { panic!("text layer missing") };
                let (tw, th) = rough(st);
                let (x, y) = (c.transform.x, c.transform.y);
                assert!(x.abs() + tw / 2.0 <= W as f32 / 2.0 - MARGIN + 0.5, "{text} {pos}: x {x}, Breite {tw}");
                assert!(y.abs() + th / 2.0 <= H as f32 / 2.0 - MARGIN + 0.5, "{text} {pos}: y {y}, height {th}");
                let side = |v: f32| if v.abs() < 0.5 { 0 } else { v.signum() as i32 };
                let (col, row) = (pos as i32 % 3 - 1, pos as i32 / 3 - 1);
                // if the text fills the width or height fully, it is centered there
                let room = |free: f32, d: i32| if free < 0.5 { 0 } else { d };
                assert_eq!((side(x), side(y)), (room(W as f32 / 2.0 - MARGIN - tw / 2.0, col), room(H as f32 / 2.0 - MARGIN - th / 2.0, row)), "{text} {pos}");
                assert_eq!(st.align as i32, col);
            }
        }
    }

    fn testsrc(dir: &Path, name: &str, w: u32, h: u32) -> MediaRef {
        let path = dir.join(name);
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-threads", "2", "-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size={w}x{h}:rate=30:duration=2"))
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(st.success());
        cl_media::probe(&path).unwrap()
    }

    fn source(dir: &Path, w: u32, h: u32) -> Project {
        let m = testsrc(dir, &format!("q{w}x{h}.mp4"), w, h);
        let mut p = Project::new("Quelle");
        (p.settings.width, p.settings.height) = (w, h);
        let mut t = Track::new(TrackKind::Video);
        t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(2.0)));
        p.media.push(m);
        p.timeline.tracks.push(t);
        p
    }

    fn rgba(png: &Path) -> image::RgbaImage {
        image::open(png).unwrap().to_rgba8()
    }

    /// Without a model: image and text, 1280×720; the text is visible, 9:16 fills without black borders.
    #[test]
    fn renders_image_and_text_and_fills_other_aspect_ratios() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (fonts, stop) = (Fonts::new(), AtomicBool::new(false));
        let src = source(dir.path(), 1920, 1080);
        let mut w = Want { rev: 0, at: Time::from_secs_f64(1.0), opts: Opts { pos: 4, ..Default::default() }, text: String::new() };
        let (plain, with) = (dir.path().join("ohne.png"), dir.path().join("mit.png"));
        let t0 = std::time::Instant::now();
        render(&src, &w, &fonts, None, dir.path(), &plain, &stop).unwrap();
        eprintln!("Thumbnail 16:9 mit Standbild: {} ms", t0.elapsed().as_millis());
        w.text = "TEST".into();
        let t0 = std::time::Instant::now();
        render(&src, &w, &fonts, None, dir.path(), &with, &stop).unwrap();
        eprintln!("Thumbnail 16:9, Standbild schon da: {} ms", t0.elapsed().as_millis());
        let (a, b) = (rgba(&plain), rgba(&with));
        assert_eq!(b.dimensions(), (W, H));
        // Center (text position 4): clearly different from without text; corner unchanged
        let diff = |x0: u32, y0: u32| {
            let mut d = 0u64;
            for y in y0..y0 + 60 {
                for x in x0..x0 + 160 {
                    let (p, q) = (a.get_pixel(x, y), b.get_pixel(x, y));
                    d += (0..3).map(|c| (p[c] as i32 - q[c] as i32).unsigned_abs() as u64).sum::<u64>();
                }
            }
            d as f64 / (60.0 * 160.0 * 3.0)
        };
        assert!(diff(560, 330) > 20.0, "text in the center not visible: {:.1}", diff(560, 330));
        assert!(diff(20, 20) < 4.0, "corner changed: {:.1}", diff(20, 20));
        // 9:16: blurred fill up to the borders
        let tall = source(dir.path(), 1080, 1920);
        let out = dir.path().join("hoch.png");
        let t0 = std::time::Instant::now();
        render(&tall, &w, &fonts, None, dir.path(), &out, &stop).unwrap();
        eprintln!("Thumbnail 9:16 mit Standbild: {} ms", t0.elapsed().as_millis());
        let img = rgba(&out);
        assert_eq!(img.dimensions(), (W, H));
        for y in (40..H - 40).step_by(40) {
            for x in [4, W - 5] {
                let p = img.get_pixel(x, y);
                assert!(p[0] as u32 + p[1] as u32 + p[2] as u32 > 30, "black border at {x},{y}: {p:?}");
            }
        }
    }

    /// Project changed while the window is open (new state): the still is re-rendered, the
    /// files of the old state disappear from the work folder.
    #[test]
    fn project_change_renders_a_new_frame() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("arbeit");
        std::fs::create_dir(&work).unwrap();
        let (fonts, stop) = (Fonts::new(), AtomicBool::new(false));
        let mut src = source(dir.path(), 640, 360);
        let mut w = Want { rev: 1, at: Time::from_secs_f64(1.0), opts: Opts::default(), text: String::new() };
        let (a, b) = (dir.path().join("a.png"), dir.path().join("b.png"));
        render(&src, &w, &fonts, None, &work, &a, &stop).unwrap();
        src.timeline.tracks[0].clips[0].fx.adjust.brightness = -1.0;
        w.rev = 2;
        render(&src, &w, &fonts, None, &work, &b, &stop).unwrap();
        let luma = |p: &Path| rgba(p).pixels().map(|q| q[0] as u64 + q[1] as u64 + q[2] as u64).sum::<u64>() / (W * H) as u64;
        assert!(luma(&b) + 30 < luma(&a), "alter Stand gespeichert: {} / {}", luma(&a), luma(&b));
        let names: Vec<String> = std::fs::read_dir(&work).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.starts_with("frame-1-")) && names.iter().any(|n| n.starts_with("frame-2-")), "{names:?}");
    }

    /// Open, change, save: no history step, no revision, nothing unsaved.
    #[test]
    fn open_project_stays_untouched() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let (mut app, home, _g) = crate::saving::tests::test_app();
        app.set_doc(source(home.path(), 640, 360));
        app.project = app.doc.lock().unwrap().project().clone();
        let before = (app.revision, app.saved_revision, app.doc.lock().unwrap().can_undo(), app.project.clone());
        let ctx = egui::Context::default();
        app.run_action(&ctx, "quick_thumbnail");
        assert!(app.thumb.is_some(), "Fenster offen");
        app.thumb_opts.pos = 0;
        app.thumb_opts.outline = false;
        app.thumb.as_mut().unwrap().text = "Hallo".into();
        for _ in 0..3 {
            let _ = ctx.run(Default::default(), |ctx| app.thumb_window(ctx));
        }
        let out = home.path().join("t.png");
        app.thumb_save(&ctx, out.clone());
        // collect results without applying them (Explorer would open)
        let t0 = std::time::Instant::now();
        let mut saved = false;
        while !saved {
            assert!(t0.elapsed().as_secs() < 120, "thumbnail not saved");
            for (k, d) in app.tasks.poll() {
                match d {
                    TaskDone::Failed(e) => panic!("{k}: {e}"),
                    _ => saved |= k == "thumb:save",
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(image::open(&out).unwrap().to_rgba8().dimensions(), (W, H));
        let after = (app.revision, app.saved_revision, app.doc.lock().unwrap().can_undo(), app.project.clone());
        assert_eq!(after, before);
    }
}
