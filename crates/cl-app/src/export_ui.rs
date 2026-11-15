//! Export-Dialog: ohne Pro-Sperre, ohne „Urheberrecht überprüfen“, mit echtem Fortschritt.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cl_core::{Project, Time};
use cl_export::{Codec, Encoder, ExportSettings, Format, Quality};
use eframe::egui::{self, vec2, Color32, Rect, RichText};

use crate::theme::{self, icon};

/// Zielplattform: setzt Projektformat (Größe, Bildrate) und Export (Codec, Qualität).
pub struct Platform {
    pub id: &'static str,
    pub name: &'static str,
    pub w: u32,
    pub h: u32,
    pub fps: Option<u32>,
    pub codec: Codec,
    pub quality: Quality,
}

pub const PLATFORMS: &[Platform] = &[
    Platform {
        id: "youtube-1080",
        name: "YouTube 1080p",
        w: 1920,
        h: 1080,
        fps: None,
        codec: Codec::H264,
        quality: Quality::Higher,
    },
    Platform {
        id: "youtube-1440",
        name: "YouTube 1440p",
        w: 2560,
        h: 1440,
        fps: None,
        codec: Codec::H264,
        quality: Quality::Higher,
    },
    Platform {
        id: "youtube-4k",
        name: "YouTube 4K",
        w: 3840,
        h: 2160,
        fps: None,
        codec: Codec::Hevc,
        quality: Quality::Higher,
    },
    Platform {
        id: "shorts",
        name: "Shorts / TikTok / Reels (1080×1920)",
        w: 1080,
        h: 1920,
        fps: Some(30),
        codec: Codec::H264,
        quality: Quality::Higher,
    },
    Platform {
        id: "instagram",
        name: "Instagram-Beitrag (1080×1350)",
        w: 1080,
        h: 1350,
        fps: Some(30),
        codec: Codec::H264,
        quality: Quality::Higher,
    },
    Platform {
        id: "twitch",
        name: "Twitch-Clip (1080p60)",
        w: 1920,
        h: 1080,
        fps: Some(60),
        codec: Codec::H264,
        quality: Quality::Recommended,
    },
    Platform {
        id: "small",
        name: "Kleine Datei zum Teilen (720p)",
        w: 1280,
        h: 720,
        fps: Some(30),
        codec: Codec::H264,
        quality: Quality::Lower,
    },
];

pub fn platform(id: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|p| p.id == id)
}

impl Platform {
    /// Projektformat für diese Plattform (Größe, Seitenverhältnis, ggf. Bildrate).
    pub fn apply_to(&self, s: &mut cl_core::ProjectSettings) {
        s.width = self.w;
        s.height = self.h;
        let g = gcd(self.w, self.h);
        s.aspect = cl_core::AspectPreset::Ratio(self.w / g, self.h / g);
        if let Some(f) = self.fps {
            s.fps = cl_core::Rational::new(f, 1);
        }
        s.platform = Some(self.id.to_string());
    }
    /// Passt das Seitenverhältnis des Projekts zu dieser Plattform?
    pub fn fits(&self, s: &cl_core::ProjectSettings) -> bool {
        let (a, b) = (
            s.width.max(1) as f32 / s.height.max(1) as f32,
            self.w as f32 / self.h as f32,
        );
        (a - b).abs() < 0.02
    }
}

/// YouTube-Richtwerte für SDR-Uploads (kbit/s) nach kürzerer Bildkante: bis 30 fps / darüber.
pub fn youtube_kbps(short_edge: u32, fps: f64) -> Option<u32> {
    let (lo, hi) = match short_edge {
        1080 => (8_000, 12_000),
        1440 => (16_000, 24_000),
        2160 => (40_000, 60_000),
        _ => return None,
    };
    Some(if fps > 30.5 { hi } else { lo })
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a.max(1)
    } else {
        gcd(b, a % b)
    }
}

struct Running {
    progress: Arc<Mutex<f32>>,
    /// Lautheit wird vorher gemessen (erster Teil des Fortschritts)
    measures: bool,
    result: Arc<Mutex<Option<Result<(), String>>>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    /// Projektstand beim Start des Exports
    rev: u64,
    /// Begleitdateien (Untertitel, Kapitel) mit Text vom Start; erst nach Erfolg geschrieben
    sidecars: Vec<(PathBuf, String)>,
    /// Einstellungen beim Start (Fingerabdruck für „Erneut exportieren“)
    settings: ExportSettings,
    /// Vorschaubild, das FFmpeg während des Exports laufend überschreibt
    preview: Option<PathBuf>,
    /// geladene Live-Bilder; Texturen werden mit `Running` freigegeben
    live: Live,
}

/// Änderungszeit und Größe der Vorschaudatei
type Sig = (std::time::SystemTime, u64);

/// Live-Vorschau des laufenden Exports: aktuelles Bild (seit wann) und vorheriges zum Überblenden.
#[derive(Default)]
struct Live {
    sig: Option<Sig>,
    checked: Option<Instant>,
    cur: Option<(egui::TextureHandle, Instant)>,
    prev: Option<egui::TextureHandle>,
}

impl Live {
    /// Höchstens alle 400 ms nachsehen; nur eine geänderte, vollständig lesbare Datei ersetzt das Bild.
    fn poll(&mut self, ctx: &egui::Context, path: &Path) {
        if self.checked.is_some_and(|t| t.elapsed().as_millis() < 400) {
            return;
        }
        self.checked = Some(Instant::now());
        let sig = std::fs::metadata(path).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
        if !changed(self.sig, sig) {
            return;
        }
        // halb geschrieben: still überspringen, das letzte gute Bild bleibt
        let Some(img) = std::fs::read(path).ok().and_then(|b| decode_jpeg(&b)) else {
            return;
        };
        self.sig = sig;
        let tex = ctx.load_texture("export-live", img, egui::TextureOptions::LINEAR);
        self.prev = self.cur.take().map(|c| c.0);
        self.cur = Some((tex, Instant::now()));
    }
}

/// Neu laden? Nur wenn die Datei da ist und sich seit dem letzten Laden geändert hat.
fn changed(old: Option<Sig>, new: Option<Sig>) -> bool {
    new.is_some() && new != old
}

/// Anteil des neuen Bilds beim Überblenden (`dur` 0 = Animationen aus: sofort ganz).
fn blend(age: f32, dur: f32) -> f32 {
    if dur <= 0.0 {
        1.0
    } else {
        (age / dur).clamp(0.0, 1.0)
    }
}

