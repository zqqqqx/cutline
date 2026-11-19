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

/// Höchstzahl wartender Exporte. Ein Abbild ist eine volle `Project`-Kopie ohne Medien, gemessen
/// ≈1,1 KB je Clip (siehe `cl_core::Document`): 3000 Clips ≈3,3 MB, 20 Abbilder ≈66 MB.
pub const QUEUE_MAX: usize = 20;

/// Export-Auftrag: Abbild des Projekts beim Einreihen, damit Weiterschneiden oder ein
/// Projektwechsel ihn nicht mehr verändern.
struct Queued {
    project: Arc<Project>,
    /// Projektdatei (Zuordnung in der Exporte-Liste)
    project_path: Option<PathBuf>,
    /// Einstellungen beim Einreihen (Fingerabdruck für „Erneut exportieren“)
    settings: ExportSettings,
    /// Zieldatei; bei „In Teile aufteilen“ der Ordner der Teile (eine Gruppe, ein Eintrag)
    out: PathBuf,
    /// nacheinander exportiert; ein normaler Export hat genau einen Teil mit `out`
    parts: Vec<Part>,
    fonts: HashMap<String, (PathBuf, u32)>,
    /// Projektstand beim Einreihen
    rev: u64,
}

/// Ein Teil eines Exports: Bereich, Zieldatei und Begleitdateien (Untertitel, Kapitel) mit Text
/// vom Einreihen; diese werden erst nach Erfolg des Teils geschrieben.
struct Part {
    range: Option<(Time, Time)>,
    out: PathBuf,
    sidecars: Vec<(PathBuf, String)>,
}

impl Queued {
    /// Teile in einem eigenen Ordner („In Teile aufteilen“)
    fn group(&self) -> bool {
        self.parts.first().is_none_or(|x| x.out != self.out)
    }
}

struct Running {
    progress: Arc<Mutex<f32>>,
    /// laufender Teil (ab 0) und fertige Teile
    part: Arc<std::sync::atomic::AtomicUsize>,
    finished: Arc<Mutex<Vec<usize>>>,
    /// erst nach dem Bau des Auftrags im Hintergrund bekannt (bis dahin „Wird vorbereitet …“):
    /// wird die Lautheit vorher gemessen (erster Teil des Fortschritts), Vorschaudatei von FFmpeg
    job: Arc<Mutex<Option<(bool, Option<PathBuf>)>>>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    src: Queued,
    /// geladene Live-Bilder; Texturen werden mit `Running` freigegeben
    live: Live,
}

impl Running {
    /// Auftrag im Hintergrund bauen und ausführen; liest nur das Abbild, nie das offene Projekt.
    fn start(ctx: &egui::Context, src: Queued) -> Self {
        let progress = Arc::new(Mutex::new(0.0));
        let job_info = Arc::new(Mutex::new(None));
        let result = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        let (part, finished) = (Arc::new(std::sync::atomic::AtomicUsize::new(0)), Arc::new(Mutex::new(Vec::new())));
        let (pr, ji, rs, cc, c2) = (progress.clone(), job_info.clone(), result.clone(), cancel.clone(), ctx.clone());
        let (pi, fin) = (part.clone(), finished.clone());
        let (p, s2, fonts) = (src.project.clone(), src.settings.clone(), src.fonts.clone());
        let parts: Vec<(Option<(Time, Time)>, PathBuf)> = src.parts.iter().map(|x| (x.range, x.out.clone())).collect();
        let folder = src.group().then(|| src.out.clone());
        // noch im Oberflächen-Faden: ein abgebrochener Vorgänger, dessen Faden noch läuft, sieht
        // den Ordner ab jetzt als belegt und löscht ihn nicht mehr
        if let Some(d) = &folder {
            folder_use(d, true);
        }
        let resolver = move |fam: Option<&str>| fam.and_then(|f| fonts.get(f).cloned());
        // Bau des Auftrags (bei großen Projekten Sekunden: Tonprüfung je Datei) nicht im Oberflächen-Faden
        std::thread::spawn(move || {
            // Teile nacheinander; ein fehlgeschlagener hält die übrigen nicht an, Abbrechen alle
            let len = |r: &Option<(Time, Time)>| r.map_or(p.duration(), |(a, b)| b - a).as_secs_f64().max(1e-3);
            let total: f64 = parts.iter().map(|x| len(&x.0)).sum();
            let (mut at, mut first_err) = (0.0, None);
            if let Some(d) = &folder {
                let _ = std::fs::create_dir_all(d);
            }
            for (i, (range, out)) in parts.iter().enumerate() {
                if cc.load(Ordering::Relaxed) {
                    first_err = Some(cl_export::ExportError::Cancelled.to_string());
                    break;
                }
                pi.store(i, Ordering::Relaxed);
                let s = ExportSettings { range: *range, ..s2.clone() };
                let w = (len(range) / total) as f32;
                let r = cl_export::build(&p, &s, &resolver, out).and_then(|job| {
                    if cc.load(Ordering::Relaxed) {
                        return Err(cl_export::ExportError::Cancelled);
                    }
                    *ji.lock().unwrap() = Some((job.measures_loudness(), job.preview_path().map(Path::to_path_buf)));
                    c2.request_repaint();
                    let r = cl_export::run(&job, cc.clone(), |x| {
                        *pr.lock().unwrap() = at + x * w;
                        c2.request_repaint();
                    });
                    if let Err(e) = &r { log::error!(target: "export", "{} · Befehl: {}", crate::applog::anon(&e.to_string()), crate::applog::ffmpeg_cmd(e.failed_args().unwrap_or(&job.args))); }
                    r
                });
                match r {
                    Ok(()) => fin.lock().unwrap().push(i),
                    Err(e) => {
                        let stop = matches!(e, cl_export::ExportError::Cancelled);
                        first_err.get_or_insert(e.to_string());
                        if stop {
                            break;
                        }
                    }
                }
                at += w;
            }
            // abgebrochen, bevor ein Teil fertig war: kein leerer Ordner (nicht leer oder von einem
            // neueren Export belegt: bleibt)
            if let Some(d) = &folder {
                folder_use(d, false);
            }
            *rs.lock().unwrap() = Some(first_err.map_or(Ok(()), Err));
            c2.request_repaint();
        });
        Running { progress, part, finished, job: job_info, result, cancel, started: Instant::now(), src, live: Live::default() }
    }

    /// Gruppe: laufender Teil und Zahl der Teile (ab 1)
    fn part_of(&self) -> Option<(usize, usize)> {
        self.src.group().then(|| (self.part.load(Ordering::Relaxed) + 1, self.src.parts.len()))
    }
}

/// Ordner von Teil-Exporten, die gerade laufen (Zahl je Ordner). Nach einem Abbruch während
/// der Vorbereitung meldet `poll` sofort, der Faden räumt aber erst danach auf; startet inzwischen
/// ein neuer Export in denselben Ordner, darf dieses Aufräumen ihn nicht löschen.
static FOLDERS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Ordner belegen (`true`) bzw. freigeben; beim letzten Freigeben wird er gelöscht, wenn er leer ist.
fn folder_use(dir: &Path, take: bool) {
    let mut v = FOLDERS.lock().unwrap_or_else(|e| e.into_inner());
    if take {
        v.push(dir.to_path_buf());
    } else if let Some(i) = v.iter().position(|d| d == dir) {
        v.swap_remove(i);
        if !v.iter().any(|d| d == dir) {
            let _ = std::fs::remove_dir(dir);
        }
    }
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
    /// Encoder in diesem Dialog von Hand gewählt (sonst Vorauswahl nach den Einstellungen)
    encoder_picked: bool,
    running: Option<Running>,
    /// wartende Exporte, der vorderste startet nach dem laufenden.
    /// shortcut: nur im Speicher, überlebt keinen Neustart; erst speichern, wenn das gewünscht wird
    queue: std::collections::VecDeque<Queued>,
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
    /// rechte Spalte im letzten Bild: sichtbare Höhe, Inhaltshöhe (Layout-Test)
    right_view: (f32, f32),
    /// „In Teile aufteilen“: an/aus, Art (0 Anzahl, 1 Länge, 2 Marker), Zahl, Sekunden je Teil
    split_on: bool,
    split_by: usize,
    split_count: u32,
    split_secs: u32,
    /// „Dateigröße: Eigene …“ gewählt; Wert in MB
    size_custom: bool,
    custom_mb: u32,
}

/// Höhe des Export-Fensters ohne die Einstellungen rechts (Titel, Meldungen, Fortschritt,
/// Knopfzeile, Ränder): bis zur Programmfensterhöhe minus diesem Wert zeigt die rechte Spalte alles,
/// darüber scrollt sie
const DIALOG_CHROME_H: f32 = 250.0;

