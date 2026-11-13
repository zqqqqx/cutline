//! Creator-Werkzeuge (Menü „Werkzeuge“ in der Zeitleiste):
//! Pausen entfernen (Jump Cuts), Füllwörter entfernen, Highlights als Marker, Auto-Ducking,
//! Zoom-Punch-in, Aufnahmen per Ton synchronisieren, Multicam-Schnitt, Rückblick erstellen,
//! Hochkant-Layout, Thumbnail exportieren, Sprecher-Aufnahmen importieren.
//! Analysen laufen im Hintergrund; das Ergebnis wird als ein Undo-Schritt angewendet.

use std::path::PathBuf;

use cl_core::*;
use eframe::egui;

use crate::app::App;
use crate::tasks::TaskDone;
use crate::theme::icon;

/// Ton eines Clips: (Datei, Quellbeginn, Quelllänge, Tempo)
fn clip_audio(app: &App, c: &Clip) -> Option<(PathBuf, f64, f64, f64)> {
    let ClipContent::Media { media, kind } = &c.content else {
        return None;
    };
    let m = app.project.media(*media)?;
    if !(m.has_audio || *kind == MediaKind::Audio) {
        return None;
    }
    let sp = c.speed.max(0.01) as f64;
    Some((
        m.path.clone(),
        c.source_in.as_secs_f64(),
        c.duration.as_secs_f64() * sp,
        sp,
    ))
}

/// Teile eines Clips entfernen (`cut` = Bereiche in Clipzeit, Sekunden) und den Rest
/// lückenlos zusammenschieben; folgende Clips derselben Spur rücken nach.
fn cut_ranges_ops(p: &Project, clip: ClipId, cut: &[(f64, f64)]) -> Vec<EditOp> {
    let Some((t, c)) = p.find_clip(clip) else {
        return vec![];
    };
    let dur = c.duration.as_secs_f64();
    let sp = c.speed.max(0.01) as f64;
    let mut cut: Vec<(f64, f64)> = cut
        .iter()
        .map(|(a, b)| (a.max(0.0), b.min(dur)))
        .filter(|(a, b)| b - a > 0.02)
        .collect();
    cut.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    // Behalten = Lücken zwischen den Schnitten
    let mut keep = Vec::new();
    let mut pos = 0.0;
    for (a, b) in &cut {
        if *a > pos + 0.04 {
            keep.push((pos, *a));
        }
        pos = pos.max(*b);
    }
    if dur > pos + 0.04 {
        keep.push((pos, dur));
    }
    if keep.is_empty() || cut.is_empty() {
        return vec![];
    }
    let removed: f64 = dur - keep.iter().map(|(a, b)| b - a).sum::<f64>();
    let mut ops = vec![EditOp::RemoveClip { clip }];
    let mut at = c.start.as_secs_f64();
    for (a, b) in keep {
        let mut n = c.clone();
        n.id = ClipId::new();
        n.start = Time::from_secs_f64(at);
        n.duration = Time::from_secs_f64(b - a);
        n.source_in = Time::from_secs_f64(c.source_in.as_secs_f64() + a * sp);
        n.keyframes = Keyframes::default();
        n.transition_out = None;
        at += b - a;
        ops.push(EditOp::AddClip {
            track: Some(t.id),
            clip: n,
        });
    }
    // Hauptspur mit Magnet schließt selbst; sonst folgende Clips nachziehen
    if !(t.main && p.settings.main_track_magnet) {
        for o in t.clips.iter().filter(|o| o.start > c.start && o.id != clip) {
            ops.push(EditOp::MoveClip {
                clip: o.id,
                track: Some(t.id),
                start: Time::from_secs_f64((o.start.as_secs_f64() - removed).max(0.0)),
            });
        }
    }
    ops
}

impl App {
    fn selected_clip(&self) -> Option<Clip> {
        self.selected
            .and_then(|s| self.project.find_clip(s).map(|(_, c)| c.clone()))
    }