/// Nur vollständige JPEGs (Endmarke FF D9) dekodieren; FFmpeg schreibt die Datei laufend neu.
fn decode_jpeg(b: &[u8]) -> Option<egui::ColorImage> {
    if !b.ends_with(&[0xFF, 0xD9]) {
        return None;
    }
    let img = image::load_from_memory_with_format(b, image::ImageFormat::Jpeg).ok()?.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

/// Zeichnet das Live-Bild eingepasst in `rect` (weich überblendet, feine Fortschrittslinie am
/// unteren Bildrand). `false`: noch kein Bild da.
fn paint_live(ui: &egui::Ui, rect: Rect, live: &Live, prog: f32) -> bool {
    let Some((cur, at)) = &live.cur else {
        return false;
    };
    let ts = cur.size_vec2();
    let fit = (rect.width() / ts.x).min(rect.height() / ts.y);
    let img = Rect::from_center_size(rect.center(), ts * fit);
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let p = ui.painter();
    let k = theme::ease(blend(at.elapsed().as_secs_f32(), theme::anim(theme::MOTION_SLOW)));
    if let Some(prev) = live.prev.as_ref().filter(|_| k < 1.0) {
        p.image(prev.id(), img, uv, Color32::WHITE);
    }
    p.image(cur.id(), img, uv, Color32::WHITE.gamma_multiply(k));
    if k < 1.0 {
        ui.ctx().request_repaint();
    }
    // gleicher weich nachlaufender Wert wie die Fortschrittslinie darunter
    let line = Rect::from_min_size(egui::pos2(img.left(), img.bottom() - 2.0), vec2(img.width(), 2.0));
    crate::widgets::progress_line(p, egui::Id::new("export-live"), line, prog);
    true
}

#[derive(Default)]
pub struct ExportDialog {
    open: bool,
    name: String,
    folder: PathBuf,
    /// Projektdatei, für die der Dialog geöffnet wurde (für die Exporte-Liste)
    project_path: Option<PathBuf>,
    settings: Option<ExportSettings>,
    /// Kürzere Bildkante (720/1080/1440/2160) oder 0 = Projekt
    short_edge: u32,
    custom_kbps: u32,
    /// In/Out-Bereich der Zeitleiste, falls gesetzt
    range_avail: Option<(Time, Time)>,
    preset_hint: Option<String>,
    /// Gewählte Plattform (wird im Projekt gemerkt, wenn das Format passt)
    platform: Option<&'static str>,
    picked_platform: Option<&'static str>,
    also_srt: bool,
    /// Marker als Kapitel schreiben (Einstellungen → Projekte → Export); setzt die App je Bild
    pub chapters: bool,
    /// Hinweis auf Kapitel unter 10 s zeigen; „Nicht mehr anzeigen“ meldet `chapter_hint_off`
    pub chapter_hint: bool,
    pub chapter_hint_off: bool,
    encoders: Arc<Mutex<Option<Vec<Encoder>>>>,
    running: Option<Running>,
    last_output: Option<PathBuf>,
    error: Option<String>,
    /// Effekte/Filter, die der Katalog nicht kennt (fallen im Export weg)
    missing: Vec<String>,
    /// Zeitpunkt und Dauer des letzten erfolgreichen Exports (kurze Erfolgs-Animation)
    done: Option<(Instant, f32)>,
    /// Projektstand beim Öffnen des Dialogs
    rev: u64,
    /// Projektstand, Datei und Einstellungen des letzten fertigen Exports („Zur Datei“ statt „Exportieren“)
    exported: Option<(u64, PathBuf, ExportSettings)>,
    /// Zieldatei existiert: Nachfrage vor dem Start
    confirm_overwrite: bool,
    /// Nachfrage erst in diesem Bild geöffnet: Enter/Esc noch nicht auswerten
    /// (das Enter, das „Exportieren“ ausgelöst hat, ist im selben Bild noch gedrückt)
    confirm_fresh: bool,
    /// „Projekt auf 9:16 umstellen“ geklickt (App stellt das Format um)
    switch_aspect: bool,
    /// Fenster soll schließen, während ein Export läuft: Rückfrage offen
    quit_ask: bool,
    /// Beenden trotz laufendem Export bestätigt
    quit_ok: bool,
    /// größte bisher gezeigte Höhe der Einstellungen (Dialog springt beim Formatwechsel nicht)
    right_h: f32,
}

/// Darf das Fenster schließen? Während eines Exports nur nach Bestätigung.
fn close_allowed(running: bool, confirmed: bool) -> bool {
    !running || confirmed
}

impl ExportDialog {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Fenster-Kreuz während eines Exports: Schließen aufhalten und nachfragen; „Beenden“ bricht
    /// den Export ab und schließt (FFmpeg räumt in [`Self::cancel_and_wait`] auf).
    pub fn guard_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !close_allowed(self.running.is_some(), self.quit_ok) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit_ask = true;
        }
        if !self.quit_ask {
            return;
        }
        let (mut quit, mut stay) = (false, false);
        let m = crate::dialog::modal(ctx, "export-quit").show(ctx, |ui| {
            ui.set_width(340.0);
            stay |= crate::dialog::title(ui, "Export läuft – wirklich beenden?");
            let (q, c) = crate::dialog::buttons(ui, theme::danger("Cutline beenden"), true);
            quit = q;
            stay |= c;
        });
        if quit {
            self.quit_ok = true;
            self.quit_ask = false;
            if let Some(r) = &self.running {
                r.cancel.store(true, Ordering::Relaxed);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if stay || m.should_close() {
            self.quit_ask = false;
        }
    }

    /// Laufenden Export abbrechen und höchstens `max` warten, bis FFmpeg beendet und die
    /// `.part`-Datei gelöscht ist (beim Beenden des Programms).
    pub fn cancel_and_wait(&mut self, max: std::time::Duration) {
        let Some(r) = &self.running else { return };
        r.cancel.store(true, Ordering::Relaxed);
        let t0 = Instant::now();
        while r.result.lock().unwrap().is_none() && t0.elapsed() < max {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }

    /// Dialog öffnen; `range` = In/Out-Bereich der Zeitleiste (angeboten und vorausgewählt), gleich über
    /// welchen Weg (Knopf, Menü, Tastenkürzel) exportiert wird.
    pub fn open_for(&mut self, p: &Project, project_path: Option<&Path>, rev: u64, range: Option<(Time, Time)>) {
        self.rev = rev;
        if self.running.is_some() {
            self.open = true;
            return;
        }
        self.open = true;
        self.range_avail = range.filter(|(a, b)| b > a);
        self.error = None;
        self.missing = cl_fx::catalog::missing(p).into_iter().map(|m| cl_fx::fx_name(&m.0)).collect();
        self.missing.sort();
        self.missing.dedup();
        self.last_output = None;
        self.name = p.name.clone();
        self.project_path = project_path.map(Path::to_path_buf);
        if self.folder.as_os_str().is_empty() {
            self.folder = project_path
                .and_then(|pp| pp.parent().map(Path::to_path_buf))
                .or_else(|| {
                    std::env::var_os("USERPROFILE")
                        .or_else(|| std::env::var_os("HOME"))
                        .map(|h| PathBuf::from(h).join("Videos"))
                })
                .unwrap_or_else(|| PathBuf::from("."));
        }
        let mut s = ExportSettings::from_project(p);
        s.range = self.range_avail;
        if let Some(old) = &self.settings {
            s.format = old.format;
            s.codec = old.codec;
            s.quality = old.quality;
            s.encoder = old.encoder;
            s.include_audio = old.include_audio;
        }
        self.short_edge = 0;
        self.preset_hint = None;
        self.platform = None;
        if let Some(pl) = p.settings.platform.as_deref().and_then(platform) {
            Self::apply_platform(&mut s, &mut self.short_edge, &mut self.custom_kbps, pl);
            self.platform = Some(pl.id);
        }
        self.settings = Some(s);
        if self.custom_kbps == 0 {
            self.custom_kbps = 12000;
        }
        // Hardware-Encoder einmalig im Hintergrund prüfen
        if self.encoders.lock().unwrap().is_none() {
            let slot = self.encoders.clone();
            *slot.lock().unwrap() = Some(vec![Encoder::Software]);
            std::thread::spawn(move || {
                let found = cl_export::available_encoders();
                *slot.lock().unwrap() = Some(found);
            });
        }
    }

    fn apply_platform(s: &mut ExportSettings, short_edge: &mut u32, custom_kbps: &mut u32, pl: &Platform) {
        s.format = Format::Mp4;
        s.codec = pl.codec;
        s.quality = pl.quality;
        *short_edge = pl.w.min(pl.h);
        if let Some(f) = pl.fps {
            s.fps = cl_core::Rational::new(f, 1);
        }
        // YouTube: offizielle Bitraten, AAC 384 kbit/s, Lautheit −14 LUFS
        let yt = pl.id.starts_with("youtube");
        s.loudness = yt.then_some(-14.0);
        s.audio_kbps = yt.then_some(384);
        if let Some(k) = youtube_kbps(*short_edge, s.fps.as_f64()).filter(|_| yt) {
            s.quality = Quality::Bitrate(k);
            *custom_kbps = k;
        }
    }

    /// „Projekt auf 9:16 umstellen“ wurde geklickt (einmalig abholen).
    pub fn take_switch_aspect(&mut self) -> bool {
        std::mem::take(&mut self.switch_aspect)
    }

    /// Im Export gewählte Plattform (einmalig abholen), damit das Projekt sie sich merkt.
    pub fn take_picked_platform(&mut self) -> Option<&'static str> {
        self.picked_platform.take()
    }

    /// Projekt, Zieldatei und Export-Einstellungen wie beim letzten fertigen Export: „Zur Datei“ und
    /// „Erneut exportieren“ (überschreibt ohne Nachfrage) statt „Exportieren“
    fn unchanged_since_export(&self, s: &ExportSettings) -> bool {
        self.exported
            .as_ref()
            .is_some_and(|(r, f, used)| *r == self.rev && f.is_file() && *f == self.output_path(s.format) && used == s)
    }

    fn output_path(&self, f: Format) -> PathBuf {
        let safe: String = self
            .name
            .chars()
            .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
            .collect();
        self.folder
            .join(format!("{}.{}", safe.trim(), f.extension()))
    }

    /// Zeigt den Dialog. Gibt eine Meldung für einen Toast zurück, wenn ein Export fertig ist.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Project,
        fonts: &HashMap<String, (PathBuf, u32)>,
        cover: Option<(egui::TextureId, egui::Vec2)>,
        rev: u64,
    ) -> Option<String> {
        // aktueller Projektstand: nach einer Änderung gilt der letzte Export als veraltet
        self.rev = rev;
        let mut toast = None;
        // Hintergrund-Export weiterverfolgen, auch wenn der Dialog geschlossen ist
        let started_rev = self.running.as_ref().map_or(rev, |r| r.rev);
        let finished = self.running.as_ref().map(|r| {
            (
                r.result.lock().unwrap().take(),
                r.started.elapsed().as_secs_f32(),
            )
        });
        match finished {
            Some((Some(res), secs)) => {
                let (sidecars, used) = self.running.take().map(|r| (r.sidecars, Some(r.settings))).unwrap_or_default();
                match res {
                    Ok(()) => {
                        self.done = Some((Instant::now(), secs));
                        let failed: Vec<String> = sidecars
                            .iter()
                            .filter_map(|(f, txt)| std::fs::write(f, txt).err().map(|e| format!("{} ({e})", f.display())))
                            .collect();
                        if let Some(out) = &self.last_output {
                            // unter dem Projektnamen, nicht dem (änderbaren) Dateinamen
                            crate::exports::record(&p.name, self.project_path.as_deref(), out);
                            if let Some(used) = used {
                                self.exported = Some((started_rev, out.clone(), used));
                            }
                        }
                        toast = Some(format!(
                            "{} Export fertig ({secs:.1} s)",
                            icon::CHECK_CIRCLE
                        ));
                        if !failed.is_empty() {
                            let msg = format!("{}: {}", crate::tr("Begleitdatei nicht gespeichert"), failed.join(", "));
                            toast = Some(format!("{} {msg}", icon::WARNING_CIRCLE));
                            self.error = Some(msg);
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Some((None, _)) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            None => {}
        }
        if !self.open {
            return toast;
        }
        // Esc schließt den Dialog, Enter löst die Hauptaktion aus (nicht während eines laufenden
        // Exports oder der Überschreiben-Nachfrage)
        let (esc, enter) = crate::dialog::keys(ctx);
        let idle = self.running.is_none() && !self.confirm_overwrite;
        if idle && esc {
            self.open = false;
            return toast;
        }
        let mut main = idle && enter;
        let Some(mut s) = self.settings.clone() else {
            return toast;
        };
        s.chapters = self.chapters;
        s.chapter_name = crate::tr("Kapitel");
        let mut open = true;
        let mut start = false;
        // ohne Nachfrage überschreiben (bestätigt oder „Erneut exportieren“)
        let mut overwrite = false;
        let title = format!("{}  Exportieren", icon::EXPORT);
        crate::dialog::window(ctx, crate::tr(title.as_str()), "export-dialog")
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(720.0, 0.0))
            .show(ctx, |ui| {
                if crate::dialog::title(ui, &title) {
                    open = false;
                }
                let busy = self.running.is_some();
                ui.horizontal_top(|ui| {
                    // Links: Cover/Vorschau
                    ui.vertical(|ui| {
                        ui.set_width(250.0);
                        let size = vec2(250.0, 250.0 * s.height as f32 / s.width.max(1) as f32).min(vec2(250.0, 300.0));
                        let audio_only = s.format.audio_only();
                        let (rect, _) = ui.allocate_exact_size(if audio_only { vec2(250.0, 0.0) } else { size }, egui::Sense::hover());
                        if !audio_only {
                            ui.painter().rect_filled(rect, theme::R_CARD, Color32::BLACK);
                        }
                        let live = match self.running.as_mut() {
                            Some(r) => {
                                if let Some(pv) = &r.preview {
                                    r.live.poll(ui.ctx(), pv);
                                }
                                let prog = *r.progress.lock().unwrap();
                                paint_live(ui, rect, &r.live, prog)
                            }
                            None => false,
                        };
                        if let Some((id, ts)) = cover.filter(|_| !live && !audio_only) {
                            // Bild füllt das Projektformat (Hochkant zeigt Hochkant, keine Balken)
                            ui.painter().image(id, rect, fill_uv(ts, rect.size()), Color32::WHITE);
                        }
                        ui.add_space(8.0);
                        let secs = s.range.map_or(p.duration(), |(a, b)| b - a).as_secs_f64();
                        let info = if audio_only {
                            format!("{} {secs:.1} s", icon::CLOCK)
                        } else {
                            let (w, h) = s.output_size();
                            format!("{} {secs:.1} s  ·  {w}×{h}", icon::CLOCK)
                        };
                        ui.label(RichText::new(crate::tr(info)).small().color(theme::TEXT_DIM()));
                        if !audio_only {
                            let (est, exact) = estimate_mb(&s, p.duration());
                            let txt = if exact { format!("{} {est:.0} MB", icon::HARD_DRIVES) } else { format!("{} ca. {est:.0} MB", icon::HARD_DRIVES) };
                            ui.label(RichText::new(crate::tr(txt)).small().color(theme::TEXT_DIM()));
                        }
                        if live {
                            ui.label(RichText::new(crate::tr("Live-Vorschau")).size(theme::FS_HINT).color(theme::TEXT_DIM()));
                        }
                    });
                    ui.add_space(12.0);
                    // Rechts: Einstellungen
                    let right = ui.vertical(|ui| {
                        // Formatwechsel blendet Zeilen aus: Höhe halten, damit Knöpfe nicht springen
                        ui.set_min_height(self.right_h);
                        ui.add_enabled_ui(!busy, |ui| {
                            theme::row(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(300.0)));
                            theme::row(ui, "Speichern in", |ui| {
                                // Platz für den Ordner-Knopf lassen, sonst ragt die Zeile über das Fenster hinaus
                                let w = (ui.available_width() - 32.0).max(40.0);
                                ui.allocate_ui(vec2(w, 20.0), |ui| {
                                    ui.add(egui::Label::new(RichText::new(crate::tr(self.folder.display().to_string())).small()).truncate());
                                });
                                if theme::icon_button(ui, icon::FOLDER_OPEN, "Ordner wählen", true, false).clicked() {
                                    if let Some(d) = crate::dialog().set_directory(&self.folder).pick_folder() {
                                        self.folder = d;
                                    }
                                }
                            });
                            theme::row(ui, "Format", |ui| {
                                egui::ComboBox::from_id_salt("exp-format").selected_text(crate::tr(s.format.label())).width(200.0).show_ui(ui, |ui| {
                                    for f in Format::ALL {
                                        ui.selectable_value(&mut s.format, f, crate::tr(f.label()));
                                    }
                                });
                            });
                            if let Some((a, b)) = self.range_avail {
                                let mut only = s.range.is_some();
                                let fmt = |t: Time| format!("{:02}:{:02}.{:02}", (t.as_secs_f64() / 60.0) as u32, t.as_secs_f64() as u32 % 60, ((t.as_secs_f64().fract()) * 100.0) as u32);
                                theme::row(ui, "", |ui| {
                                    if crate::widgets::check(ui, &mut only, crate::tr(format!("Nur In/Out-Bereich ({} – {})", fmt(a), fmt(b)))).changed() {
                                        s.range = only.then_some((a, b));
                                    }
                                });
                            }
                            if !s.format.audio_only() {
                                theme::section(ui, "Video");
                                // Plattform-Vorlagen: setzen Format, Codec, Größe, Bildrate und Qualität
                                theme::row(ui, "Plattform", |ui| {
                                    let sel = self.platform.and_then(platform).map(|pl| crate::tr(pl.name)).unwrap_or_else(|| crate::tr("Plattform wählen …"));
                                    egui::ComboBox::from_id_salt("exp-preset").selected_text(sel).width(200.0).show_ui(ui, |ui| {
                                        for pl in PLATFORMS {
                                            if ui.selectable_label(self.platform == Some(pl.id), crate::tr(pl.name)).clicked() {
                                                Self::apply_platform(&mut s, &mut self.short_edge, &mut self.custom_kbps, pl);
                                                self.platform = Some(pl.id);
                                                self.preset_hint = (!pl.fits(&p.settings)).then(|| crate::tr("Das Projekt hat ein anderes Seitenverhältnis – Format im Projekt-Reiter umstellen oder Schnellaktionen → Hochkant-Layout"));
                                                if pl.fits(&p.settings) {
                                                    self.picked_platform = Some(pl.id);
                                                }
                                            }
                                        }
                                    });
                                });
                                if let Some(h) = &self.preset_hint {
                                    ui.label(egui::RichText::new(h).small().color(theme::WARN_TEXT()));
                                }
                                // Hochkant-Vorgabe im Querformat-Projekt: Format direkt umstellen
                                let vertical = self.platform.and_then(platform).filter(|pl| pl.w * 16 == pl.h * 9 && !pl.fits(&p.settings));
                                if let Some(pl) = vertical.filter(|_| p.settings.width > p.settings.height) {
                                    if ui.add(theme::secondary(format!("{}  Projekt auf 9:16 umstellen", icon::DEVICE_MOBILE))).clicked() {
                                        self.switch_aspect = true;
                                        self.picked_platform = Some(pl.id);
                                        self.preset_hint = None;
                                    }
                                }
                                theme::row(ui, "Auflösung", |ui| {
                                    let label = |e: u32| match e {
                                        0 => format!("Projekt ({}×{})", p.settings.width, p.settings.height),
                                        2160 => "4K (2160p)".into(),
                                        e => format!("{e}p"),
                                    };
                                    egui::ComboBox::from_id_salt("exp-res").selected_text(crate::tr(label(self.short_edge))).width(200.0).show_ui(ui, |ui| {
                                        for e in [0, 480, 720, 1080, 1440, 2160] {
                                            ui.selectable_value(&mut self.short_edge, e, crate::tr(label(e)));
                                        }
                                    });
                                });
                                let (pw, ph) = (p.settings.width.max(2), p.settings.height.max(2));
                                if self.short_edge == 0 {
                                    s.width = pw;
                                    s.height = ph;
                                } else {
                                    let f = self.short_edge as f64 / pw.min(ph) as f64;
                                    s.width = ((pw as f64 * f).round() as u32) & !1;
                                    s.height = ((ph as f64 * f).round() as u32) & !1;
                                }
                                if s.format != Format::Gif {
                                    theme::row(ui, "Codec", |ui| {
                                        if !Codec::for_format(s.format).contains(&s.codec) {
                                            s.codec = Codec::for_format(s.format)[0];
                                        }
                                        egui::ComboBox::from_id_salt("exp-codec").selected_text(crate::tr(s.codec.label())).width(200.0).show_ui(ui, |ui| {
                                            for c in Codec::for_format(s.format) {
                                                ui.selectable_value(&mut s.codec, *c, crate::tr(c.label()));
                                            }
                                        });
                                    });
                                    if s.alpha_possible() {
                                        theme::row(ui, "Hintergrund", |ui| {
                                            crate::widgets::check(ui, &mut s.transparent, crate::tr(if s.codec == Codec::ProRes { "Transparent (ProRes 4444)" } else { "Transparent (VP9 mit Alpha)" }))
                                                .on_hover_text(crate::tr("Leere Bereiche und freigestellte Personen bleiben durchsichtig, z. B. für Overlays in anderen Programmen"));
                                        });
                                    }
                                    theme::row(ui, "Qualität", |ui| {
                                        let label = match s.quality {
                                            Quality::Lower => "Kleinere Datei".to_string(),
                                            Quality::Recommended => "Empfohlen".into(),
                                            Quality::Higher => "Höher".into(),
                                            Quality::Bitrate(k) => format!("{k} kbit/s"),
                                        };
                                        egui::ComboBox::from_id_salt("exp-q").selected_text(crate::tr(label)).width(200.0).show_ui(ui, |ui| {
                                            ui.selectable_value(&mut s.quality, Quality::Lower, crate::tr("Kleinere Datei"));
                                            ui.selectable_value(&mut s.quality, Quality::Recommended, crate::tr("Empfohlen"));
                                            ui.selectable_value(&mut s.quality, Quality::Higher, crate::tr("Höher"));
                                            ui.selectable_value(&mut s.quality, Quality::Bitrate(self.custom_kbps), crate::tr("Benutzerdefinierte Bitrate"));
                                        });
                                    });
                                    if let Quality::Bitrate(_) = s.quality {
                                        theme::row(ui, "", |ui| {
                                            ui.add(egui::DragValue::new(&mut self.custom_kbps).range(500..=200_000).suffix(" kbit/s").speed(100));
                                        });
                                        s.quality = Quality::Bitrate(self.custom_kbps);
                                    }
                                    theme::row(ui, "Bildrate", |ui| {
                                        egui::ComboBox::from_id_salt("exp-fps").selected_text(crate::tr(format!("{:.3} fps", s.fps.as_f64()))).width(200.0).show_ui(ui, |ui| {
                                            for f in ["23.976", "24", "25", "29.97", "30", "50", "59.94", "60"] {
                                                let r = cl_core::Rational::parse(f).unwrap();
                                                ui.selectable_value(&mut s.fps, r, crate::tr(format!("{f} fps")));
                                            }
                                        });
                                    });
                                    let encs = self.encoders.lock().unwrap().clone().unwrap_or_default();
                                    theme::row(ui, "Encoder", |ui| {
                                        let name = |e: Encoder| match e {
                                            Encoder::Software => "Software (beste Qualität)",
                                            Encoder::Nvenc => "NVIDIA NVENC (schnell)",
                                            Encoder::Qsv => "Intel Quick Sync (schnell)",
                                            Encoder::Amf => "AMD AMF (schnell)",
                                        };
                                        egui::ComboBox::from_id_salt("exp-enc").selected_text(crate::tr(name(s.encoder))).width(200.0).show_ui(ui, |ui| {
                                            for e in encs {
                                                ui.selectable_value(&mut s.encoder, e, crate::tr(name(e)));
                                            }
                                        });
                                    });
                                    theme::section(ui, "Ton");
                                    crate::widgets::check(ui, &mut s.include_audio, crate::tr(format!("Ton exportieren (AAC {} kbit/s, bei WebM Opus)", s.audio_kbps.unwrap_or(256))));
                                    ui.add_enabled_ui(s.include_audio, |ui| loudness_row(ui, &mut s.loudness));
                                }
                            } else {
                                loudness_row(ui, &mut s.loudness);
                            }
                            if !s.format.audio_only() {
                                crate::widgets::check(ui, &mut self.also_srt, crate::tr("Texte zusätzlich als Untertiteldateien (SRT und VTT) speichern"));
                                let ms = &p.timeline.markers;
                                if self.chapters && !ms.is_empty() {
                                    theme::section(ui, "Kapitel");
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(format!("{} {}", ms.len(), crate::tr("aus Markern"))).small().color(theme::TEXT_DIM()));
                                        if ui.add(theme::secondary(format!("{}  Kapitel kopieren", icon::COPY))).clicked() {
                                            ui.ctx().copy_text(crate::navigate::chapters(ms));
                                            toast = Some(crate::tr("Kapitel in die Zwischenablage kopiert"));
                                        }
                                    });
                                    let (a, b) = s.range.unwrap_or((Time::ZERO, p.duration()));
                                    if let Some(w) = crate::navigate::chapter_issue(ms, a, b).filter(|_| self.chapter_hint) {
                                        if crate::navigate::chapter_warning(ui, &w) {
                                            self.chapter_hint = false;
                                            self.chapter_hint_off = true;
                                        }
                                    }
                                }
                            }
                        });
                    });
                    self.right_h = self.right_h.max(right.response.rect.height());
                });

                ui.add_space(12.0);
                if !self.missing.is_empty() && self.running.is_none() {
                    ui.label(
                        RichText::new(crate::tr(format!(
                            "{}  {}",
                            icon::WARNING,
                            theme::count_text(
                                self.missing.len(),
                                "{} Inhalt fehlt und wird ohne ihn exportiert: {}",
                                "{} Inhalte fehlen und werden ohne sie exportiert: {}",
                            )
                            .replacen("{}", &self.missing.join(", "), 1)
                        )))
                        .small()
                        .color(theme::WARN_TEXT()),
                    );
                }
                if let Some(e) = &self.error {
                    ui.label(RichText::new(crate::tr(format!("{} {e}", icon::WARNING_CIRCLE))).color(theme::DANGER_TEXT()));
                }
                if let Some(r) = &self.running {
                    let prog = *r.progress.lock().unwrap();
                    export_progress(ui, prog, r.started.elapsed().as_secs_f32(), r.measures && prog < cl_export::MEASURE_SHARE);
                } else if let Some((at, secs)) = self.done {
                    if !export_done(ui, at, secs) {
                        self.done = None;
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if let Some(out) = &self.last_output {
                        if self.running.is_none() && out.exists() && self.exported.as_ref().is_none_or(|(r, f, used)| *r != self.rev || f != out || *used != s) {
                            if ui.add(theme::secondary(format!("{}  Im Explorer zeigen", icon::FOLDER_OPEN))).clicked() {
                                crate::home::reveal(out);
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.running.is_some() {
                            if ui.add(theme::secondary(format!("{}  Abbrechen", icon::X))).clicked() {
                                if let Some(r) = &self.running {
                                    r.cancel.store(true, Ordering::Relaxed);
                                }
                            }
                        } else if self.unchanged_since_export(&s) {
                            // unverändert seit dem letzten Export: direkt zur fertigen Datei
                            if ui.add(theme::primary(format!("{}  Zur Datei", icon::FOLDER_OPEN))).clicked() || std::mem::take(&mut main) {
                                if let Some((_, f, _)) = &self.exported {
                                    crate::home::reveal(f);
                                }
                            }
                            if ui.add(theme::secondary(format!("{}  Erneut exportieren", icon::ARROW_CLOCKWISE))).clicked() {
                                start = true;
                                overwrite = true;
                            }
                        } else if ui.add(theme::primary(format!("{}  Exportieren", icon::EXPORT))).clicked() || std::mem::take(&mut main) {
                            start = true;
                        }
                        let close_label = if self.done.is_some() || self.unchanged_since_export(&s) { "Schließen" } else { "Abbrechen" };
                        if self.running.is_none() && ui.add(theme::secondary(close_label)).clicked() {
                            open = false;
                        }
                    });
                });
            });
        self.settings = Some(s.clone());
        if !open {
            self.open = false;
        }
        if start && !overwrite && self.output_path(s.format).exists() {
            self.confirm_overwrite = true;
            self.confirm_fresh = true;
            start = false;
        }
        if self.confirm_overwrite {
            // 0 = überschreiben, 1 = anderer Name, 2 = abbrechen
            let mut choice = None;
            let file = self.output_path(s.format);
            let m = crate::dialog::modal(ctx, "export-overwrite").show(ctx, |ui| {
                ui.set_width(380.0);
                if crate::dialog::title(ui, "Datei existiert bereits") {
                    choice = Some(2);
                }
                ui.label(RichText::new(crate::tr(file.display().to_string())).small().color(theme::TEXT_DIM()));
                crate::dialog::footer(ui, |ui| {
                    if ui.add(theme::primary("Anderen Namen")).clicked() {
                        choice = Some(1);
                    }
                    if ui.add(theme::danger("Überschreiben")).clicked() {
                        choice = Some(0);
                    }
                    if ui.add(theme::secondary("Abbrechen")).clicked() {
                        choice = Some(2);
                    }
                });
            });
            let fresh = std::mem::take(&mut self.confirm_fresh);
            if fresh {
                ctx.request_repaint();
            } else if choice.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                // Enter nie zum Überschreiben: ungefährliche Hauptaktion
                choice = Some(1);
            } else if choice.is_none() && m.should_close() {
                choice = Some(2);
            }
            match choice {
                Some(0) => start = true,
                Some(1) => {
                    let base = self.name.trim().to_string();
                    for n in 2.. {
                        self.name = format!("{base} ({n})");
                        if !self.output_path(s.format).exists() {
                            break;
                        }
                    }
                    start = true;
                }
                _ => {}
            }
            if choice.is_some() {
                self.confirm_overwrite = false;
            }
        }
        if start {
            self.error = None;
            let out = self.output_path(s.format);
            let sidecars = sidecars(p, &s, self.also_srt, &out);
            let fonts = fonts.clone();
            let resolver = move |fam: Option<&str>| fam.and_then(|f| fonts.get(f).cloned());
            match cl_export::build(p, &s, &resolver, &out) {
                Ok(job) => {
                    let measures = job.measures_loudness();
                    let preview = job.preview_path().map(Path::to_path_buf);
                    let progress = Arc::new(Mutex::new(0.0));
                    let result = Arc::new(Mutex::new(None));
                    let cancel = Arc::new(AtomicBool::new(false));
                    let (pr, rs, cc, c2) = (
                        progress.clone(),
                        result.clone(),
                        cancel.clone(),
                        ctx.clone(),
                    );
                    std::thread::spawn(move || {
                        let r = cl_export::run(&job, cc, |x| {
                            *pr.lock().unwrap() = x;
                            c2.request_repaint();
                        });
                        *rs.lock().unwrap() = Some(r.map_err(|e| e.to_string()));
                        c2.request_repaint();
                    });
                    self.done = None;
                    self.running = Some(Running {
                        progress,
                        measures,
                        result,
                        cancel,
                        started: Instant::now(),
                        rev: self.rev,
                        sidecars,
                        settings: s.clone(),
                        preview,
                        live: Live::default(),
                    });
                    self.last_output = Some(out);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        toast
    }
}

/// Begleitdateien neben `out` (Pfad, Inhalt) für den Exportbereich: Untertitel (SRT, VTT)
/// und Kapitel wie „Kapitel kopieren“ (nur wenn eingeschaltet), Zeiten ab Bereichsanfang.
fn sidecars(p: &Project, s: &ExportSettings, subs: bool, out: &Path) -> Vec<(PathBuf, String)> {
    let mut v = Vec::new();
    if s.format.audio_only() {
        return v;
    }
    let (a, b) = s.range.unwrap_or((Time::ZERO, p.duration()));
    if subs {
        v.push((out.with_extension("srt"), cl_export::to_srt_in(p, a, b)));
        v.push((out.with_extension("vtt"), cl_export::to_vtt_in(p, a, b)));
    }
    let ms = &p.timeline.markers;
    if s.chapters && ms.iter().any(|m| m.at >= a && m.at < b) {
        let txt = cl_export::chapter_list(ms, a, b, &s.chapter_name)
            .iter()
            .map(|(t, n)| format!("{} {n}", crate::navigate::chapter_time(*t)))
            .collect::<Vec<_>>()
            .join("\n");
        v.push((out.with_extension("chapters.txt"), txt));
    }
    v
}

/// Fortschritt beim Export: Text mit Prozent und Restzeit, darunter die Fortschrittslinie.
fn export_progress(ui: &mut egui::Ui, prog: f32, elapsed: f32, measuring: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::tr(if measuring { "Lautheit messen …" } else { "Exportiere …" })).color(theme::TEXT()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if prog > 0.03 && elapsed > 2.0 {
                let left = (elapsed / prog * (1.0 - prog)).max(0.0) as u32;
                ui.label(
                    RichText::new(crate::tr(format!("noch ca. {}:{:02}", left / 60, left % 60)))
                        .color(theme::TEXT_DIM()),
                );
            }
            ui.label(RichText::new(format!("{:.0} %", prog * 100.0)).strong());
        });
    });
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    crate::widgets::progress_line(ui.painter(), egui::Id::new("export-prog"), rect, prog);
    // Restzeit läuft auch ohne neue Fortschrittsmeldung weiter
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
}