/// Name eines Encoders (Export-Dialog, Einstellungen → Leistung)
pub fn encoder_name(e: Encoder) -> &'static str {
    match e {
        Encoder::Software => "Software (beste Qualität)",
        Encoder::Nvenc => "NVIDIA NVENC (schnell)",
        Encoder::Qsv => "Intel Quick Sync (schnell)",
        Encoder::Amf => "AMD AMF (schnell)",
        Encoder::Mf => "Windows (Media Foundation)",
    }
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
        let pending = self.running.iter().count() + self.queue.len();
        if ctx.input(|i| i.viewport().close_requested()) && !close_allowed(pending > 0, self.quit_ok) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit_ask = true;
        }
        if !self.quit_ask {
            return;
        }
        let (mut quit, mut stay) = (false, false);
        let m = crate::dialog::modal(ctx, "export-quit").show(ctx, |ui| {
            ui.set_width(340.0);
            let t = theme::count_text(pending, "Export läuft – wirklich beenden?", "{} Exporte ausstehend – wirklich beenden?");
            stay |= crate::dialog::title(ui, &t);
            let (q, c) = crate::dialog::buttons(ui, theme::danger("Cutline beenden"), true);
            quit = q;
            stay |= c;
        });
        if quit {
            self.quit_ok = true;
            self.quit_ask = false;
            self.queue.clear();
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
        self.queue.clear();
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
        // auch während eines Exports neu einrichten: das offene Projekt lässt sich dann einreihen
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
            s.include_audio = old.include_audio;
            s.target_mb = old.target_mb;
        }
        // eine im Dialog getroffene Wahl gilt nur für diesen Export
        self.encoder_picked = false;
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
        if self.split_count == 0 {
            (self.split_count, self.split_secs, self.custom_mb) = (2, 60, 200);
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
        self.split(s).is_none()
            && self
                .exported
                .as_ref()
                .is_some_and(|(r, f, used)| *r == self.rev && f.is_file() && *f == self.output_path(s.format) && used == s)
    }

    fn output_path(&self, f: Format) -> PathBuf {
        self.folder.join(format!("{}.{}", file_safe(&self.name), f.extension()))
    }

    /// Gewählte Aufteilung (nur mit Bild)
    fn split(&self, s: &ExportSettings) -> Option<Split> {
        (self.split_on && !s.format.audio_only()).then_some(match self.split_by {
            0 => Split::Count(self.split_count),
            1 => Split::Length(self.split_secs),
            _ => Split::Markers,
        })
    }

    /// Ziel: Datei bzw. Ordner mit dem Exportnamen für die Teile; dazu die Teile (Bereich, Datei)
    fn targets(&self, p: &Project, s: &ExportSettings) -> (PathBuf, Vec<(Option<(Time, Time)>, PathBuf)>) {
        let Some(how) = self.split(s) else {
            let out = self.output_path(s.format);
            return (out.clone(), vec![(s.range, out)]);
        };
        let name = file_safe(&self.name);
        let dir = self.folder.join(&name);
        let range = s.range.unwrap_or((Time::ZERO, p.duration()));
        let parts = split_parts(how, range, p.settings.fps, &p.timeline.markers, &name, &crate::tr("Teil"), s.format.extension());
        let parts = parts.into_iter().map(|(r, f)| (Some(r), dir.join(f))).collect();
        (dir, parts)
    }

    /// Längster Teil (Sekunden) und Zahl der Teile (1 ohne Aufteilen); die Zielgröße gilt je Teil
    fn longest_part(&self, p: &Project, s: &ExportSettings) -> (f64, usize) {
        let (_, parts) = self.targets(p, s);
        let len = |r: &Option<(Time, Time)>| r.map_or(p.duration(), |(a, b)| b - a).as_secs_f64();
        (parts.iter().map(|x| len(&x.0)).fold(0.0, f64::max), parts.len())
    }

    /// „Dateigröße“: Automatisch, feste Zielgrößen oder eigene; ausgegraut ohne passenden Codec.
    /// Darunter der Hinweis, wenn dafür verkleinert wird.
    fn size_rows(&mut self, ui: &mut egui::Ui, p: &Project, s: &mut ExportSettings) {
        let can = s.target_possible();
        if !can {
            s.target_mb = None;
        }
        ui.add_enabled_ui(can, |ui| {
            theme::row(ui, "Dateigröße", |ui| {
                let label = match s.target_mb {
                    None => "Automatisch".to_string(),
                    Some(_) if self.size_custom => "Eigene …".into(),
                    Some(m) => format!("Höchstens {m} MB"),
                };
                egui::ComboBox::from_id_salt("exp-size").selected_text(crate::tr(label)).width(200.0).show_ui(ui, |ui| {
                    if ui.selectable_label(s.target_mb.is_none(), crate::tr("Automatisch")).clicked() {
                        (s.target_mb, self.size_custom) = (None, false);
                    }
                    for m in [10, 25, 50, 100] {
                        if ui.selectable_label(s.target_mb == Some(m) && !self.size_custom, crate::tr(format!("Höchstens {m} MB"))).clicked() {
                            (s.target_mb, self.size_custom) = (Some(m), false);
                        }
                    }
                    if ui.selectable_label(s.target_mb.is_some() && self.size_custom, crate::tr("Eigene …")).clicked() {
                        (s.target_mb, self.size_custom) = (Some(self.custom_mb.max(1)), true);
                    }
                });
            });
        });
        let Some(_) = s.target_mb else { return };
        if self.size_custom {
            theme::row(ui, "", |ui| ui.add(egui::DragValue::new(&mut self.custom_mb).range(1..=100_000).suffix(" MB")));
            s.target_mb = Some(self.custom_mb.max(1));
        }
        // nie still verkleinern (bei Teilen gilt der längste)
        let t = s.for_target(self.longest_part(p, s).0, audible(p, s));
        if let Quality::Bitrate(k) = t.quality {
            if let Some((e, enough)) = cl_export::target_edge(k, (s.width, s.height), s.fps.as_f64()) {
                let h = if enough { format!("Wird in {e}p exportiert") } else { format!("Wird in {e}p exportiert – auch dafür ist die Größe knapp") };
                ui.label(RichText::new(crate::tr(h)).small().color(theme::WARN_TEXT()));
            }
        }
    }

    /// „In Teile aufteilen“ mit Art, Zahl bzw. Länge und Ergebniszeile (Abschnitt „Video“)
    fn split_rows(&mut self, ui: &mut egui::Ui, p: &Project, s: &ExportSettings) {
        theme::row(ui, "", |ui| crate::widgets::check(ui, &mut self.split_on, crate::tr("In Teile aufteilen")));
        let Some(how) = self.split(s) else { return };
        let range = s.range.unwrap_or((Time::ZERO, p.duration()));
        let parts = |how| split_parts(how, range, p.settings.fps, &p.timeline.markers, "", "", "");
        // „Marker“ nur mit mindestens einem Marker im Bereich (sonst bliebe ein Teil)
        let marks = parts(Split::Markers).len() > 1;
        if !marks && how == Split::Markers {
            self.split_by = 0;
        }
        theme::row(ui, "Aufteilen nach", |ui| {
            ui.allocate_ui(vec2(200.0, 24.0), |ui| crate::widgets::segmented_some(ui, &["Anzahl", "Länge", "Marker"], &mut self.split_by, &[true, true, marks]));
        });
        theme::row(ui, "", |ui| {
            match self.split_by {
                0 => drop(ui.add(egui::DragValue::new(&mut self.split_count).range(2..=SPLIT_MAX))),
                1 => drop(ui.add(egui::DragValue::new(&mut self.split_secs).range(1..=36_000).suffix(" s"))),
                _ => {}
            }
            if let Some(how) = self.split(s) {
                ui.label(RichText::new(split_summary(&parts(how))).small().color(theme::TEXT_DIM()));
            }
        });
    }

    /// Ziel schon vergeben: eine der Dateien liegt schon da, oder ein laufender oder wartender
    /// Export schreibt dorthin (`out` = Datei bzw. Ordner der Teile)
    fn taken(&self, out: &Path, files: &[(Option<(Time, Time)>, PathBuf)]) -> bool {
        files.iter().any(|f| f.1.exists()) || self.running.as_ref().is_some_and(|r| r.src.out == out) || self.queue.iter().any(|q| q.out == out)
    }

    /// Laufender Export für die Exporte-Liste: Name (bei Teilen „Name · Teil 3 von 6“) und Fortschritt
    pub fn current(&self) -> Option<(String, f32)> {
        self.running.as_ref().map(|r| {
            let mut name = r.src.out.file_name().unwrap_or_default().to_string_lossy().to_string();
            if let Some((i, n)) = r.part_of() {
                name = format!("{name} · {}", crate::tr(format!("Teil {i} von {n}")));
            }
            (name, *r.progress.lock().unwrap())
        })
    }

    /// Wartende Exporte: Zieldatei bzw. Ordner, Einstellungen, Zahl der Teile (0 = ohne Teile)
    pub fn waiting(&self) -> impl Iterator<Item = (&Path, &ExportSettings, usize)> {
        self.queue.iter().map(|q| (q.out.as_path(), &q.settings, if q.group() { q.parts.len() } else { 0 }))
    }

    pub fn cancel_current(&self) {
        if let Some(r) = &self.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Wartenden Export entfernen (`None`: alle)
    pub fn remove_waiting(&mut self, i: Option<usize>) {
        match i {
            Some(i) => drop(self.queue.remove(i)),
            None => self.queue.clear(),
        }
    }

    /// Vordersten wartenden Export starten, wenn keiner läuft
    fn start_next(&mut self, ctx: &egui::Context) {
        if self.running.is_none() {
            if let Some(q) = self.queue.pop_front() {
                self.last_output = Some(q.out.clone());
                self.running = Some(Running::start(ctx, q));
            }
        }
    }

    /// Laufenden Export weiterverfolgen (jedes Bild, auch auf der Startseite und bei geschlossenem
    /// Dialog) und danach den nächsten wartenden starten. Gibt eine Meldung für einen Toast zurück,
    /// wenn ein Export fertig ist, bei Erfolg mit dem Projekt-Abbild, aus dem er stammt.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<(String, Option<Arc<Project>>)> {
        let mut toast = None;
        let finished = self.running.as_ref().map(|r| {
            let cancelled = r.cancel.load(Ordering::Relaxed);
            let mut res = r.result.lock().unwrap().take();
            // Abbruch während der Vorbereitung: nicht auf den Bau warten (er prüft je Datei den Ton
            // und kennt kein Abbruch-Kennzeichen); der Faden endet danach ohne FFmpeg-Lauf
            if res.is_none() && cancelled && r.job.lock().unwrap().is_none() {
                res = Some(Err(cl_export::ExportError::Cancelled.to_string()));
            }
            (res, r.started.elapsed().as_secs_f32(), cancelled)
        });
        match finished {
            Some((Some(res), secs, cancelled)) => {
                let run = self.running.take().expect("läuft");
                let done = run.finished.lock().unwrap().clone();
                let src = run.src;
                let s = &src.settings;
                let res = res.map_err(|e| (!cancelled).then_some(e));
                log::info!(target: "export", "{} nach {secs:.1} s: {} · {} von {} Teilen · {:?}", match res { Ok(()) => "fertig", Err(None) => "abgebrochen", Err(Some(_)) => "Fehler" }, src.out.display(), done.len(), src.parts.len(), (s.width, s.height, s.fps, s.format, s.codec, s.quality, s.encoder, s.include_audio, s.target_mb));
                // fertige Teile (auch wenn andere scheiterten oder danach abgebrochen wurde):
                // Begleitdateien schreiben, dem Projekt zugeordnet, aus dem der Export stammt (nicht
                // dem gerade offenen), unter dem Projektnamen, nicht dem (änderbaren) Dateinamen
                let mut failed = Vec::new();
                for x in done.iter().filter_map(|&i| src.parts.get(i)) {
                    failed.extend(x.sidecars.iter().filter_map(|(f, txt)| std::fs::write(f, txt).err().map(|e| format!("{} ({e})", f.display()))));
                    crate::exports::record(&src.project.name, src.project_path.as_deref(), &x.out);
                }
                let group = src.group().then_some(src.parts.len());
                match res {
                    Ok(()) => {
                        self.done = Some((Instant::now(), secs));
                        // „Zur Datei“ nur im Dialog des Projekts, aus dem der Export stammt; nach
                        // Teilen bleibt „Im Explorer zeigen“ (Ordner)
                        if src.project_path == self.project_path && group.is_none() {
                            self.exported = Some((src.rev, src.out.clone(), src.settings.clone()));
                        }
                        // Toasts übersetzt die App
                        let what = group.map_or_else(|| "Export fertig".to_string(), |n| format!("{n} Teile fertig"));
                        let mut msg = format!("{} {what} ({})", icon::CHECK_CIRCLE, theme::secs(secs as f64, 1));
                        if !failed.is_empty() {
                            let e = format!("{}: {}", crate::tr("Begleitdatei nicht gespeichert"), failed.join(", "));
                            msg = format!("{} {e}", icon::WARNING_CIRCLE);
                            self.error = Some(e);
                        }
                        toast = Some((msg, Some(src.project)));
                    }
                    Err(Some(e)) => {
                        // Gruppe: wie viele Teile fertig wurden, dazu der erste Fehler
                        let e = match group {
                            Some(n) => format!("{}: {e}", crate::tr(format!("{} von {n} Teilen fertig", done.len()))),
                            None => e,
                        };
                        // bei geschlossenem Dialog sähe man den Fehler sonst nicht (die Schlange läuft weiter)
                        if !self.open {
                            let f = src.out.file_name().unwrap_or_default().to_string_lossy().to_string();
                            toast = Some((format!("{} {f}: {e}", icon::WARNING_CIRCLE), None));
                        }
                        self.error = Some(e);
                    }
                    Err(None) => {}
                }
            }
            Some((None, ..)) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            None => {}
        }
        // Fehlschlag oder Abbruch halten die Schlange nicht an
        self.start_next(ctx);
        toast
    }

    /// Zeigt den Dialog. Gibt eine Meldung für einen Toast zurück („Kapitel kopiert“).
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
        if !self.open {
            return toast;
        }
        // Esc schließt den Dialog (nicht während eines laufenden Exports), Enter löst die
        // Hauptaktion aus (reiht ein, wenn schon einer läuft); beides nicht während der Nachfrage
        let (esc, enter) = crate::dialog::keys(ctx);
        if self.running.is_none() && !self.confirm_overwrite && esc {
            self.open = false;
            return toast;
        }
        let mut main = !self.confirm_overwrite && enter;
        let Some(mut s) = self.settings.clone() else {
            return toast;
        };
        s.chapters = self.chapters;
        s.chapter_name = crate::tr("Kapitel");
        // Vorauswahl nach „Hardware-Kodierung“, bis im Dialog gewählt wird (Erkennung im Hintergrund)
        if !self.encoder_picked {
            let found = self.encoders.lock().unwrap().clone().unwrap_or_default();
            s.encoder = cl_export::export_default(&found, &crate::proxy::encoder_pref());
        }
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
                                let pv = r.job.lock().unwrap().as_ref().and_then(|j| j.1.clone());
                                if let Some(pv) = pv {
                                    r.live.poll(ui.ctx(), &pv);
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
                        // Zielgröße gilt je Teil: Größe des Bildes wie beim längsten Teil
                        let (longest, n) = self.longest_part(p, &s);
                        let info = if audio_only {
                            format!("{} {}", icon::CLOCK, theme::secs(secs, 1))
                        } else {
                            let (w, h) = s.for_target(longest, audible(p, &s)).output_size();
                            format!("{} {}  ·  {w}×{h}", icon::CLOCK, theme::secs(secs, 1))
                        };
                        ui.label(RichText::new(crate::tr(info)).small().color(theme::TEXT_DIM()));
                        if !audio_only {
                            let (est, exact) = estimate_mb(&s, p.duration());
                            let txt = match s.target_mb.filter(|_| s.target_possible()) {
                                Some(m) if n > 1 => format!("{} {n} × {m} MB", icon::HARD_DRIVES),
                                Some(m) => format!("{} {m} MB", icon::HARD_DRIVES),
                                None if exact => format!("{} {est:.0} MB", icon::HARD_DRIVES),
                                None => format!("{} ca. {est:.0} MB", icon::HARD_DRIVES),
                            };
                            ui.label(RichText::new(crate::tr(txt)).small().color(theme::TEXT_DIM()));
                        }
                        if live {
                            ui.label(RichText::new(crate::tr("Live-Vorschau")).size(theme::FS_HINT).color(theme::TEXT_DIM()));
                        }
                    });
                    ui.add_space(12.0);
                    // Rechts: Einstellungen
                    ui.vertical(|ui| {
                        // so hoch wie der Inhalt (größte bisher gezeigte Fassung: springt nicht beim
                        // Umschalten), höchstens bis das Fenster ins Programmfenster passt; dann scrollt es.
                        // Ohne Mindesthöhe nähme der Bereich nur die Höhe der linken Spalte.
                        let max_h = (ctx.screen_rect().height() - DIALOG_CHROME_H).max(160.0);
                        let sc = egui::ScrollArea::vertical()
                            .id_salt("export-settings")
                            .max_height(max_h)
                            .min_scrolled_height(self.right_h.min(max_h))
                            .show(ui, |ui| {
                        // Formatwechsel blendet Zeilen aus: Höhe halten, damit Knöpfe nicht springen
                        ui.set_min_height(self.right_h);
                        // auch während eines Exports einstellbar: der nächste wird eingereiht
                        ui.scope(|ui| {
                            theme::row(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(300.0)));
                            theme::row(ui, "Speichern in", |ui| {
                                // Platz für den Ordner-Knopf lassen, sonst ragt die Zeile über das Fenster hinaus
                                let w = (ui.available_width() - 32.0).max(40.0);
                                ui.allocate_ui(vec2(w, 20.0), |ui| {
                                    ui.add(egui::Label::new(RichText::new(crate::paths::shown(&self.folder)).small()).truncate());
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
                                self.size_rows(ui, p, &mut s);
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
                                    // mit Zielgröße bestimmt sie die Bitrate
                                    ui.add_enabled_ui(s.target_mb.is_none(), |ui| theme::row(ui, "Qualität", |ui| {
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
                                    }));
                                    if let Quality::Bitrate(_) = s.quality {
                                        theme::row(ui, "", |ui| {
                                            ui.add(egui::DragValue::new(&mut self.custom_kbps).range(500..=200_000).suffix(" kbit/s").speed(100));
                                        });
                                        s.quality = Quality::Bitrate(self.custom_kbps);
                                    }
                                    theme::row(ui, "Bildrate", |ui| {
                                        egui::ComboBox::from_id_salt("exp-fps").selected_text(crate::tr(format!("{} fps", theme::dec(s.fps.as_f64(), 3)))).width(200.0).show_ui(ui, |ui| {
                                            for f in ["23.976", "24", "25", "29.97", "30", "50", "59.94", "60"] {
                                                let r = cl_core::Rational::parse(f).unwrap();
                                                ui.selectable_value(&mut s.fps, r, crate::tr(format!("{f} fps")));
                                            }
                                        });
                                    });
                                    let encs = self.encoders.lock().unwrap().clone().unwrap_or_default();
                                    // mit Zielgröße immer Software (zwei Durchgänge); die Wahl bleibt für danach
                                    let target = s.target_mb.is_some();
                                    ui.add_enabled_ui(!target, |ui| theme::row(ui, "Encoder", |ui| {
                                        let name = encoder_name;
                                        let shown = if target { Encoder::Software } else { s.encoder };
                                        egui::ComboBox::from_id_salt("exp-enc").selected_text(crate::tr(name(shown))).width(200.0).show_ui(ui, |ui| {
                                            for e in encs {
                                                if ui.selectable_value(&mut s.encoder, e, crate::tr(name(e))).clicked() {
                                                    self.encoder_picked = true;
                                                }
                                            }
                                        });
                                    }));
                                    self.split_rows(ui, p, &s);
                                    theme::section(ui, "Ton");
                                    crate::widgets::check(ui, &mut s.include_audio, crate::tr(format!("Ton exportieren (AAC {} kbit/s, bei WebM Opus)", s.audio_kbps.unwrap_or(256))));
                                    ui.add_enabled_ui(s.include_audio, |ui| loudness_row(ui, &mut s.loudness));
                                } else {
                                    self.split_rows(ui, p, &s);
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
                        self.right_view = (sc.inner_rect.height(), sc.content_size.y);
                        self.right_h = self.right_h.max(sc.content_size.y);
                    });
                });

                ui.add_space(12.0);
                if !self.missing.is_empty() {
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
                    let measures = r.job.lock().unwrap().as_ref().map(|j| j.0);
                    match measures {
                        Some(m) => {
                            // Teile: Fortschritt über alle, der Messlauf je Teil zeigt sich nicht eigens
                            let part = r.part_of();
                            export_progress(ui, prog, r.started.elapsed().as_secs_f32(), m && prog < cl_export::MEASURE_SHARE && part.is_none(), part, self.queue.len())
                        }
                        None => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(crate::tr("Wird vorbereitet …")).color(theme::TEXT()));
                                waiting_count(ui, self.queue.len());
                            });
                            ui.add_space(4.0);
                            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
                            crate::widgets::busy_line(ui.painter(), rect);
                            ui.ctx().request_repaint();
                        }
                    }
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
                        let full = self.queue.len() >= QUEUE_MAX;
                        if self.running.is_none() && self.unchanged_since_export(&s) {
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
                        } else {
                            // läuft schon einer, stellt sich auch „Exportieren“ hinten an
                            let tip = crate::tr(format!("Höchstens {QUEUE_MAX} wartende Exporte"));
                            if ui.add_enabled(!full, theme::primary(format!("{}  Exportieren", icon::EXPORT))).on_disabled_hover_text(&tip).clicked() || (!full && std::mem::take(&mut main)) {
                                start = true;
                            }
                            if ui.add_enabled(!full, theme::secondary(format!("{}  In Warteschlange", icon::LIST_PLUS))).on_disabled_hover_text(&tip).clicked() {
                                // wie „Exportieren“: startet sofort, wenn keiner läuft; Fenster bleibt offen
                                start = true;
                            }
                        }
                        if self.running.is_some() {
                            if ui.add(theme::secondary(format!("{}  Abbrechen", icon::X))).clicked() {
                                self.cancel_current();
                            }
                        } else {
                            let close_label = if self.done.is_some() || self.unchanged_since_export(&s) { "Schließen" } else { "Abbrechen" };
                            if ui.add(theme::secondary(close_label)).clicked() {
                                open = false;
                            }
                        }
                    });
                });
            });
        self.settings = Some(s.clone());
        if !open {
            self.open = false;
        }
        let (out, parts) = self.targets(p, &s);
        // bei Teilen: eine Nachfrage für die ganze Gruppe
        if start && !overwrite && self.taken(&out, &parts) {
            self.confirm_overwrite = true;
            self.confirm_fresh = true;
            start = false;
        }
        if self.confirm_overwrite {
            // 0 = überschreiben, 1 = anderer Name, 2 = abbrechen
            let mut choice = None;
            let file = out.clone();
            let exists = parts.iter().any(|f| f.1.exists());
            // Teile eines früheren Laufs, die es diesmal nicht mehr gibt (z. B. „Teil 06“ von 6, jetzt 4)
            let keep: Vec<PathBuf> = parts.iter().map(|f| f.1.clone()).collect();
            let stale = if self.split(&s).is_some() { stale_parts(&out, &file_safe(&self.name), s.format.extension(), &keep) } else { Vec::new() };
            let old = stale.iter().filter(|f| f.extension().is_some_and(|e| e == s.format.extension())).count();
            let m = crate::dialog::modal(ctx, "export-overwrite").show(ctx, |ui| {
                ui.set_width(380.0);
                let t = match (exists, parts.len() > 1) {
                    (false, _) => "Name schon in der Warteschlange",
                    (true, false) => "Datei existiert bereits",
                    (true, true) => "Dateien existieren bereits",
                };
                if crate::dialog::title(ui, t) {
                    choice = Some(2);
                }
                ui.label(RichText::new(crate::paths::shown(&file)).small().color(theme::TEXT_DIM()));
                if old > 0 {
                    ui.label(RichText::new(crate::tr(format!("Beim Überschreiben werden {old} ältere Teile entfernt"))).small().color(theme::TEXT_DIM()));
                }
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
                Some(0) => {
                    for f in &stale {
                        let _ = std::fs::remove_file(f);
                    }
                    start = true;
                }
                Some(1) => {
                    let base = self.name.trim().to_string();
                    for n in 2.. {
                        self.name = format!("{base} ({n})");
                        let (o, f) = self.targets(p, &s);
                        if !self.taken(&o, &f) {
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
        if start && self.queue.len() < QUEUE_MAX {
            self.error = None;
            self.done = None;
            // nach „Anderen Namen“ neu
            let (out, parts) = self.targets(p, &s);
            let parts = parts
                .into_iter()
                .map(|(range, f)| Part { sidecars: sidecars(p, &ExportSettings { range, ..s.clone() }, self.also_srt, &f), range, out: f })
                .collect();
            self.queue.push_back(Queued {
                project: Arc::new(p.clone()),
                project_path: self.project_path.clone(),
                settings: s.clone(),
                parts,
                out,
                fonts: fonts.clone(),
                rev: self.rev,
            });
            self.start_next(ctx);
        }
        toast
    }
}

/// Hörbarer Ton im Exportbereich? (Zielgröße: sonst wird für Ton nichts eingeplant)
fn audible(p: &Project, s: &ExportSettings) -> bool {
    let (a, b) = s.range.unwrap_or((Time::ZERO, p.duration()));
    !cl_export::audio_clips_in(p, a, b).is_empty()
}

/// Name für Dateien und Ordner: Zeichen, die Windows verbietet, werden zu `_`; Länge und
/// Gerätenamen wie bei Projektdateien ([`crate::paths::fit_name`]).
fn file_safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    crate::paths::fit_name(&s)
}

/// Dateien einer früheren Teile-Gruppe in `dir`, die nicht zu den neuen Teilen `keep` gehören. Nur
/// genau das Muster dieser Gruppe: `Name – Teil NN`, `Name – Part NN` oder `Name – NN[ Marker]`
/// mit der Endung des Formats oder als Begleitdatei (.srt, .vtt, .chapters.txt); sonst nichts.
fn stale_parts(dir: &Path, name: &str, ext: &str, keep: &[PathBuf]) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let ends = [format!(".{ext}"), ".srt".into(), ".vtt".into(), ".chapters.txt".into()];
    let prefix = format!("{name} – ");
    let nr = |t: &str| t.len() >= 2 && t.bytes().all(|b| b.is_ascii_digit());
    let ours = |n: &str| {
        let Some(rest) = ends.iter().find_map(|e| n.strip_suffix(e.as_str())).and_then(|x| x.strip_prefix(&prefix)) else {
            return false;
        };
        match rest.split_once(' ') {
            Some(("Teil" | "Part", t)) => nr(t),
            Some((h, t)) => nr(h) && !t.trim().is_empty(),
            None => nr(rest),
        }
    };
    let kept = |f: &Path| keep.iter().any(|k| ["srt", "vtt", "chapters.txt"].iter().any(|e| k.with_extension(e) == f) || k == f);
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|f| f.is_file() && f.file_name().is_some_and(|n| ours(&n.to_string_lossy())) && !kept(f))
        .collect();
    v.sort();
    v
}

/// „In Teile aufteilen“: gleich lange Teile, Teile fester Länge (Sekunden) oder Schnitt an Markern
#[derive(Debug, Clone, Copy, PartialEq)]
enum Split {
    Count(u32),
    Length(u32),
    Markers,
}

/// Höchstzahl der Teile bei „Anzahl“
const SPLIT_MAX: u32 = 50;
/// Ein kürzerer Rest bei „Länge“ wird dem vorletzten Teil zugeschlagen
const SPLIT_MIN_REST: Time = Time(3_000_000);

/// Teile eines Exports von `a` bis `b`: Bereich und Dateiname je Teil, lückenlos und ohne
/// Überlappung; Schnitte liegen auf Bildgrenzen des Projekts (`fps`). `name` ist schon
/// dateitauglich, `part` das Wort „Teil“ in der Sprache der Oberfläche.
fn split_parts(how: Split, (a, b): (Time, Time), fps: cl_core::Rational, markers: &[cl_core::Marker], name: &str, part: &str, ext: &str) -> Vec<((Time, Time), String)> {
    let snap = |t: Time| t.snap_to_frame(fps);
    // Schnittstellen mit Markername (leer ohne Marker)
    let mut cuts: Vec<(Time, &str)> = match how {
        Split::Count(n) => {
            let n = n.clamp(2, SPLIT_MAX) as i64;
            (1..n).map(|k| (snap(a + Time((b - a).0 * k / n)), "")).collect()
        }
        Split::Length(secs) => {
            let step = Time(secs.max(1) as i64 * 1_000_000);
            let mut v: Vec<(Time, &str)> = (1..).map(|k| a + Time(step.0 * k)).take_while(|t| *t < b).map(|t| (snap(t), "")).collect();
            if v.last().is_some_and(|(t, _)| b - *t < SPLIT_MIN_REST) {
                v.pop();
            }
            v
        }
        Split::Markers => markers.iter().map(|m| (snap(m.at), m.name.as_str())).collect(),
    };
    cuts.sort_by_key(|c| c.0);
    // Marker genau am Anfang benennt den ersten Teil
    let first = cuts.iter().find(|c| c.0 == snap(a) || c.0 == a).map_or("", |c| c.1);
    // Rand nicht auf dem Bildraster: kein Splitterteil zwischen Rand und gerundetem Schnitt
    cuts.retain(|c| c.0 > a.max(snap(a)) && c.0 < b.min(snap(b)));
    cuts.dedup_by_key(|c| c.0);
    let starts: Vec<(Time, &str)> = std::iter::once((a, first)).chain(cuts).collect();
    let n = starts.len();
    let digits = n.to_string().len().max(2);
    starts
        .iter()
        .enumerate()
        .map(|(i, &(t, label))| {
            let end = starts.get(i + 1).map_or(b, |s| s.0);
            let nr = format!("{:0digits$}", i + 1);
            let file = match (how, file_safe(label)) {
                (Split::Markers, l) if l.is_empty() => format!("{name} – {nr}.{ext}"),
                (Split::Markers, l) => format!("{name} – {nr} {l}.{ext}"),
                _ => format!("{name} – {part} {nr}.{ext}"),
            };
            ((t, end), file)
        })
        .collect()
}

/// Ergebniszeile unter „Aufteilen nach“: „6 Teile · je 1:00“ bzw. „4 Teile · 0:42 bis 2:10“
fn split_summary(parts: &[((Time, Time), String)]) -> String {
    let durs = parts.iter().map(|((a, b), _)| *b - *a);
    // auf ganze Sekunden gerundet: 59,97 s (Bildraster) zeigt 1:00 wie die übrigen Teile
    let t = |d: Option<Time>| crate::navigate::chapter_time(Time((d.unwrap_or_default().0 + 500_000) / 1_000_000 * 1_000_000));
    let (lo, hi) = (t(durs.clone().min()), t(durs.max()));
    if lo == hi {
        crate::tr(format!("{} Teile · je {lo}", parts.len()))
    } else {
        crate::tr(format!("{} Teile · {lo} bis {hi}", parts.len()))
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
fn export_progress(ui: &mut egui::Ui, prog: f32, elapsed: f32, measuring: bool, part: Option<(usize, usize)>, waiting: usize) {
    ui.horizontal(|ui| {
        let what = match part {
            _ if measuring => "Lautheit messen …".to_string(),
            Some((i, n)) => format!("Exportiere Teil {i} von {n} …"),
            None => "Exportiere …".into(),
        };
        ui.label(RichText::new(crate::tr(what)).color(theme::TEXT()));
        waiting_count(ui, waiting);
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

/// Zahl der wartenden Exporte hinter dem laufenden („+2“)
fn waiting_count(ui: &mut egui::Ui, n: usize) {
    if n > 0 {
        ui.label(RichText::new(format!("+{n}")).color(theme::TEXT_DIM()));
    }
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
        ui.label(RichText::new(crate::tr(format!("Fertig in {}", theme::secs(secs as f64, 1)))).strong());
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
    for (name, mut st) in styles {
        // Untertitel-Stile haben keinen Text; leere Zeilen werden nicht gezeichnet
        if st.text.trim().is_empty() {
            st.text = "Probe".into();
        }
        let mut p = Project::new("t");
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(ClipContent::Text(st.clone()), Time::ZERO, Time::from_secs_f64(1.0)));
        p.timeline.tracks.push(t);
        let job = cl_export::build(&p, &ExportSettings::from_project(&p), &none, &dir.path().join("o.mp4")).unwrap();
        let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
        let [r, gr, b, a] = st.color;
        assert!(g.contains(&format!("fontcolor=0x{r:02x}{gr:02x}{b:02x}@{:.3}", a as f32 / 255.0)), "{name}: Farbe");
        // Schnitt aus denselben Schaltern wie die Vorschau: fett = zweiter Abdruck, kursiv = Scherung
        // je Zeile ein Abdruck; Karaoke: zweite Fassung in der Hervorhebungsfarbe
        let zeilen = st.text.lines().filter(|l| !l.trim().is_empty()).count();
        let fassungen = if st.karaoke.is_some() { 2 } else { 1 };
        assert_eq!(g.matches("drawtext=").count(), zeilen * fassungen * if st.bold { 2 } else { 1 }, "{name}: fett");
        assert_eq!(g.contains("shear=shx=0.25"), st.italic, "{name}: kursiv");
        assert_eq!(g.contains(":borderw="), st.outline.is_some(), "{name}: Umriss");
        // Fläche als ein Kasten um alle Zeilen
        assert_eq!(g.contains("drawbox="), st.background.is_some(), "{name}: Hintergrund");
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
        d.poll(ctx);
        d.show(ctx, p, &HashMap::new(), None, 0);
    });
}

#[test]
fn cancel_while_preparing_ends_at_once_without_error() {
    let (ctx, p) = (egui::Context::default(), Project::new("t"));
    let mut d = ExportDialog::default();
    // Bau läuft noch (kein Auftrag, kein Ergebnis): Abbrechen wirkt im nächsten Bild
    d.running = Some(running(None));
    frame(&mut d, &ctx, &p, false);
    assert!(d.running.is_some());
    d.running.as_ref().unwrap().cancel.store(true, Ordering::Relaxed);
    frame(&mut d, &ctx, &p, false);
    assert!(d.running.is_none() && d.error.is_none());
    // Abbruch während FFmpeg läuft: der Faden meldet „Abgebrochen“, das ist kein Fehler
    let r = running(Some(Err("Abgebrochen".into())));
    r.cancel.store(true, Ordering::Relaxed);
    d.running = Some(r);
    frame(&mut d, &ctx, &p, false);
    assert!(d.running.is_none() && d.error.is_none());
    // echter Fehler bleibt einer
    d.running = Some(running(Some(Err("FFmpeg: kaputt".into()))));
    frame(&mut d, &ctx, &p, false);
    assert_eq!(d.error.as_deref(), Some("FFmpeg: kaputt"));
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
    assert!(!d.confirm_overwrite);
    finish(&mut d, &ctx, &p);
    assert!(d.error.is_some());
    assert!(d.name.starts_with("t ("), "{}", d.name);
}

/// Bilder zeichnen, bis der Export-Faden fertig ist
#[cfg(test)]
fn finish(d: &mut ExportDialog, ctx: &egui::Context, p: &Project) {
    let t0 = Instant::now();
    while d.running.is_some() && t0.elapsed().as_secs() < 30 {
        std::thread::yield_now();
        frame(d, ctx, p, false);
    }
}

#[test]
fn export_job_is_built_off_the_ui_thread() {
    // großer Entwurf: der Bau dauerte Sekunden im Oberflächen-Faden (Tonprüfung je Datei).
    // Hier scheitert der Bau (leeres Projekt) – das Ergebnis kommt trotzdem erst aus dem Faden.
    let (ctx, p) = (egui::Context::default(), Project::new("t"));
    let dir = tempfile::tempdir().unwrap();
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(&p, None, 0, None);
    d.folder = dir.path().to_path_buf();
    frame(&mut d, &ctx, &p, true);
    assert!(d.running.is_some() && d.error.is_none(), "Auftrag im Aufrufer gebaut");
    finish(&mut d, &ctx, &p);
    assert_eq!(d.error.as_deref(), Some("Das Projekt ist leer"));
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
fn queued(p: &Project, out: &Path) -> Queued {
    Queued {
        project: Arc::new(p.clone()),
        project_path: None,
        settings: ExportSettings::from_project(p),
        out: out.to_path_buf(),
        parts: vec![Part { range: None, out: out.to_path_buf(), sidecars: Vec::new() }],
        fonts: HashMap::new(),
        rev: 0,
    }
}

#[cfg(test)]
fn running(result: Option<Result<(), String>>) -> Running {
    Running {
        progress: Default::default(),
        part: Default::default(),
        finished: Default::default(),
        job: Default::default(),
        result: Arc::new(Mutex::new(result)),
        cancel: Default::default(),
        started: Instant::now(),
        src: queued(&Project::new("t"), Path::new("t.mp4")),
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
    // Begleitdateien erst nach erfolgreichem Export schreiben (Erfolg trägt in `exports.json` ein)
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("CUTLINE_HOME", dir.path());
    let (ctx, mut d) = (egui::Context::default(), ExportDialog::default());
    for ok in [false, true] {
        let r = running(Some(if ok { Ok(()) } else { Err("x".into()) }));
        let out = dir.path().join("a.mp4");
        let parts = vec![Part { range: s.range, out: out.clone(), sidecars: v.clone() }];
        // fertig gemeldet wird je Teil
        r.finished.lock().unwrap().extend(ok.then_some(0));
        d.running = Some(Running { src: Queued { parts, settings: s.clone(), out, ..r.src }, ..r });
        frame(&mut d, &ctx, &p, false);
        assert_eq!(v.iter().all(|(f, _)| f.is_file()), ok);
    }
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn queue_takes_turns_names_limit_and_keeps_its_snapshot() {
    let (ctx, dir, p) = (egui::Context::default(), tempfile::tempdir().unwrap(), Project::new("t"));
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(&p, None, 0, None);
    d.folder = dir.path().to_path_buf();
    // läuft schon einer: „Exportieren“ (Enter) stellt sich hinten an, statt abgewiesen zu werden
    d.running = Some(running(None));
    frame(&mut d, &ctx, &p, true);
    assert_eq!(d.queue.len(), 1);
    assert_eq!(d.queue[0].out, dir.path().join("t.mp4"));
    // gleicher Zielpfad wie ein eingereihter: Nachfrage wie bei einer vorhandenen Datei,
    // Enter wählt „Anderen Namen“
    frame(&mut d, &ctx, &p, true);
    assert!(d.confirm_overwrite && d.queue.len() == 1);
    frame(&mut d, &ctx, &p, true);
    assert!(!d.confirm_overwrite);
    assert_eq!(d.queue.iter().map(|q| q.out.clone()).collect::<Vec<_>>(), [dir.path().join("t.mp4"), dir.path().join("t (2).mp4")]);
    // offenes Projekt ändert sich: das Abbild bleibt
    let mut changed = p.clone();
    changed.name = "geändert".into();
    frame(&mut d, &ctx, &changed, false);
    assert!(d.queue.iter().all(|q| *q.project == p));
    // einzelnen wartenden entfernen
    d.queue.push_back(queued(&p, &dir.path().join("c.mp4")));
    d.remove_waiting(Some(1));
    assert_eq!(d.waiting().map(|(f, _, _)| f.to_path_buf()).collect::<Vec<_>>(), [dir.path().join("t.mp4"), dir.path().join("c.mp4")]);
    // Obergrenze: weder Enter noch Knopf reihen mehr ein
    while d.queue.len() < QUEUE_MAX {
        d.queue.push_back(queued(&p, &dir.path().join(format!("{}.mp4", d.queue.len()))));
    }
    d.name = "frei".into();
    frame(&mut d, &ctx, &p, true);
    assert!(d.queue.len() == QUEUE_MAX && !d.confirm_overwrite);
    // „Alle wartenden entfernen“: der laufende bleibt
    d.remove_waiting(None);
    assert!(d.queue.is_empty() && d.running.is_some());
    // Abbruch des laufenden startet den nächsten; Fehlschläge halten die Schlange nicht an
    // (leeres Projekt: der Bau scheitert sofort)
    d.queue.extend([queued(&p, &dir.path().join("x.mp4")), queued(&p, &dir.path().join("y.mp4"))]);
    d.cancel_current();
    frame(&mut d, &ctx, &p, false);
    assert_eq!((d.current().map(|c| c.0), d.queue.len()), (Some("x.mp4".to_string()), 1));
    finish(&mut d, &ctx, &p);
    assert!(d.running.is_none() && d.queue.is_empty());
    assert_eq!(d.error.as_deref(), Some("Das Projekt ist leer"));
    // Beenden mit wartenden Exporten: Rückfrage nennt die Zahl, Schließen räumt die Schlange
    d.running = Some(running(None));
    d.queue.push_back(queued(&p, &dir.path().join("z.mp4")));
    let mut raw = egui::RawInput::default();
    raw.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { events: vec![egui::ViewportEvent::Close], ..Default::default() });
    let _ = ctx.run(raw, |ctx| d.guard_close(ctx));
    assert!(d.quit_ask);
    d.running.as_ref().unwrap().result.lock().unwrap().replace(Err("Abgebrochen".into()));
    d.cancel_and_wait(std::time::Duration::from_secs(1));
    assert!(d.queue.is_empty());
}

#[test]
fn queued_exports_run_in_turn_from_their_own_snapshot() {
    use cl_core::{Clip, ClipContent, TextStyle, Track, TrackKind};
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let project = |name: &str, txt: &str, secs: f64| {
        let mut p = Project::new(name);
        (p.settings.width, p.settings.height) = (320, 180);
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(ClipContent::Text(TextStyle { text: txt.into(), ..Default::default() }), Time::ZERO, Time::from_secs_f64(secs)));
        p.timeline.tracks.push(t);
        p
    };
    let (ctx, mut d) = (egui::Context::default(), ExportDialog::default());
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    let (p, pa) = (project("A", "eins", 1.0), dir.path().join("A.cutline"));
    d.open_for(&p, Some(&pa), 0, None);
    d.folder = dir.path().to_path_buf();
    d.also_srt = true;
    // ein (nie fertiger) Export läuft: beide Fassungen warten
    d.running = Some(running(None));
    frame(&mut d, &ctx, &p, true);
    d.name = "A kurz".into();
    frame(&mut d, &ctx, &p, true);
    assert_eq!(d.queue.len(), 2);
    // anderes Projekt offen und länger, dann erst laufen die beiden
    let other = project("Anders", "zwei", 3.0);
    let po = dir.path().join("Anders.cutline");
    d.open_for(&other, Some(&po), 7, None);
    d.cancel_current();
    let t0 = Instant::now();
    while (d.running.is_some() || !d.queue.is_empty()) && t0.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(&mut d, &ctx, &other, false);
    }
    assert_eq!(d.error, None);
    for f in ["A.mp4", "A kurz.mp4"] {
        let m = cl_media::probe(&dir.path().join(f)).unwrap();
        // Länge und Größe aus dem Abbild (1 s, 320×180), nicht aus dem offenen Projekt (3 s)
        assert!((m.duration.as_secs_f64() - 1.0).abs() < 0.2 && m.width == 320, "{f}: {:?}", m.duration);
    }
    // Begleitdatei aus dem Abbild
    assert!(std::fs::read_to_string(dir.path().join("A kurz.srt")).unwrap().contains("eins"));
    // Exporte-Liste: dem Projekt zugeordnet, aus dem sie stammen
    assert_eq!(crate::exports::for_project("A", Some(&pa)).len(), 2);
    assert!(crate::exports::for_project("Anders", Some(&po)).is_empty());
    std::env::remove_var("CUTLINE_HOME");
}

/// Testprojekt 320×180, ein Text über `secs` Sekunden
#[cfg(test)]
fn text_project(name: &str, txt: &str, secs: f64) -> Project {
    use cl_core::{Clip, ClipContent, TextStyle, Track, TrackKind};
    let mut p = Project::new(name);
    (p.settings.width, p.settings.height) = (320, 180);
    let mut t = Track::new(TrackKind::Text);
    t.clips.push(Clip::new(ClipContent::Text(TextStyle { text: txt.into(), ..Default::default() }), Time::ZERO, Time::from_secs_f64(secs)));
    p.timeline.tracks.push(t);
    p
}

/// Dialog mit „In Teile aufteilen“ (Anzahl) für `p`, Ziel `dir`
#[cfg(test)]
fn split_dialog(p: &Project, dir: &Path, n: u32) -> ExportDialog {
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(p, Some(&dir.join("p.cutline")), 0, None);
    d.folder = dir.to_path_buf();
    (d.split_on, d.split_by, d.split_count) = (true, 0, n);
    d
}

/// Bilder zeichnen, bis Export und Schlange leer sind
#[cfg(test)]
fn drain(d: &mut ExportDialog, ctx: &egui::Context, p: &Project) {
    let t0 = Instant::now();
    while (d.running.is_some() || !d.queue.is_empty()) && t0.elapsed().as_secs() < 120 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(d, ctx, p, false);
    }
}

#[test]
fn split_export_writes_parts_into_a_folder_as_one_entry() {
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let p = text_project("Film", "eins", 4.0);
    let mut d = split_dialog(&p, dir.path(), 4);
    d.also_srt = true;
    // läuft schon einer: die Gruppe wartet als ein Eintrag
    d.running = Some(running(None));
    frame(&mut d, &ctx, &p, true);
    assert_eq!(d.queue.len(), 1);
    assert_eq!(d.waiting().map(|(f, _, n)| (f.to_path_buf(), n)).collect::<Vec<_>>(), [(dir.path().join("Film"), 4)]);
    d.cancel_current();
    frame(&mut d, &ctx, &p, false);
    assert_eq!(d.current().unwrap().0, "Film · Teil 1 von 4");
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    let folder = dir.path().join("Film");
    let mut names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".mp4")).collect();
    names.sort();
    assert_eq!(names, ["Film – Teil 01.mp4", "Film – Teil 02.mp4", "Film – Teil 03.mp4", "Film – Teil 04.mp4"]);
    for n in &names {
        let m = cl_media::probe(&folder.join(n)).unwrap();
        assert!((m.duration.as_secs_f64() - 1.0).abs() < 0.1, "{n}: {:?}", m.duration);
    }
    // Untertitel je Teil, Zeiten ab Teilanfang
    assert_eq!(std::fs::read_to_string(folder.join("Film – Teil 03.srt")).unwrap(), "1\n00:00:00,000 --> 00:00:01,000\neins\n");
    // Exporte-Liste: jeder Teil
    assert_eq!(crate::exports::for_project("Film", Some(&dir.path().join("p.cutline"))).len(), 4);
    // noch einmal: eine Nachfrage für die ganze Gruppe, „Anderen Namen“ (Enter) → neuer Ordner
    frame(&mut d, &ctx, &p, true);
    assert!(d.confirm_overwrite && d.queue.is_empty());
    frame(&mut d, &ctx, &p, true);
    assert!(!d.confirm_overwrite);
    assert_eq!(d.running.as_ref().map(|r| r.src.out.clone()), Some(dir.path().join("Film (2)")));
    drain(&mut d, &ctx, &p);
    assert!(dir.path().join("Film (2)").join("Film (2) – Teil 04.mp4").is_file());
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn failed_part_does_not_stop_the_others() {
    use cl_core::{Clip, ClipContent, MediaKind, Track, TrackKind};
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    // Teil 3 (2–3 s) braucht ein Video, das inzwischen fehlt
    let mut p = text_project("Film", "eins", 4.0);
    let v = dir.path().join("weg.mp4");
    assert!(cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=red:size=320x180:rate=30:duration=1"]).arg(&v).status().unwrap().success());
    let m = cl_media::probe(&v).unwrap();
    std::fs::remove_file(&v).unwrap();
    let mut t = Track::new(TrackKind::Video);
    t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::from_secs_f64(2.0), Time::from_secs_f64(1.0)));
    p.media.push(m);
    p.timeline.tracks.push(t);
    let mut d = split_dialog(&p, dir.path(), 4);
    frame(&mut d, &ctx, &p, true);
    drain(&mut d, &ctx, &p);
    let e = d.error.clone().unwrap_or_default();
    assert!(e.starts_with("3 von 4 Teilen fertig: ") && e.contains("weg.mp4"), "{e}");
    let folder = dir.path().join("Film");
    for (n, there) in [(1, true), (2, true), (3, false), (4, true)] {
        assert_eq!(folder.join(format!("Film – Teil 0{n}.mp4")).is_file(), there, "Teil {n}");
    }
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn cancel_stops_the_whole_group_without_half_files() {
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let p = text_project("Film", "eins", 8.0);
    let folder = dir.path().join("Film");
    // sofort abgebrochen (während der Vorbereitung): `poll` meldet gleich, der Faden räumt erst
    // danach auf. Ein sofort gestarteter neuer Export in denselben Ordner darf dieses Aufräumen
    // nicht treffen (früher: Ordner weg, alle Teile scheiterten)
    let mut d = split_dialog(&p, dir.path(), 8);
    frame(&mut d, &ctx, &p, true);
    d.cancel_current();
    drain(&mut d, &ctx, &p);
    assert!(d.error.is_none());
    // gleich neu: nach dem ersten Teil abgebrochen → fertige Teile bleiben, keine halbe Datei, Rest fehlt
    frame(&mut d, &ctx, &p, true);
    let t0 = Instant::now();
    while d.running.as_ref().is_some_and(|r| r.finished.lock().unwrap().is_empty()) && t0.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    d.cancel_current();
    drain(&mut d, &ctx, &p);
    assert!(d.error.is_none(), "{:?}", d.error);
    let names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert!(names.contains(&"Film – Teil 01.mp4".to_string()) && !names.contains(&"Film – Teil 08.mp4".to_string()), "{names:?}");
    assert!(names.iter().all(|n| !n.contains(".part")), "{names:?}");
    std::env::remove_var("CUTLINE_HOME");
}

/// Höhe des Export-Fensters ohne echtes Fenster: im Programmfenster `w`×`h` zwei Bilder zeichnen
/// (das erste kennt die Inhaltshöhe noch nicht). Gibt das Fensterrechteck und (sichtbare Höhe,
/// Inhaltshöhe) der Einstellungen rechts zurück.
#[cfg(test)]
fn dialog_layout(d: &mut ExportDialog, p: &Project, w: f32, h: f32) -> (Rect, (f32, f32)) {
    let ctx = egui::Context::default();
    // Öffnen-Animation aus
    ctx.style_mut(|s| s.animation_time = 0.0);
    for _ in 0..4 {
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(w, h))), ..Default::default() };
        let _ = ctx.run(raw, |ctx| {
            d.show(ctx, p, &HashMap::new(), None, 0);
        });
    }
    (ctx.memory(|m| m.area_rect(egui::Id::new("export-dialog"))).expect("Fenster gezeichnet"), d.right_view)
}

