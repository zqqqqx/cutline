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
    encoders: Arc<Mutex<Option<Vec<Encoder>>>>,
    running: Option<Running>,
    last_output: Option<PathBuf>,
    error: Option<String>,
    /// Zeitpunkt und Dauer des letzten erfolgreichen Exports (kurze Erfolgs-Animation)
    done: Option<(Instant, f32)>,
    /// Projektstand beim Öffnen des Dialogs
    rev: u64,
    /// Projektstand und Datei des letzten fertigen Exports („Zur Datei“ statt „Exportieren“)
    exported: Option<(u64, PathBuf)>,
    /// Zieldatei existiert: Nachfrage vor dem Start
    confirm_overwrite: bool,
    /// „Projekt auf 9:16 umstellen“ geklickt (App stellt das Format um)
    switch_aspect: bool,
}

impl ExportDialog {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// In/Out-Bereich aus der Zeitleiste anbieten (vorausgewählt).
    pub fn set_range(&mut self, r: Option<(Time, Time)>) {
        self.range_avail = r.filter(|(a, b)| b > a);
        if let Some(s) = &mut self.settings {
            s.range = self.range_avail;
        }
    }

    pub fn open_for(&mut self, p: &Project, project_path: Option<&Path>, rev: u64) {
        self.rev = rev;
        if self.running.is_some() {
            self.open = true;
            return;
        }
        self.open = true;
        self.error = None;
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
                self.running = None;
                match res {
                    Ok(()) => {
                        self.done = Some((Instant::now(), secs));
                        if let Some(out) = &self.last_output {
                            // unter dem Projektnamen, nicht dem (änderbaren) Dateinamen
                            crate::exports::record(&p.name, self.project_path.as_deref(), out);
                            self.exported = Some((started_rev, out.clone()));
                        }
                        toast = Some(format!(
                            "{} Export fertig ({secs:.1} s)",
                            icon::CHECK_CIRCLE
                        ))
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
        // Esc schließt den Dialog (nicht während eines laufenden Exports)
        if self.running.is_none() && !self.confirm_overwrite && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
            return toast;
        }
        let Some(mut s) = self.settings.clone() else {
            return toast;
        };
        let mut open = true;
        let mut start = false;
        // ohne Nachfrage überschreiben (bestätigt oder „Erneut exportieren“)
        let mut overwrite = false;
        egui::Window::new(RichText::new(crate::tr(format!("{}  Exportieren", icon::EXPORT))).strong())
            .id(egui::Id::new("export-dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(720.0, 0.0))
            .open(&mut open)
            .frame(egui::Frame::window(&ctx.style()).inner_margin(theme::PAD_DIALOG))
            .show(ctx, |ui| {
                let busy = self.running.is_some();
                ui.horizontal_top(|ui| {
                    // Links: Cover/Vorschau
                    ui.vertical(|ui| {
                        ui.set_width(250.0);
                        let size = vec2(250.0, 250.0 * s.height as f32 / s.width.max(1) as f32).min(vec2(250.0, 300.0));
                        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                        ui.painter().rect_filled(rect, theme::R_CARD, Color32::BLACK);
                        if let Some((id, ts)) = cover {
                            let fit = (rect.width() / ts.x).min(rect.height() / ts.y);
                            ui.painter().image(id, egui::Rect::from_center_size(rect.center(), ts * fit), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new(crate::tr(format!("{} {:.1} s  ·  {}×{}", icon::CLOCK, p.duration().as_secs_f64(), s.width, s.height))).small().color(theme::TEXT_DIM()));
                        if !s.format.audio_only() {
                            let (est, exact) = estimate_mb(&s, p.duration());
                            let txt = if exact { format!("{} {est:.0} MB", icon::HARD_DRIVES) } else { format!("{} ca. {est:.0} MB", icon::HARD_DRIVES) };
                            ui.label(RichText::new(crate::tr(txt)).small().color(theme::TEXT_DIM()));
                        }
                    });
                    ui.add_space(12.0);
                    // Rechts: Einstellungen
                    ui.vertical(|ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            theme::row(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(300.0)));
                            theme::row(ui, "Speichern in", |ui| {
                                ui.add(egui::Label::new(RichText::new(crate::tr(self.folder.display().to_string())).small()).truncate());
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
                                                self.preset_hint = (!pl.fits(&p.settings)).then(|| crate::tr("Das Projekt hat ein anderes Seitenverhältnis – Format im Projekt-Reiter umstellen oder Quick Actions → Hochkant-Layout"));
                                                if pl.fits(&p.settings) {
                                                    self.picked_platform = Some(pl.id);
                                                }
                                            }
                                        }
                                    });
                                });
                                if let Some(h) = &self.preset_hint {
                                    ui.label(egui::RichText::new(h).small().color(theme::WARN));
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
                                    if let Some((a, b)) = self.range_avail {
                                        let mut only = s.range.is_some();
                                        let fmt = |t: Time| format!("{:02}:{:02}.{:02}", (t.as_secs_f64() / 60.0) as u32, t.as_secs_f64() as u32 % 60, ((t.as_secs_f64().fract()) * 100.0) as u32);
                                        if crate::widgets::check(ui, &mut only, crate::tr(format!("Nur In/Out-Bereich ({} – {})", fmt(a), fmt(b)))).changed() {
                                            s.range = only.then_some((a, b));
                                        }
                                    }
                                }
                            } else {
                                loudness_row(ui, &mut s.loudness);
                            }
                            if !s.format.audio_only() {
                                crate::widgets::check(ui, &mut self.also_srt, crate::tr("Texte zusätzlich als Untertiteldateien (SRT und VTT) speichern"));
                                let ms = &p.timeline.markers;
                                if !ms.is_empty() {
                                    theme::section(ui, "Kapitel");
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(format!("{} {}", ms.len(), crate::tr("aus Markern, werden eingebettet"))).small().color(theme::TEXT_DIM()));
                                        if ui.add(theme::secondary(format!("{}  YouTube-Kapitel kopieren", icon::COPY))).clicked() {
                                            ui.ctx().copy_text(crate::navigate::chapters(ms));
                                            toast = Some(crate::tr("Kapitel in die Zwischenablage kopiert"));
                                        }
                                    });
                                    if let Some(w) = crate::navigate::chapter_issue(ms, p.duration()) {
                                        ui.label(RichText::new(format!("{}  {}", icon::WARNING, crate::tr(w))).small().color(theme::SNAP));
                                    }
                                }
                            }
                        });
                    });
                });

                ui.add_space(12.0);
                if let Some(e) = &self.error {
                    ui.label(RichText::new(crate::tr(format!("{} {e}", icon::WARNING_CIRCLE))).color(theme::DANGER));
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
                        if self.running.is_none() && out.exists() && self.exported.as_ref().is_none_or(|(r, f)| *r != self.rev || f != out) {
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
                        } else if self.exported.as_ref().is_some_and(|(r, f)| *r == self.rev && f.is_file() && *f == self.output_path(s.format)) {
                            // unverändert seit dem letzten Export: direkt zur fertigen Datei
                            if ui.add(theme::primary(format!("{}  Zur Datei", icon::FOLDER_OPEN))).clicked() {
                                if let Some((_, f)) = &self.exported {
                                    crate::home::reveal(f);
                                }
                            }
                            if ui.add(theme::secondary(format!("{}  Erneut exportieren", icon::ARROW_CLOCKWISE))).clicked() {
                                start = true;
                                overwrite = true;
                            }
                        } else if ui.add(theme::primary(format!("{}  Exportieren", icon::EXPORT))).clicked() {
                            start = true;
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
            start = false;
        }
        if self.confirm_overwrite {
            // 0 = überschreiben, 1 = anderer Name, 2 = abbrechen
            let mut choice = None;
            let file = self.output_path(s.format);
            let m = egui::Modal::new(egui::Id::new("export-overwrite")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(crate::tr("Datei existiert bereits")).strong().size(theme::FS_TITLE));
                ui.add_space(8.0);
                ui.label(RichText::new(crate::tr(file.display().to_string())).small().color(theme::TEXT_DIM()));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::primary("Überschreiben")).clicked() {
                            choice = Some(0);
                        }
                        if ui.add(theme::secondary("Anderen Namen")).clicked() {
                            choice = Some(1);
                        }
                        if ui.add(theme::secondary("Abbrechen")).clicked() {
                            choice = Some(2);
                        }
                    });
                });
            });
            if choice.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                choice = Some(0);
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
            if self.also_srt {
                let _ = std::fs::write(out.with_extension("srt"), cl_export::to_srt(p));
                let _ = std::fs::write(out.with_extension("vtt"), cl_export::to_vtt(p));
            }
            // Kapitel wie „YouTube-Kapitel kopieren“ als Textdatei neben das Video
            if !s.format.audio_only() && !p.timeline.markers.is_empty() {
                let _ = std::fs::write(out.with_extension("chapters.txt"), crate::navigate::chapters(&p.timeline.markers));
            }
            let fonts = fonts.clone();
            let resolver = move |fam: Option<&str>| fam.and_then(|f| fonts.get(f).cloned());
            match cl_export::build(p, &s, &resolver, &out) {
                Ok(job) => {
                    let measures = job.measures_loudness();
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
                    });
                    self.last_output = Some(out);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        toast
    }
}

