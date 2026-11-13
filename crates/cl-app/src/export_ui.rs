//! Export-Dialog: ohne Pro-Sperre, ohne „Urheberrecht überprüfen“, mit echtem Fortschritt.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cl_core::{Project, Time};
use cl_export::{Codec, Encoder, ExportSettings, Format, Quality};
use eframe::egui::{self, vec2, Color32, RichText};

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

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a.max(1)
    } else {
        gcd(b, a % b)
    }
}

struct Running {
    progress: Arc<Mutex<f32>>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
}

#[derive(Default)]
pub struct ExportDialog {
    open: bool,
    name: String,
    folder: PathBuf,
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

    pub fn open_for(&mut self, p: &Project, project_path: Option<&Path>) {
        if self.running.is_some() {
            self.open = true;
            return;
        }
        self.open = true;
        self.error = None;
        self.last_output = None;
        self.name = p.name.clone();
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
            Self::apply_platform(&mut s, &mut self.short_edge, pl);
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

    fn apply_platform(s: &mut ExportSettings, short_edge: &mut u32, pl: &Platform) {
        s.format = Format::Mp4;
        s.codec = pl.codec;
        s.quality = pl.quality;
        *short_edge = pl.w.min(pl.h);
        if let Some(f) = pl.fps {
            s.fps = cl_core::Rational::new(f, 1);
        }
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
    ) -> Option<String> {
        let mut toast = None;
        // Hintergrund-Export weiterverfolgen, auch wenn der Dialog geschlossen ist
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
        if self.running.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
            return toast;
        }
        let Some(mut s) = self.settings.clone() else {
            return toast;
        };
        let mut open = true;
        let mut start = false;
        egui::Window::new(RichText::new(crate::tr(format!("{}  Exportieren", icon::EXPORT))).strong())
            .id(egui::Id::new("export-dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(720.0, 0.0))
            .open(&mut open)
            .frame(egui::Frame::window(&ctx.style()).inner_margin(18))
            .show(ctx, |ui| {
                let busy = self.running.is_some();
                ui.horizontal_top(|ui| {
                    // Links: Cover/Vorschau
                    ui.vertical(|ui| {
                        ui.set_width(250.0);
                        let size = vec2(250.0, 250.0 * s.height as f32 / s.width.max(1) as f32).min(vec2(250.0, 300.0));
                        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                        ui.painter().rect_filled(rect, 6.0, Color32::BLACK);
                        if let Some((id, ts)) = cover {
                            let fit = (rect.width() / ts.x).min(rect.height() / ts.y);
                            ui.painter().image(id, egui::Rect::from_center_size(rect.center(), ts * fit), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new(crate::tr(format!("{} {:.1} s  ·  {}×{}", icon::CLOCK, p.duration().as_secs_f64(), s.width, s.height))).small().color(theme::TEXT_DIM));
                        if !s.format.audio_only() {
                            let est = estimate_mb(&s, p.duration());
                            ui.label(RichText::new(crate::tr(format!("{} ca. {est:.0} MB", icon::HARD_DRIVES))).small().color(theme::TEXT_DIM));
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
                                theme::row(ui, "Vorlage", |ui| {
                                    let sel = self.platform.and_then(platform).map(|pl| crate::tr(pl.name)).unwrap_or_else(|| crate::tr("Plattform wählen …"));
                                    egui::ComboBox::from_id_salt("exp-preset").selected_text(sel).width(200.0).show_ui(ui, |ui| {
                                        for pl in PLATFORMS {
                                            if ui.selectable_label(self.platform == Some(pl.id), crate::tr(pl.name)).clicked() {
                                                Self::apply_platform(&mut s, &mut self.short_edge, pl);
                                                self.platform = Some(pl.id);
                                                self.preset_hint = (!pl.fits(&p.settings)).then(|| crate::tr("Das Projekt hat ein anderes Seitenverhältnis – Format im Projekt-Reiter umstellen oder Werkzeuge → Hochkant-Layout"));
                                                if pl.fits(&p.settings) {
                                                    self.picked_platform = Some(pl.id);
                                                }
                                            }
                                        }
                                    });
                                });
                                if let Some(h) = &self.preset_hint {
                                    ui.label(egui::RichText::new(h).small().color(egui::Color32::from_rgb(0xE0, 0xA0, 0x40)));
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
                                            ui.checkbox(&mut s.transparent, crate::tr(if s.codec == Codec::ProRes { "Transparent (ProRes 4444)" } else { "Transparent (VP9 mit Alpha)" }))
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
                                    ui.checkbox(&mut s.include_audio, crate::tr("Ton exportieren (AAC 256 kbit/s, bei WebM Opus)"));
                                    if let Some((a, b)) = self.range_avail {
                                        let mut only = s.range.is_some();
                                        let fmt = |t: Time| format!("{:02}:{:02}.{:02}", (t.as_secs_f64() / 60.0) as u32, t.as_secs_f64() as u32 % 60, ((t.as_secs_f64().fract()) * 100.0) as u32);
                                        if ui.checkbox(&mut only, crate::tr(format!("Nur In/Out-Bereich ({} – {})", fmt(a), fmt(b)))).changed() {
                                            s.range = only.then_some((a, b));
                                        }
                                    }
                                }
                                ui.checkbox(&mut self.also_srt, crate::tr("Texte zusätzlich als Untertiteldatei (SRT) speichern"));
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
                    ui.add(egui::ProgressBar::new(prog).show_percentage().fill(theme::ACCENT).desired_height(8.0));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if let Some(out) = &self.last_output {
                        if self.running.is_none() && out.exists() {
                            if ui.add(theme::secondary(format!("{}  Im Ordner zeigen", icon::FOLDER_OPEN))).clicked() {
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
        if start {
            self.error = None;
            let out = self.output_path(s.format);
            if self.also_srt {
                let _ = std::fs::write(out.with_extension("srt"), cl_export::to_srt(p));
            }
            let fonts = fonts.clone();
            let resolver = move |fam: Option<&str>| fam.and_then(|f| fonts.get(f).cloned());
            match cl_export::build(p, &s, &resolver, &out) {
                Ok(job) => {
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
                    self.running = Some(Running {
                        progress,
                        result,
                        cancel,
                        started: Instant::now(),
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
fn estimate_mb(s: &ExportSettings, d: Time) -> f64 {
    let px = (s.width * s.height) as f64;
    let kbps = match s.quality {
        Quality::Bitrate(k) => k as f64,
        q => {
            let base = px / (1920.0 * 1080.0) * 9000.0 * s.fps.as_f64() / 30.0;
            base * match q {
                Quality::Lower => 0.5,
                Quality::Higher => 1.8,
                _ => 1.0,
            }
        }
    };
    let audio = if s.include_audio { 256.0 } else { 0.0 };
    (kbps + audio) * d.as_secs_f64() / 8.0 / 1000.0
}