#[test]
fn export_dialog_shows_all_settings_when_there_is_room() {
    let mut p = text_project("Film", "eins", 600.0);
    (p.settings.width, p.settings.height) = (1920, 1080);
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(&p, None, 0, None);
    // volle Fassung: Teile mit Ergebniszeile, Zielgröße mit Hinweis („wird in 360p exportiert“)
    d.split_on = true;
    d.settings.as_mut().unwrap().target_mb = Some(10);
    // 1440×900: alles sichtbar, nichts scrollt
    let (win, (shown, content)) = dialog_layout(&mut d, &p, 1440.0, 900.0);
    assert!(content > 400.0, "{content}");
    assert!(shown >= content - 1.0, "rechte Spalte scrollt: {shown} von {content}");
    assert!(win.bottom() <= 900.0 && win.top() >= 0.0, "{win:?}");
    // 1280×720: scrollt, aber das Fenster (Titel und Knöpfe) bleibt im Programmfenster
    let (win, (shown, content)) = dialog_layout(&mut d, &p, 1280.0, 720.0);
    assert!(win.top() >= 0.0 && win.bottom() <= 720.0, "{win:?}");
    assert!(shown > 300.0 && shown <= content, "{shown} von {content}");
}

#[test]
fn overwriting_a_group_finds_only_its_own_older_parts() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let files = [
        "Film – Teil 01.mp4", "Film – Teil 04.mp4", "Film – Teil 05.mp4", "Film – Teil 06.mp4", "Film – Teil 06.srt",
        "Film – Teil 06.vtt", "Film – Teil 06.chapters.txt", "Film – Part 07.mp4", "Film – 08 Ende.mp4", "Film – 09.mp4",
        // fremd: andere Endung, anderer Name, Zusatz, einstellige Nummer, Notizen
        "Film – Teil 06.mov", "Film – Teil 06 Kopie.mp4", "Anderer – Teil 06.mp4", "Film – Teil 6.mp4", "Notizen.txt", "Film.mp4",
        "Film – 10 .mp4",
    ];
    for f in files {
        std::fs::write(d.join(f), b"x").unwrap();
    }
    std::fs::create_dir(d.join("Film – Teil 11.mp4")).unwrap();
    let keep: Vec<PathBuf> = (1..=4).map(|i| d.join(format!("Film – Teil 0{i}.mp4"))).collect();
    let stale: Vec<String> = stale_parts(d, "Film", "mp4", &keep).iter().map(|f| f.file_name().unwrap().to_string_lossy().to_string()).collect();
    assert_eq!(
        stale,
        ["Film – 08 Ende.mp4", "Film – 09.mp4", "Film – Part 07.mp4", "Film – Teil 05.mp4", "Film – Teil 06.chapters.txt", "Film – Teil 06.mp4", "Film – Teil 06.srt", "Film – Teil 06.vtt"]
    );
    // Begleitdatei eines bleibenden Teils bleibt
    std::fs::write(d.join("Film – Teil 01.srt"), b"x").unwrap();
    assert!(!stale_parts(d, "Film", "mp4", &keep).iter().any(|f| f.ends_with("Film – Teil 01.srt")));
    // ohne Ordner: nichts
    assert!(stale_parts(&d.join("fehlt"), "Film", "mp4", &keep).is_empty());
}