/// Grobe Dateigrößenschätzung (für die Anzeige im Dialog).
/// Fortschritt beim Export: weich nachlaufender Balken mit wanderndem Glanz, Prozent und Restzeit.
fn export_progress(ui: &mut egui::Ui, prog: f32, elapsed: f32, measuring: bool) {
    let shown = ui.ctx().animate_value_with_time(egui::Id::new("export-prog"), prog, 0.35);
    let t = ui.input(|i| i.time) as f32;
    ui.horizontal(|ui| {
        // 14 px wie ein Icon in der Zeile (Standard wäre Zeilenhöhe)
        ui.add(egui::Spinner::new().size(14.0).color(theme::ACCENT()));
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
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), egui::Sense::hover());
    let p = ui.painter();
    // Pille: Radius = halbe Balkenhöhe (8 px), bewusst ganz rund
    p.rect_filled(rect, 4.0, theme::BG_HOVER());
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * shown.clamp(0.0, 1.0), rect.height()));
    p.rect_filled(fill, 4.0, theme::ACCENT());
    // Glanz wandert über den gefüllten Teil
    if fill.width() > 12.0 {
        let x = fill.left() + (t * 0.8).fract() * (fill.width() + 60.0) - 30.0;
        let glow = Rect::from_center_size(egui::pos2(x, fill.center().y), vec2(36.0, fill.height()));
        p.with_clip_rect(fill).rect_filled(glow, 4.0, egui::Color32::from_white_alpha(70));
    }
    ui.ctx().request_repaint();
}