/// Erfolg: Häkchen wächst weich auf (`MOTION_SLOW`), bleibt 6 s sichtbar. `false` = ausgeblendet.
fn export_done(ui: &mut egui::Ui, at: Instant, secs: f32) -> bool {
    let age = at.elapsed().as_secs_f32();
    if age > 6.0 {
        return false;
    }
    let dur = theme::anim(theme::MOTION_SLOW);
    let pop = if dur > 0.0 { 0.6 + 0.4 * theme::ease(age / dur) } else { 1.0 };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::hover());
        let p = ui.painter();
        p.circle_filled(r.center(), 11.0 * pop, theme::SUCCESS);
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            icon::CHECK,
            egui::FontId::proportional(13.0 * pop.max(0.01)),
            theme::SUCCESS_BG,
        );
        ui.label(RichText::new(crate::tr(format!("Fertig in {secs:.1} s"))).strong());
    });
    if age < dur {
        ui.ctx().request_repaint();
    } else {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    true
}

/// „Lautheit angleichen“: YouTube (−14 LUFS), Sprache (−16 LUFS) oder aus.
fn loudness_row(ui: &mut egui::Ui, l: &mut Option<f32>) {
    let name = |l: Option<f32>| match l {
        Some(x) if x > -15.0 => "YouTube (−14 LUFS)",
        Some(_) => "Sprache (−16 LUFS)",
        None => "Aus",
    };
    theme::row(ui, "Lautheit", |ui| {
        egui::ComboBox::from_id_salt("exp-loud").selected_text(crate::tr(name(*l))).width(200.0).show_ui(ui, |ui| {
            for v in [Some(-14.0), Some(-16.0), None] {
                ui.selectable_value(l, v, crate::tr(name(v)));
            }
        })
        .response
        .on_hover_text(crate::tr("Misst den Ton vorab und hebt oder senkt ihn gleichmäßig, Spitzen höchstens −1 dBTP"));
    });
}