    /// Menü „Werkzeuge“ (in der Werkzeugleiste der Zeitleiste)
    pub fn tools_menu(&mut self, ui: &mut egui::Ui) {
        let sel = self.selected_clip();
        let has_audio = sel.as_ref().is_some_and(|c| clip_audio(self, c).is_some());
        let visual = sel.as_ref().is_some_and(|c| c.kind() == TrackKind::Video);
        let n_sel = self.selection().len();
        let item = |ui: &mut egui::Ui, enabled: bool, g: &str, t: &str, tip: &str| -> bool {
            ui.add_enabled(
                enabled,
                egui::Button::new(crate::tr(format!("{g}   {t}")))
                    .frame(false)
                    .min_size(egui::vec2(250.0, 22.0)),
            )
            .on_hover_text(crate::tr(tip))
            .on_disabled_hover_text(crate::tr(tip))
            .clicked()
        };
        let ctx = ui.ctx().clone();
        let mut act: Option<&str> = None;
        ui.label(
            egui::RichText::new(crate::tr("Ton und Sprache"))
                .small()
                .color(crate::theme::TEXT_FAINT),
        );
        if item(
            ui,
            has_audio,
            icon::SCISSORS,
            "Pausen entfernen (Jump Cuts)",
            "Schneidet stille Stellen aus dem ausgewählten Clip",
        ) {
            act = Some("jump");
        }
        if item(
            ui,
            has_audio,
            icon::CHAT_CIRCLE_DOTS,
            "Füllwörter entfernen (äh, ähm, uh, um)",
            "Erkennt Füllwörter lokal (Parakeet) und schneidet sie heraus",
        ) {
            act = Some("filler");
        }
        if item(
            ui,
            has_audio,
            icon::SPEAKER_SIMPLE_LOW,
            "Musik unter Sprache absenken (Auto-Ducking)",
            "Ausgewählter Musik-Clip wird leiser, wo auf anderen Spuren gesprochen wird",
        ) {
            act = Some("duck");
        }
        if item(
            ui,
            has_audio,
            icon::LIGHTNING,
            "Highlights finden (laute Momente → Marker)",
            "Setzt Marker an Schreien, Explosionen, Jubel im ausgewählten Clip",
        ) {
            act = Some("highlights");
        }
        ui.separator();
        ui.label(
            egui::RichText::new(crate::tr("Mehrere Aufnahmen"))
                .small()
                .color(crate::theme::TEXT_FAINT),
        );
        if item(ui, n_sel >= 2, icon::WAVEFORM, "Auswahl per Ton synchronisieren", "Richtet alle ausgewählten Clips am Ton des zuerst gewählten aus (z. B. mehrere Blickwinkel derselben Szene)") {
            act = Some("sync");
        }
        if item(ui, n_sel >= 2, icon::VIDEO_CAMERA, "Multicam-Schnitt starten", "Synchronisierte Clips auf eigenen Spuren: während der Wiedergabe mit 1–9 den Blickwinkel wählen") {
            act = Some("multicam");
        }
        if item(ui, true, icon::USERS, "Sprecher-Aufnahmen importieren …", "Eine Datei pro Person (z. B. aus einer Call-Aufnahme, auch als ZIP): je eine benannte Tonspur am Playhead") {
            act = Some("speakers");
        }
        ui.separator();
        ui.label(
            egui::RichText::new(crate::tr("Bild und Format"))
                .small()
                .color(crate::theme::TEXT_FAINT),
        );
        if item(
            ui,
            visual,
            icon::MAGNIFYING_GLASS_PLUS,
            "Zoom-Punch-in am Playhead",
            "Kurzer Zoom auf 125 % mit Keyframes (danach anpassbar)",
        ) {
            act = Some("punch");
        }
        if item(
            ui,
            visual,
            icon::DEVICE_MOBILE,
            "Hochkant-Layout (9:16, Facecam oben)",
            "Stellt auf 1080×1920 um: Gameplay unten, Kopie des Clips oben als Facecam-Ausschnitt",
        ) {
            act = Some("vertical");
        }
        if item(
            ui,
            true,
            icon::IMAGE,
            "Thumbnail exportieren (aktuelles Bild)",
            "Speichert das Bild am Playhead als PNG (1280×720 bei 16:9)",
        ) {
            act = Some("thumb");
        }
        if item(
            ui,
            !self.project.timeline.markers.is_empty(),
            icon::CLOCK_COUNTER_CLOCKWISE,
            "Rückblick aus Markern erstellen",
            "Neues Projekt mit je 3 s um jeden Marker, mit Überblendungen („Was bisher geschah“)",
        ) {
            act = Some("recap");
        }
        if crate::story::enabled(self) {
            ui.separator();
            self.story_menu(ui);
        }
        if let Some(a) = act {
            ui.close_menu();
            match a {
                "jump" => self.tool_jump_cuts(&ctx),
                "filler" => self.tool_fillers(&ctx),
                "duck" => self.tool_ducking(&ctx),
                "highlights" => self.tool_highlights(&ctx),
                "sync" => self.tool_sync(&ctx),
                "multicam" => self.tool_multicam_start(),
                "speakers" => self.tool_speakers(&ctx),
                "punch" => self.tool_punch_in(),
                "vertical" => self.tool_vertical(),
                "thumb" => self.tool_thumbnail(&ctx),
                _ => self.tool_recap(),
            }
        }
    }