/// Erfolg: Häkchen springt kurz auf (ca. 0,4 s), bleibt 6 s sichtbar. `false` = ausgeblendet.
fn export_done(ui: &mut egui::Ui, at: Instant, secs: f32) -> bool {
    let age = at.elapsed().as_secs_f32();
    if age > 6.0 {
        return false;
    }
    // „ease out back“: leicht über 1 hinaus, dann zurück
    let k = (age / 0.4).min(1.0);
    let c = 1.70158;
    let pop = 1.0 + (c + 1.0) * (k - 1.0).powi(3) + c * (k - 1.0).powi(2);
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
    if age < 0.5 {
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
/// je +6 CRF etwa halbe Größe (Kleiner = CRF 26, Höher = CRF 16).
fn estimate_mb(s: &ExportSettings, d: Time) -> (f64, bool) {
    let px = (s.width * s.height) as f64;
    let kbps = match s.quality {
        Quality::Bitrate(k) => k as f64,
        q => {
            let base = px / (1920.0 * 1080.0) * 23_000.0 * s.fps.as_f64() / 30.0;
            base * match q {
                Quality::Lower => 0.5,
                Quality::Higher => 2f64.powf(4.0 / 6.0),
                _ => 1.0,
            }
        }
    };
    let exact = matches!(s.quality, Quality::Bitrate(_)) && s.codec != Codec::ProRes && s.format != Format::Gif;
    let secs = s.range.map_or(d.as_secs_f64(), |(a, b)| (b.as_secs_f64() - a.as_secs_f64()).max(0.0));
    ((kbps + s.audio_bitrate() as f64) * secs / 8.0 / 1000.0, exact)
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
fn estimate_matches_a_real_1080p30_export() {
    let mut s = ExportSettings::from_project(&Project::new("t"));
    (s.width, s.height, s.fps) = (1920, 1080, cl_core::Rational::parse("30").unwrap());
    let (mb, exact) = estimate_mb(&s, Time::from_secs_f64(40.0));
    assert!(!exact);
    // echte Datei: 117,6 MB
    assert!((80.0..=160.0).contains(&mb), "{mb}");
}