/// Dateigröße schätzen; `true` = aus fester Zielbitrate (Video + Ton) × Dauer.
/// Bei konstanter Qualität (CRF) grob aus Pixel × Bildrate, geeicht auf
/// 1080p30 „Empfohlen“ ≈ 23 Mbit/s (was libx264 mit CRF 20 tatsächlich liefert);
/// je +6 CRF etwa halbe Größe (Kleiner = CRF 26, Höher = CRF 16). Andere Codecs im Verhältnis
/// dazu (VP9/AV1 mit ihren CRF-Werten etwa 0,3, HEVC 0,5), ProRes fest ≈ 220 Mbit/s (HQ),
/// GIF aus Fläche × Bildern (Palette mit Rastern, bei Filmmaterial ≈ 0,6 Byte je Pixel).
fn estimate_mb(s: &ExportSettings, d: Time) -> (f64, bool) {
    let secs = s.range.map_or(d.as_secs_f64(), |(a, b)| (b.as_secs_f64() - a.as_secs_f64()).max(0.0));
    let (w, h) = s.output_size();
    let px = (w * h) as f64 / (1920.0 * 1080.0);
    if s.format == Format::Gif {
        let frames = s.fps.as_f64().min(cl_export::GIF_FPS as f64) * secs;
        return (px * 1920.0 * 1080.0 * frames * 0.6 / 1e6, false);
    }
    let fps = s.fps.as_f64() / 30.0;
    let kbps = match (s.codec, s.quality) {
        (Codec::ProRes, _) => px * fps * if s.alpha() { 330_000.0 } else { 220_000.0 },
        (_, Quality::Bitrate(k)) => k as f64,
        (c, q) => {
            let codec = match c {
                Codec::Vp9 | Codec::Av1 => 0.3,
                Codec::Hevc => 0.5,
                _ => 1.0,
            };
            px * 23_000.0 * fps * codec * match q {
                Quality::Lower => 0.5,
                Quality::Higher => 2f64.powf(4.0 / 6.0),
                _ => 1.0,
            }
        }
    };
    let exact = matches!(s.quality, Quality::Bitrate(_)) && s.codec != Codec::ProRes;
    ((kbps + s.audio_bitrate() as f64) * secs / 8.0 / 1000.0, exact)
}