    pub fn tool_jump_cuts(&mut self, ctx: &egui::Context) {
        let Some(c) = self.selected_clip() else {
            return;
        };
        let Some((path, src, span, sp)) = clip_audio(self, &c) else {
            return;
        };
        let id = c.id;
        self.tasks
            .spawn(ctx, "tool", "Pausen werden gesucht …", move |p, _| {
                p(0.3);
                match cl_analyze::audio::envelope(&path, src, Some(span)) {
                    Ok(env) => {
                        let pad = 0.12;
                        let cuts: Vec<(f64, f64)> = cl_analyze::audio::silences(&env, -32.0, 0.55)
                            .into_iter()
                            .map(|(a, b)| ((a + pad) / sp, (b - pad) / sp))
                            .filter(|(a, b)| b > a)
                            .collect();
                        TaskDone::Run(Box::new(move |app: &mut App| {
                            let ops = cut_ranges_ops(&app.project, id, &cuts);
                            if ops.is_empty() {
                                app.toast("Keine längeren Pausen gefunden");
                            } else {
                                let total: f64 = cuts.iter().map(|(a, b)| b - a).sum();
                                app.apply(EditOp::Batch {
                                    label: "Pausen entfernen".into(),
                                    ops,
                                });
                                app.toast(format!("{} Pausen entfernt ({total:.1} s)", cuts.len()));
                            }
                        }))
                    }
                    Err(e) => TaskDone::Failed(e),
                }
            });
    }

    pub fn tool_fillers(&mut self, ctx: &egui::Context) {
        let Some(c) = self.selected_clip() else {
            return;
        };
        let Some((path, src, span, sp)) = clip_audio(self, &c) else {
            return;
        };
        let models = self.models();
        if models.parakeet().is_none() {
            self.toast_err("Benötigt: Parakeet TDT 0.6B v3 (Einstellungen → Modelle)");
            return;
        }
        let id = c.id;
        self.tasks.spawn(
            ctx,
            "tool",
            "Füllwörter werden gesucht …",
            move |p, cancel| match cl_analyze::asr::parakeet(&models, &path, src, span, p, cancel) {
                Ok(words) => {
                    let cuts: Vec<(f64, f64)> = words
                        .iter()
                        .filter(|w| cl_analyze::audio::is_filler(&w.text))
                        .map(|w| ((w.start - 0.03).max(0.0) / sp, (w.end + 0.03) / sp))
                        .collect();
                    TaskDone::Run(Box::new(move |app: &mut App| {
                        let ops = cut_ranges_ops(&app.project, id, &cuts);
                        if ops.is_empty() {
                            app.toast("Keine Füllwörter gefunden");
                        } else {
                            app.apply(EditOp::Batch {
                                label: "Füllwörter entfernen".into(),
                                ops,
                            });
                            app.toast(format!("{} Füllwörter entfernt", cuts.len()));
                        }
                    }))
                }
                Err(e) => TaskDone::Failed(e),
            },
        );
    }