#[test]
fn cleanup_of_a_cancelled_group_spares_a_newer_export_in_the_same_folder() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("Film");
    std::fs::create_dir(&f).unwrap();
    // alter Export belegt, neuer belegt, der alte räumt danach auf: Ordner bleibt (noch leer)
    folder_use(&f, true);
    folder_use(&f, true);
    folder_use(&f, false);
    assert!(f.is_dir());
    // der letzte gibt frei: leerer Ordner weg; nicht leer: bleibt
    folder_use(&f, false);
    assert!(!f.exists());
    std::fs::create_dir(&f).unwrap();
    std::fs::write(f.join("x.mp4"), b"x").unwrap();
    folder_use(&f, true);
    folder_use(&f, false);
    assert!(f.join("x.mp4").is_file());
}

/// Zielgröße gilt je Teil: Bitrate aus der Länge des Teils.
#[test]
fn target_size_applies_per_part() {
    use cl_core::{Clip, ClipContent, MediaKind, Track, TrackKind};
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    // starkes Rauschen (sonst unterschreitet der Encoder jede Bitrate) und Ton (der eingeplante
    // Anteil bliebe sonst ungenutzt)
    let v = dir.path().join("rauschen.mp4");
    let st = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=480x270:rate=30:duration=4,noise=alls=40:allf=t"])
        .args(["-f", "lavfi", "-i", "sine=f=440:d=4"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "8", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&v)
        .status()
        .unwrap();
    assert!(st.success());
    let m = cl_media::probe(&v).unwrap();
    let mut p = Project::new("Klein");
    (p.settings.width, p.settings.height) = (480, 270);
    let mut t = Track::new(TrackKind::Video);
    t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(4.0)));
    p.media.push(m);
    p.timeline.tracks.push(t);
    let mut d = split_dialog(&p, dir.path(), 2);
    d.settings.as_mut().unwrap().target_mb = Some(1);
    frame(&mut d, &ctx, &p, true);
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    for n in ["Klein – Teil 01.mp4", "Klein – Teil 02.mp4"] {
        let size = std::fs::metadata(dir.path().join("Klein").join(n)).unwrap().len();
        eprintln!("Zielgröße 1 MB je Teil (2 s): {n} {size} Byte");
        assert!((850_000..=1_000_000).contains(&size), "{n}: {size}");
    }
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn split_parts_are_gapless_on_frames_with_names() {
    let fps = cl_core::Rational::new(30, 1);
    let s = Time::from_secs_f64;
    let parts = |how, a: f64, b: f64, ms: &[cl_core::Marker]| split_parts(how, (s(a), s(b)), fps, ms, "Film", "Teil", "mp4");
    // lückenlos, ohne Überlappung, Summe = Bereich, Schnitte auf Bildgrenzen
    let check = |v: &[((Time, Time), String)], a: f64, b: f64| {
        assert_eq!((v[0].0 .0, v.last().unwrap().0 .1), (s(a), s(b)));
        for w in v.windows(2) {
            assert_eq!(w[0].0 .1, w[1].0 .0);
            let t = w[0].0 .1;
            assert_eq!(t, t.snap_to_frame(fps), "{t:?}");
            assert!(w[0].0 .0 < w[0].0 .1);
        }
    };
    // Anzahl: 10 s (In/Out ab 1 s) in 3 Teile → 3,333 s auf Bilder gerundet
    let v = parts(Split::Count(3), 1.0, 11.0, &[]);
    check(&v, 1.0, 11.0);
    assert_eq!(v.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(), ["Film – Teil 01.mp4", "Film – Teil 02.mp4", "Film – Teil 03.mp4"]);
    assert_eq!(v[0].0 .1, Time(4_333_333));
    assert_eq!(split_summary(&v), "3 Teile · je 0:03");
    // höchstens 50, mindestens 2 Teile; Stellen wachsen mit der Zahl (mindestens zwei)
    assert_eq!(parts(Split::Count(80), 0.0, 100.0, &[]).len(), 50);
    assert_eq!(parts(Split::Count(1), 0.0, 100.0, &[]).len(), 2);
    // 1 s je Teil über 120 s: der Rest von 1 s geht an den vorletzten → 119 Teile, dreistellig
    let v = parts(Split::Length(1), 0.0, 120.0, &[]);
    assert_eq!((v.len(), v[0].1.as_str(), v[118].1.as_str()), (119, "Film – Teil 001.mp4", "Film – Teil 119.mp4"));
    // Länge: Rest ab 3 s bleibt eigener Teil, darunter geht er an den vorletzten
    let v = parts(Split::Length(60), 0.0, 363.0, &[]);
    check(&v, 0.0, 363.0);
    assert_eq!(v.iter().map(|p| (p.0 .1 - p.0 .0).as_secs_f64()).collect::<Vec<_>>(), [60.0, 60.0, 60.0, 60.0, 60.0, 60.0, 3.0]);
    assert_eq!(split_summary(&v), "7 Teile · 0:03 bis 1:00");
    let v = parts(Split::Length(60), 0.0, 362.9, &[]);
    assert_eq!((v.len(), (v[5].0 .1 - v[5].0 .0).as_secs_f64()), (6, 62.9));
    // kürzer als ein Teil: ein Teil
    assert_eq!(parts(Split::Length(60), 0.0, 30.0, &[]).len(), 1);
    // Marker: außerhalb und genau auf dem Ende zählen nicht, genau am Anfang benennt Teil 1;
    // Namen bereinigt, leer → nur Nummer, doppelte Namen bleiben wegen der Nummer eindeutig
    let mk = |at: f64, name: &str| cl_core::Marker { id: 0, at: s(at), duration: Time::ZERO, name: name.into(), note: String::new(), color: 0 };
    let ms = [mk(1.0, "Vorher"), mk(2.0, "Start"), mk(4.01, "a/b: c?"), mk(6.0, ""), mk(7.0, "Ende"), mk(7.0, "Ende"), mk(9.0, "Ende"), mk(10.0, "Rand"), mk(12.0, "Danach")];
    let v = parts(Split::Markers, 2.0, 10.0, &ms);
    check(&v, 2.0, 10.0);
    assert_eq!(
        v.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(),
        ["Film – 01 Start.mp4", "Film – 02 a_b_ c_.mp4", "Film – 03.mp4", "Film – 04 Ende.mp4", "Film – 05 Ende.mp4"]
    );
    // Marker auf 4,01 s rastet auf das Bild bei 4 s ein
    assert_eq!(v[1].0 .0, s(4.0));
    // Gerätenamen und Steuerzeichen
    let v = parts(Split::Markers, 0.0, 10.0, &[mk(5.0, "CON"), mk(6.0, "x\ty")]);
    assert_eq!(v.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(), ["Film – 01.mp4", "Film – 02 CON_.mp4", "Film – 03 x_y.mp4"]);
    // ohne Marker im Bereich: nur ein Teil (Auswahl ausgegraut)
    assert_eq!(parts(Split::Markers, 2.0, 10.0, &[mk(1.0, "x"), mk(10.0, "y"), mk(2.0, "z")]).len(), 1);
    // 29,97 fps: Schnitte auf dem Bildraster, Bereich bleibt exakt
    let ntsc = cl_core::Rational::new(30000, 1001);
    let v = split_parts(Split::Count(4), (s(0.0), s(7.0)), ntsc, &[], "F", "Part", "webm");
    assert_eq!((v.len(), v[3].0 .1, v[0].1.as_str()), (4, s(7.0), "F – Part 01.webm"));
    for w in v.windows(2) {
        assert_eq!(w[0].0 .1, w[1].0 .0);
        assert_eq!(w[0].0 .1, w[0].0 .1.snap_to_frame(ntsc));
    }
}