/// UV-Ausschnitt eines Bilds der Größe `ts`, der `area` ohne Balken füllt (mittig beschnitten).
fn fill_uv(ts: egui::Vec2, area: egui::Vec2) -> Rect {
    let (img, want) = (ts.x / ts.y.max(1.0), area.x / area.y.max(1.0));
    let size = if img > want { vec2(want / img, 1.0) } else { vec2(1.0, img / want) };
    Rect::from_center_size(egui::pos2(0.5, 0.5), size)
}

#[test]
fn youtube_presets_hit_official_bitrates() {
    let mut s = ExportSettings::from_project(&Project::new("t"));
    let (mut edge, mut kbps) = (0, 0);
    for (id, fps, want) in [
        ("youtube-1080", "30", 8_000),
        ("youtube-1080", "60", 12_000),
        ("youtube-1440", "29.97", 16_000),
        ("youtube-1440", "50", 24_000),
        ("youtube-4k", "24", 40_000),
        ("youtube-4k", "60", 60_000),
    ] {
        s.fps = cl_core::Rational::parse(fps).unwrap();
        ExportDialog::apply_platform(&mut s, &mut edge, &mut kbps, platform(id).unwrap());
        assert_eq!((s.quality, kbps), (Quality::Bitrate(want), want), "{id} {fps}");
        assert_eq!((s.audio_kbps, s.loudness), (Some(384), Some(-14.0)));
    }
    // andere Vorgaben: eigene Qualität, Lautheit aus
    ExportDialog::apply_platform(&mut s, &mut edge, &mut kbps, platform("small").unwrap());
    assert_eq!((s.quality, s.audio_kbps, s.loudness), (Quality::Lower, None, None));
    // Größe aus Zielbitrate: (8000 + 384) kbit/s × 60 s
    s.quality = Quality::Bitrate(8_000);
    s.audio_kbps = Some(384);
    let (mb, exact) = estimate_mb(&s, Time::from_secs_f64(60.0));
    assert!(exact && (mb - 62.88).abs() < 0.01, "{mb}");
}