    pub fn tool_ducking(&mut self, ctx: &egui::Context) {
        let Some(music) = self.selected_clip() else {
            return;
        };
        // Sprache = alle anderen Clips mit Ton, die den Musik-Clip überlappen
        let voices: Vec<(PathBuf, f64, f64, f64, f64)> = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.muted)
            .flat_map(|t| t.clips.iter())
            .filter(|c| {
                c.id != music.id && !c.disabled && c.start < music.end() && c.end() > music.start
            })
            .filter_map(|c| {
                clip_audio(self, c).map(|(p, s, l, sp)| (p, s, l, sp, c.start.as_secs_f64()))
            })
            .collect();
        if voices.is_empty() {
            self.toast_err("Keine anderen Tonspuren unter dem Musik-Clip");
            return;
        }
        let (m0, m1, id) = (
            music.start.as_secs_f64(),
            music.end().as_secs_f64(),
            music.id,
        );
        let base = music.volume_db;
        self.tasks
            .spawn(ctx, "tool", "Sprache wird erkannt …", move |p, _| {
                // Sprechbereiche in Projektzeit sammeln
                let mut speech: Vec<(f64, f64)> = Vec::new();
                for (i, (path, src, span, sp, start)) in voices.iter().enumerate() {
                    p(i as f32 / voices.len() as f32);
                    let Ok(env) = cl_analyze::audio::envelope(path, *src, Some(*span)) else {
                        continue;
                    };
                    let len = env.len() as f64 / cl_analyze::audio::ENV_RATE as f64;
                    let mut pos = 0.0;
                    for (a, b) in cl_analyze::audio::silences(&env, -30.0, 0.8)
                        .into_iter()
                        .chain(std::iter::once((len, len)))
                    {
                        if a > pos {
                            speech.push((start + pos / sp, start + a / sp));
                        }
                        pos = b;
                    }
                }
                speech.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                // überlappende zusammenfassen
                let mut merged: Vec<(f64, f64)> = Vec::new();
                for s in speech {
                    match merged.last_mut() {
                        Some(l) if s.0 <= l.1 + 0.5 => l.1 = l.1.max(s.1),
                        _ => merged.push(s),
                    }
                }
                TaskDone::Run(Box::new(move |app: &mut App| {
                    let Some(mut c) = app.project.find_clip(id).map(|(_, c)| c.clone()) else {
                        return;
                    };
                    let low = base - 12.0;
                    let ramp = 0.3;
                    let rel = |t: f64| Time::from_secs_f64((t - m0).clamp(0.0, m1 - m0));
                    let mut n = 0;
                    c.keyframes.0.remove(&Prop::Volume);
                    for (a, b) in merged.iter().filter(|(a, b)| *b > m0 && *a < m1) {
                        c.keyframes.set(Prop::Volume, rel(a - ramp), base);
                        c.keyframes.set(Prop::Volume, rel(*a), low);
                        c.keyframes.set(Prop::Volume, rel(*b), low);
                        c.keyframes.set(Prop::Volume, rel(b + ramp), base);
                        n += 1;
                    }
                    if n == 0 {
                        app.toast("Keine Sprache unter der Musik gefunden");
                        return;
                    }
                    app.apply(EditOp::SetKeyframes {
                        clip: id,
                        keyframes: c.keyframes,
                    });
                    app.toast(format!("Musik an {n} Stellen abgesenkt (−12 dB)"));
                }))
            });
    }

    pub fn tool_highlights(&mut self, ctx: &egui::Context) {
        let Some(c) = self.selected_clip() else {
            return;
        };
        let Some((path, src, span, sp)) = clip_audio(self, &c) else {
            return;
        };
        let start = c.start.as_secs_f64();
        self.tasks
            .spawn(ctx, "tool", "Highlights werden gesucht …", move |p, _| {
                p(0.3);
                match cl_analyze::audio::envelope(&path, src, Some(span)) {
                    Ok(env) => {
                        let n = ((span / 60.0).ceil() as usize * 3).clamp(3, 40);
                        let hits: Vec<f64> = cl_analyze::audio::loud_moments(&env, n, 8.0)
                            .into_iter()
                            .map(|t| start + t / sp)
                            .collect();
                        TaskDone::Run(Box::new(move |app: &mut App| {
                            if hits.is_empty() {
                                app.toast("Keine auffällig lauten Momente gefunden");
                                return;
                            }
                            let mut ms = app.project.timeline.markers.clone();
                            let first = ms.iter().map(|m| m.id).max().unwrap_or(0) + 1;
                            for (i, t) in hits.iter().enumerate() {
                                ms.push(Marker {
                                    id: first + i as u64,
                                    at: Time::from_secs_f64(*t),
                                    duration: Time::ZERO,
                                    name: format!("Highlight {}", i + 1),
                                    note: String::new(),
                                    color: 4,
                                });
                            }
                            app.apply(EditOp::SetMarkers { markers: ms });
                            app.toast(format!("{} Highlights als Marker gesetzt", hits.len()));
                        }))
                    }
                    Err(e) => TaskDone::Failed(e),
                }
            });
    }

    pub fn tool_sync(&mut self, ctx: &egui::Context) {
        let Some(ref_clip) = self.selected_clip() else {
            return;
        };
        let Some((rp, rs, rl, _)) = clip_audio(self, &ref_clip) else {
            self.toast_err("Der zuerst gewählte Clip braucht Ton");
            return;
        };
        let others: Vec<(ClipId, PathBuf, f64, f64)> = self
            .selection()
            .into_iter()
            .filter(|id| *id != ref_clip.id)
            .filter_map(|id| self.project.find_clip(id).map(|(_, c)| c.clone()))
            .filter_map(|c| clip_audio(self, &c).map(|(p, s, l, _)| (c.id, p, s, l)))
            .collect();
        let r_start = ref_clip.start.as_secs_f64();
        self.tasks.spawn(
            ctx,
            "tool",
            "Aufnahmen werden synchronisiert …",
            move |p, _| {
                let Ok(a) = cl_analyze::audio::envelope(&rp, rs, Some(rl.min(120.0))) else {
                    return TaskDone::Failed("Ton nicht lesbar".into());
                };
                let mut moves = Vec::new();
                for (i, (id, path, src, len)) in others.iter().enumerate() {
                    p(i as f32 / others.len().max(1) as f32);
                    let Ok(b) = cl_analyze::audio::envelope(path, *src, Some(len.min(120.0)))
                    else {
                        continue;
                    };
                    if let Some((o, score)) = cl_analyze::audio::offset(&a, &b, 60.0) {
                        if score > 0.05 {
                            moves.push((*id, (r_start - o).max(0.0)));
                        }
                    }
                }
                TaskDone::Run(Box::new(move |app: &mut App| {
                    if moves.is_empty() {
                        app.toast_err("Kein gemeinsamer Ton gefunden");
                        return;
                    }
                    let ops: Vec<EditOp> = moves
                        .iter()
                        .filter_map(|(id, s)| {
                            app.project.find_clip(*id).map(|(t, _)| EditOp::MoveClip {
                                clip: *id,
                                track: Some(t.id),
                                start: Time::from_secs_f64(*s),
                            })
                        })
                        .collect();
                    let n = ops.len();
                    app.apply(EditOp::Batch {
                        label: "Synchronisieren".into(),
                        ops,
                    });
                    app.toast(format!("{n} Aufnahmen synchronisiert"));
                }))
            },
        );
    }

    /// Multicam: ausgewählte Clips als Blickwinkel; 1–9 während der Wiedergabe wählt.
    pub fn tool_multicam_start(&mut self) {
        let mut angles: Vec<(usize, ClipId)> = self
            .selection()
            .into_iter()
            .filter_map(|id| {
                self.project
                    .timeline
                    .tracks
                    .iter()
                    .position(|t| t.clips.iter().any(|c| c.id == id))
                    .map(|ti| (ti, id))
            })
            .collect();
        angles.sort_by_key(|a| a.0);
        if angles.len() < 2 {
            return;
        }
        self.multicam = Some((angles.into_iter().map(|a| a.1).collect(), Vec::new()));
        self.toast("Multicam: Wiedergabe starten und mit 1–9 den Blickwinkel wählen, Stopp übernimmt den Schnitt");
    }

    /// Taste 1–9 im Multicam-Modus
    pub fn multicam_switch(&mut self, n: usize) {
        if let Some((angles, cuts)) = &mut self.multicam {
            if n < angles.len() {
                cuts.push((self.playhead, n));
                self.toast(format!("Blickwinkel {}", n + 1));
            }
        }
    }

    /// Beim Anhalten: Schnitte anwenden (nicht gewählte Blickwinkel werden je Abschnitt deaktiviert).
    pub fn multicam_finish(&mut self) {
        let Some((angles, mut cuts)) = self.multicam.take() else {
            return;
        };
        if cuts.is_empty() {
            return;
        }
        cuts.sort_by_key(|c| c.0);
        let mut ops = Vec::new();
        for (ai, id) in angles.iter().enumerate() {
            let Some((t, c)) = self.project.find_clip(*id).map(|(t, c)| (t.id, c.clone())) else {
                continue;
            };
            // Abschnitte dieses Clips an allen Umschaltpunkten
            let mut bounds: Vec<Time> = cuts
                .iter()
                .map(|c| c.0)
                .filter(|x| *x > c.start && *x < c.end())
                .collect();
            bounds.dedup();
            let mut pieces = Vec::new();
            let mut s = c.start;
            for b in bounds.iter().chain(std::iter::once(&c.end())) {
                pieces.push((s, *b));
                s = *b;
            }
            ops.push(EditOp::RemoveClip { clip: c.id });
            for (a, b) in pieces {
                // aktiver Blickwinkel zum Abschnittsbeginn
                let active = cuts
                    .iter()
                    .rev()
                    .find(|x| x.0 <= a)
                    .map(|x| x.1)
                    .unwrap_or(0);
                let mut n = c.clone();
                n.id = ClipId::new();
                n.start = a;
                n.duration = Time(b.0 - a.0);
                n.source_in =
                    Time(c.source_in.0 + ((a.0 - c.start.0) as f64 * c.speed as f64) as i64);
                n.disabled = active != ai;
                if active != ai {
                    n.volume_db = c.volume_db; // Ton der anderen Spuren bleibt, nur das Bild wird ausgeblendet
                }
                n.keyframes = Keyframes::default();
                ops.push(EditOp::AddClip {
                    track: Some(t),
                    clip: n,
                });
            }
        }
        self.apply(EditOp::Batch {
            label: "Multicam-Schnitt".into(),
            ops,
        });
        self.toast(format!(
            "Multicam-Schnitt mit {} Wechseln übernommen",
            cuts.len()
        ));
    }

    pub fn tool_speakers(&mut self, _ctx: &egui::Context) {
        let Some(mut files) = crate::dialog()
            .add_filter(
                crate::tr("Audio oder ZIP"),
                &["wav", "flac", "mp3", "ogg", "m4a", "opus", "aac", "zip"],
            )
            .pick_files()
        else {
            return;
        };
        // ZIP (z. B. Call-Aufnahme mit einer Datei pro Person) entpacken
        let mut extra = Vec::new();
        for f in files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")))
        {
            let dir = crate::paths::data_dir().join("Recordings").join(
                f.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            if cl_update::extract_zip(f, &dir).is_ok() {
                let mut found: Vec<PathBuf> = walk_audio(&dir);
                found.sort();
                extra.extend(found);
            }
        }
        files.retain(|f| !f.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")));
        files.extend(extra);
        let at = self.playhead;
        let mut n = 0;
        for f in files {
            let name = f
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Craig & Co.: „1-Name.flac“ → „Name“
            let name = name
                .split_once('-')
                .filter(|(a, _)| a.chars().all(|c| c.is_ascii_digit()))
                .map(|(_, b)| b.to_string())
                .unwrap_or(name);
            let Some(media) = self.import_media(vec![f]).first().copied() else {
                continue;
            };
            let Some(OpOutput::Track(t)) = self.apply(EditOp::InsertTrack {
                index: self.project.timeline.tracks.len(),
                kind: TrackKind::Audio,
            }) else {
                continue;
            };
            self.apply(EditOp::RenameTrack {
                track: t,
                name: Some(name),
            });
            self.add_media_at(media, at, Some(t));
            n += 1;
        }
        if n > 0 {
            self.toast(format!("{n} Sprecherspuren angelegt"));
        }
    }

    pub fn tool_punch_in(&mut self) {
        let Some(mut c) = self.selected_clip() else {
            return;
        };
        let rel = (self.playhead.as_secs_f64() - c.start.as_secs_f64())
            .clamp(0.0, c.duration.as_secs_f64());
        let base = c
            .keyframes
            .eval(Prop::Scale, Time::from_secs_f64(rel))
            .unwrap_or(c.transform.scale);
        let end = c.duration.as_secs_f64();
        let k = |t: f64| Time::from_secs_f64(t.clamp(0.0, end));
        c.keyframes.set(Prop::Scale, k(rel), base);
        c.keyframes.set(Prop::Scale, k(rel + 0.2), base * 1.25);
        c.keyframes.set(Prop::Scale, k(rel + 1.4), base * 1.25);
        c.keyframes.set(Prop::Scale, k(rel + 1.7), base);
        self.apply(EditOp::SetKeyframes {
            clip: c.id,
            keyframes: c.keyframes,
        });
        self.toast("Zoom-Punch-in gesetzt (Keyframes im Basis-Reiter anpassbar)");
    }

    pub fn tool_vertical(&mut self) {
        let Some(c) = self.selected_clip() else {
            return;
        };
        let mut s = self.project.settings.clone();
        s.width = 1080;
        s.height = 1920;
        s.aspect = AspectPreset::Ratio(9, 16);
        let mut ops = vec![EditOp::SetSettings { settings: s }];
        // Gameplay unten (füllt die Breite), Facecam-Kopie oben, stark vergrößert
        let mut game = c.clone();
        game.transform = Transform {
            x: 0.0,
            y: 420.0,
            scale: 1.25,
            rotation_deg: 0.0,
        };
        ops.push(EditOp::ReplaceClip { clip: game });
        let mut cam = c.clone();
        cam.id = ClipId::new();
        cam.transform = Transform {
            x: -400.0,
            y: -260.0,
            scale: 3.0,
            rotation_deg: 0.0,
        };
        cam.audio = AudioFx::default();
        cam.volume_db = -60.0;
        cam.fx.mask = Some(Mask {
            cx: 0.5,
            cy: 0.5,
            w: 0.5,
            h: 0.5,
            feather: 0.02,
            ..Mask::new(MaskShape::Rect)
        });
        let above = self
            .project
            .timeline
            .tracks
            .iter()
            .position(|t| t.clips.iter().any(|x| x.id == c.id))
            .map(|i| i + 1)
            .unwrap_or(1);
        ops.push(EditOp::InsertTrack {
            index: above,
            kind: TrackKind::Video,
        });
        self.apply(EditOp::Batch {
            label: "Hochkant-Layout".into(),
            ops,
        });
        // Spur wurde eben angelegt: Facecam darauf
        if let Some(t) = self.project.timeline.tracks.get(above).map(|t| t.id) {
            self.apply(EditOp::AddClip {
                track: Some(t),
                clip: cam,
            });
        }
        self.toast("Hochkant-Layout: Facecam oben mit Position X/Y auf dein Gesicht ausrichten");
    }

    pub fn tool_thumbnail(&mut self, ctx: &egui::Context) {
        let base = self
            .project_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(crate::paths::data_dir);
        let Some(out) = crate::dialog()
            .set_directory(&base)
            .set_file_name(format!("{} – Thumbnail.png", self.project.name))
            .add_filter("PNG", &["png"])
            .save_file()
        else {
            return;
        };
        let p = self.project.clone();
        let t = self.playhead;
        let fonts = self.font_files();
        let fps = p.settings.fps;
        self.tasks
            .spawn(ctx, "tool", "Thumbnail wird erstellt …", move |_, _| {
                let mut s = cl_export::ExportSettings::from_project(&p);
                // 16:9 → 1280×720, sonst Projektgröße
                if p.settings.width * 9 == p.settings.height * 16 {
                    s.width = 1280;
                    s.height = 720;
                }
                s.include_audio = false;
                let frame = Time::from_secs_f64(1.0 / fps.as_f64());
                s.range = Some((t, t + frame + frame));
                let tmp =
                    std::env::temp_dir().join(format!("cutline-thumb-{}.mp4", std::process::id()));
                let resolver = move |fam: Option<&str>| {
                    fam.and_then(|f| fonts.get(f).cloned())
                        .or_else(|| cl_export::default_font().map(|p| (p, 0)))
                };
                let res = cl_export::build(&p, &s, &resolver, &tmp).and_then(|job| {
                    cl_export::run(
                        &job,
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        |_| {},
                    )
                });
                if let Err(e) = res {
                    return TaskDone::Failed(e.to_string());
                }
                let ok = cl_media::ffmpeg_command()
                    .map(|mut c| {
                        c.args(["-v", "error", "-y", "-i"])
                            .arg(&tmp)
                            .args(["-frames:v", "1"])
                            .arg(&out)
                            .status()
                            .map(|s| s.success())
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                let _ = std::fs::remove_file(&tmp);
                if !ok {
                    return TaskDone::Failed("Thumbnail konnte nicht gespeichert werden".into());
                }
                TaskDone::Run(Box::new(move |app: &mut App| {
                    app.toast(format!("Thumbnail gespeichert: {}", out.display()));
                    crate::home::reveal(&out);
                }))
            });
    }

    /// Neues Projekt mit je 3 s um jeden Marker (Ton und Bild aller Spuren).
    pub fn tool_recap(&mut self) {
        let src = self.project.clone();
        let mut ms = src.timeline.markers.clone();
        ms.sort_by_key(|m| m.at);
        let mut p = Project::new(format!("{} – Rückblick", src.name));
        p.settings = src.settings.clone();
        p.media = src.media.clone();
        p.timeline.tracks = src
            .timeline
            .tracks
            .iter()
            .map(|t| Track {
                clips: vec![],
                ..t.clone()
            })
            .collect();
        let len = 3.0;
        let mut at = 0.0;
        for m in &ms {
            let (a, b) = (
                (m.at.as_secs_f64() - 1.0).max(0.0),
                m.at.as_secs_f64() + len - 1.0,
            );
            for (ti, t) in src.timeline.tracks.iter().enumerate() {
                for c in t.clips.iter().filter(|c| {
                    !c.disabled && c.start.as_secs_f64() < b && c.end().as_secs_f64() > a
                }) {
                    let (s, e) = (a.max(c.start.as_secs_f64()), b.min(c.end().as_secs_f64()));
                    let mut n = c.clone();
                    n.id = ClipId::new();
                    n.start = Time::from_secs_f64(at + s - a);
                    n.duration = Time::from_secs_f64(e - s);
                    n.source_in = Time::from_secs_f64(
                        c.source_in.as_secs_f64() + (s - c.start.as_secs_f64()) * c.speed as f64,
                    );
                    n.keyframes = Keyframes::default();
                    n.anim = Animations::default();
                    n.transition_out = t.main.then(|| TransitionRef {
                        id: "fade".into(),
                        duration: Time::from_secs_f64(0.4),
                    });
                    p.timeline.tracks[ti].clips.push(n);
                }
            }
            at += b - a;
        }
        self.new_project();
        *self.doc.lock().unwrap() = Document::new(p);
        self.revision = u64::MAX;
        self.sync_project();
        self.toast(format!("Rückblick aus {} Markern erstellt", ms.len()));
    }
}

fn walk_audio(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_audio(&p));
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["wav", "flac", "mp3", "ogg", "m4a", "opus", "aac"]
                .contains(&e.to_ascii_lowercase().as_str())
        }) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_ranges_keeps_rest_contiguous() {
        let mut p = Project::new("t");
        let mut tr = Track::new(TrackKind::Audio);
        let mut c = Clip::new(
            ClipContent::Media {
                media: MediaId::new(),
                kind: MediaKind::Audio,
            },
            Time::from_secs_f64(1.0),
            Time::from_secs_f64(10.0),
        );
        c.source_in = Time::from_secs_f64(2.0);
        let after = Clip::new(
            ClipContent::Media {
                media: MediaId::new(),
                kind: MediaKind::Audio,
            },
            Time::from_secs_f64(12.0),
            Time::from_secs_f64(1.0),
        );
        let (cid, aid) = (c.id, after.id);
        tr.clips = vec![c, after];
        p.timeline.tracks.push(tr);
        let ops = cut_ranges_ops(&p, cid, &[(2.0, 3.0), (5.0, 7.0)]);
        let adds: Vec<&Clip> = ops
            .iter()
            .filter_map(|o| {
                if let EditOp::AddClip { clip, .. } = o {
                    Some(clip)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(adds.len(), 3);
        assert_eq!(adds[1].start, Time::from_secs_f64(3.0));
        assert_eq!(adds[1].source_in, Time::from_secs_f64(5.0));
        assert_eq!(adds[2].start, Time::from_secs_f64(5.0));
        assert!(ops.iter().any(|o| matches!(o, EditOp::MoveClip { clip, start, .. } if *clip == aid && *start == Time::from_secs_f64(9.0))));
    }
}