#[test]
fn estimate_per_format_within_factor_two_of_real_exports() {
    // echte Exporte aus dem Klicktest (Strandvideo 1080p30, In/Out-Bereich)
    let within = |mb: f64, real: f64| (real / 2.0..=real * 2.0).contains(&mb);
    let mut s = ExportSettings::from_project(&Project::new("t"));
    (s.width, s.height, s.fps) = (1920, 1080, cl_core::Rational::parse("30").unwrap());
    s.range = Some((Time::from_secs_f64(5.03), Time::from_secs_f64(9.038)));
    // WebM/VP9 „Empfohlen“ + Opus, 4,008 s: 3,73 MB (vorher geschätzt 12 MB)
    (s.format, s.codec) = (Format::WebM, Codec::Vp9);
    let (mb, _) = estimate_mb(&s, Time::ZERO);
    assert!(within(mb, 3.73), "WebM {mb}");
    // GIF 480p gewählt (852×480 → 720×406, 15 Bilder/s), 4,0 s: 11,1 MB (vorher 2 MB)
    (s.format, s.width, s.height) = (Format::Gif, 852, 480);
    s.range = Some((Time::from_secs_f64(5.0), Time::from_secs_f64(9.0)));
    let (mb, exact) = estimate_mb(&s, Time::ZERO);
    assert!(within(mb, 11.08) && !exact, "GIF {mb}");
    // Dialog zeigt dieselbe Größe, die der Export schreibt
    assert_eq!(s.output_size(), (720, 406));
    // Hochkant-Shorts H.264 „Höher“, 18 s: 17,6 MB – Standbilder, CRF schätzt hier zu hoch;
    // nur grob plausibel (keine Größenordnung daneben)
    (s.format, s.codec, s.quality, s.width, s.height) = (Format::Mp4, Codec::H264, Quality::Higher, 1080, 1920);
    s.range = None;
    let (mb, _) = estimate_mb(&s, Time::from_secs_f64(18.0));
    assert!(mb > 17.6 && mb < 17.6 * 10.0, "Shorts {mb}");
}

#[test]
fn every_text_style_reaches_the_export_like_the_preview() {
    use cl_core::{Clip, ClipContent, TextStyle, Track, TrackKind};
    let styles: Vec<(String, TextStyle)> = crate::captions::PRESETS
        .iter()
        .map(|c| (c.name.to_string(), (c.style)()))
        .chain(cl_fx::text::TEMPLATES.iter().map(|t| (t.name.to_string(), t.make())))
        .collect();
    assert!(styles.iter().any(|(_, s)| s.italic) && styles.iter().any(|(_, s)| s.bold));
    let dir = tempfile::tempdir().unwrap();
    let none = |_: Option<&str>| None;
    for (name, st) in styles {
        let mut p = Project::new("t");
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(ClipContent::Text(st.clone()), Time::ZERO, Time::from_secs_f64(1.0)));
        p.timeline.tracks.push(t);
        let job = cl_export::build(&p, &ExportSettings::from_project(&p), &none, &dir.path().join("o.mp4")).unwrap();
        let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
        let [r, gr, b, a] = st.color;
        assert!(g.contains(&format!("fontcolor=0x{r:02x}{gr:02x}{b:02x}@{:.3}", a as f32 / 255.0)), "{name}: Farbe");
        // Schnitt aus denselben Schaltern wie die Vorschau: fett = zweiter Abdruck, kursiv = Scherung
        assert_eq!(g.matches("drawtext=").count(), if st.bold { 2 } else { 1 }, "{name}: fett");
        assert_eq!(g.contains("shear=shx=0.25"), st.italic, "{name}: kursiv");
        assert_eq!(g.contains(":borderw="), st.outline.is_some(), "{name}: Umriss");
        assert_eq!(g.contains("box=1"), st.background.is_some(), "{name}: Hintergrund");
        assert_eq!(g.contains("shadowx="), st.shadow, "{name}: Schatten");
    }
}

#[test]
fn cover_fills_portrait_frame_without_bars() {
    // 16:9-Bild in 9:16-Fläche: mittiger senkrechter Streifen, volle Höhe
    let uv = fill_uv(vec2(1920.0, 1080.0), vec2(250.0, 444.0));
    assert!((uv.height() - 1.0).abs() < 1e-4 && (uv.width() - 0.3166).abs() < 1e-3 && (uv.center().x - 0.5).abs() < 1e-6, "{uv:?}");
    // gleiches Format: ganzes Bild
    assert_eq!(fill_uv(vec2(1920.0, 1080.0), vec2(250.0, 140.625)), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)));
}

#[test]
fn estimate_matches_a_real_1080p30_export() {
    let mut s = ExportSettings::from_project(&Project::new("t"));
    (s.width, s.height, s.fps) = (1920, 1080, cl_core::Rational::parse("30").unwrap());
    let (mb, exact) = estimate_mb(&s, Time::from_secs_f64(40.0));
    assert!(!exact);
    // echte Datei: 117,6 MB
    assert!((80.0..=160.0).contains(&mb), "{mb}");
}

#[cfg(test)]
fn frame(d: &mut ExportDialog, ctx: &egui::Context, p: &Project, enter: bool) {
    let events = if enter {
        vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]
    } else {
        Vec::new()
    };
    let _ = ctx.run(egui::RawInput { events, ..Default::default() }, |ctx| {
        d.show(ctx, p, &HashMap::new(), None, 0);
    });
}

#[test]
fn in_out_range_is_preselected_on_every_open() {
    let p = Project::new("t");
    let mut d = ExportDialog::default();
    let r = (Time::from_secs_f64(2.0), Time::from_secs_f64(5.0));
    d.open_for(&p, None, 0, Some(r));
    assert_eq!(d.settings.as_ref().unwrap().range, Some(r));
    // Bereich inzwischen gelöscht: kein alter Bereich mehr
    d.open_for(&p, None, 0, None);
    assert_eq!((d.settings.as_ref().unwrap().range, d.range_avail), (None, None));
}

#[test]
fn enter_that_opened_overwrite_prompt_does_not_confirm_it() {
    let (ctx, p) = (egui::Context::default(), Project::new("t"));
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(&p, None, 0, None);
    // wie nach Enter auf „Exportieren“: Nachfrage im selben Bild geöffnet, Enter noch gedrückt
    (d.confirm_overwrite, d.confirm_fresh) = (true, true);
    frame(&mut d, &ctx, &p, true);
    assert!(d.confirm_overwrite && d.error.is_none());
    // erst ein neues Enter bestätigt – mit „Anderen Namen“, nie Überschreiben
    // (leeres Projekt: Start scheitert mit Fehler)
    frame(&mut d, &ctx, &p, true);
    assert!(!d.confirm_overwrite && d.error.is_some());
    assert!(d.name.starts_with("t ("), "{}", d.name);
}

#[test]
fn reexport_only_with_unchanged_settings() {
    let (dir, p) = (tempfile::tempdir().unwrap(), Project::new("t"));
    let mut d = ExportDialog { folder: dir.path().to_path_buf(), name: "t".into(), ..Default::default() };
    let s = ExportSettings::from_project(&p);
    let out = d.output_path(s.format);
    std::fs::write(&out, b"x").unwrap();
    d.exported = Some((0, out, s.clone()));
    assert!(d.unchanged_since_export(&s));
    // andere Qualität: wieder „Exportieren“ (fragt bei vorhandener Datei nach)
    let mut q = s.clone();
    q.quality = Quality::Bitrate(1234);
    assert!(!d.unchanged_since_export(&q));
    d.rev = 1;
    assert!(!d.unchanged_since_export(&s));
}

#[test]
fn live_preview_reload_blend_and_partial_jpeg() {
    let t = std::time::SystemTime::UNIX_EPOCH;
    assert!(!changed(None, None) && !changed(Some((t, 5)), None) && !changed(Some((t, 5)), Some((t, 5))));
    assert!(changed(None, Some((t, 5))) && changed(Some((t, 5)), Some((t, 6))));
    assert_eq!((blend(0.0, 0.25), blend(0.125, 0.25), blend(1.0, 0.25), blend(0.0, 0.0)), (0.0, 0.5, 1.0, 1.0));
    let mut jpg = Vec::new();
    image::RgbImage::from_pixel(48, 27, image::Rgb([200, 30, 30]))
        .write_to(&mut std::io::Cursor::new(&mut jpg), image::ImageFormat::Jpeg)
        .unwrap();
    assert_eq!(decode_jpeg(&jpg).unwrap().size, [48, 27]);
    // halb geschriebene Datei: übersprungen
    assert!(decode_jpeg(&jpg[..jpg.len() / 2]).is_none() && decode_jpeg(&[]).is_none());
    // Datei fehlt noch / unverändert: kein Bild, nichts geladen
    let ctx = egui::Context::default();
    let dir = tempfile::tempdir().unwrap();
    let pv = dir.path().join("preview.jpg");
    let mut live = Live::default();
    live.poll(&ctx, &pv);
    assert!(live.cur.is_none());
    std::fs::write(&pv, &jpg).unwrap();
    live.checked = None;
    live.poll(&ctx, &pv);
    assert!(live.cur.is_some() && live.prev.is_none());
    // kaputt überschrieben: letztes gutes Bild bleibt
    std::fs::write(&pv, &jpg[..10]).unwrap();
    live.checked = None;
    live.poll(&ctx, &pv);
    assert!(live.cur.is_some() && live.prev.is_none());
}

#[cfg(test)]
fn running(result: Option<Result<(), String>>) -> Running {
    Running {
        progress: Default::default(),
        measures: false,
        result: Arc::new(Mutex::new(result)),
        cancel: Default::default(),
        started: Instant::now(),
        rev: 0,
        sidecars: Vec::new(),
        settings: ExportSettings::from_project(&Project::new("t")),
        preview: None,
        live: Live::default(),
    }
}

#[test]
fn window_close_during_export_asks_and_cancels() {
    assert!(close_allowed(false, false) && close_allowed(true, true) && !close_allowed(true, false));
    let ctx = egui::Context::default();
    let close = || {
        let mut raw = egui::RawInput::default();
        raw.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo { events: vec![egui::ViewportEvent::Close], ..Default::default() },
        );
        raw
    };
    // ohne Export: kein Aufhalten
    let mut d = ExportDialog::default();
    let out = ctx.run(close(), |ctx| d.guard_close(ctx));
    assert!(!d.quit_ask && !out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
    // mit laufendem Export: Schließen aufgehalten, Rückfrage offen
    d.running = Some(running(None));
    let out = ctx.run(close(), |ctx| d.guard_close(ctx));
    assert!(d.quit_ask && out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
    // Beenden bestätigt: der zweite Schließ-Wunsch geht durch
    d.quit_ok = true;
    d.quit_ask = false;
    let out = ctx.run(close(), |ctx| d.guard_close(ctx));
    assert!(!d.quit_ask && !out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
    // Beenden wartet, bis der Export-Faden den Abbruch gemeldet hat
    let r = running(None);
    let (cancel, result) = (r.cancel.clone(), r.result.clone());
    d.running = Some(r);
    std::thread::spawn(move || {
        while !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        *result.lock().unwrap() = Some(Err("Abgebrochen".into()));
    });
    d.cancel_and_wait(std::time::Duration::from_secs(5));
    assert!(d.running.as_ref().unwrap().result.lock().unwrap().is_some());
}

#[test]
fn sidecars_follow_range_and_wait_for_success() {
    use cl_core::{Clip, ClipContent, Marker, TextStyle, Track, TrackKind};
    let mut p = Project::new("t");
    let mut t = Track::new(TrackKind::Text);
    let txt = |s: &str| ClipContent::Text(TextStyle { text: s.into(), ..Default::default() });
    t.clips.push(Clip::new(txt("drin"), Time::from_secs_f64(50.0), Time::from_secs_f64(20.0)));
    t.clips.push(Clip::new(txt("draußen"), Time::from_secs_f64(130.0), Time::from_secs_f64(5.0)));
    p.timeline.tracks.push(t);
    let mk = |id, at: f64, name: &str| Marker { id, at: Time::from_secs_f64(at), duration: Time::ZERO, name: name.into(), note: String::new(), color: 0 };
    p.timeline.markers = vec![mk(1, 75.0, "Mitte"), mk(2, 125.0, "Später")];
    let mut s = ExportSettings::from_project(&p);
    s.range = Some((Time::from_secs_f64(60.0), Time::from_secs_f64(120.0)));
    let dir = tempfile::tempdir().unwrap();
    let v = sidecars(&p, &s, true, &dir.path().join("a.mp4"));
    let get = |ext: &str| v.iter().find(|(f, _)| f.to_string_lossy().ends_with(ext)).unwrap().1.clone();
    assert_eq!(get(".chapters.txt"), "0:00 Intro\n0:15 Mitte");
    // Kapitel ausgeschaltet: keine Kapiteldatei, Untertitel bleiben
    let off = sidecars(&p, &ExportSettings { chapters: false, ..s.clone() }, true, &dir.path().join("a.mp4"));
    assert_eq!(off.len(), 2);
    assert!(off.iter().all(|(f, _)| !f.to_string_lossy().ends_with(".chapters.txt")));
    assert_eq!(get(".srt"), "1\n00:00:00,000 --> 00:00:10,000\ndrin\n");
    // Begleitdateien erst nach erfolgreichem Export schreiben
    let (ctx, mut d) = (egui::Context::default(), ExportDialog::default());
    for ok in [false, true] {
        d.running = Some(Running {
            progress: Default::default(),
            measures: false,
            result: Arc::new(Mutex::new(Some(if ok { Ok(()) } else { Err("x".into()) }))),
            cancel: Default::default(),
            started: Instant::now(),
            rev: 0,
            sidecars: v.clone(),
            settings: s.clone(),
            preview: None,
            live: Live::default(),
        });
        frame(&mut d, &ctx, &p, false);
        assert_eq!(v.iter().all(|(f, _)| f.is_file()), ok);
    }
}
