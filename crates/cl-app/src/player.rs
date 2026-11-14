//! Player: setzt alle sichtbaren Ebenen am Playhead zusammen (Video/Bild/Sticker mit
//! Effektkette als Textur je Clip, Text mit Kontur/Fläche/Schatten), wertet Keyframes
//! und Animationen aus und blendet Übergänge über. Bearbeitung direkt im Bild über
//! `player_edit`.

use std::hash::{Hash, Hasher};

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText};

use crate::app::{App, DragAsset};
use crate::player_edit::Region;
use crate::preview::{to_color_image, Done, Job, Playback};
use crate::theme::{self, icon};

/// Eine sichtbare Ebene zum Zeitpunkt t.
struct Layer {
    clip: Clip,
    /// Zeit innerhalb des Clips (kann bei Übergängen außerhalb liegen)
    t_rel: Time,
    /// Zusätzliche Deckkraft (Übergänge)
    alpha: f32,
}

impl App {
    /// Fertige Decoder-Ergebnisse übernehmen.
    pub fn receive_decoded(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.decoder.rx.try_recv() {
            match d {
                Done::Frame { clip, key, frame } => {
                    if self.requested.get(&clip) == Some(&key) {
                        self.set_frame(ctx, clip, key, &frame);
                    }
                }
                Done::Thumb { media, frame } => {
                    let tex = ctx.load_texture(
                        format!("thumb-{media}"),
                        to_color_image(&frame),
                        egui::TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(media, tex);
                }
                Done::Wave { media, peaks } => {
                    self.waves.insert(media, peaks);
                }
                Done::SoundMeta { key, media, bars } => {
                    if self.sound_pending.as_deref() == Some(key.as_str()) {
                        self.sound_pending = None;
                        self.preview_media(key.clone(), media.clone());
                    }
                    self.sound_meta.insert(key, (media, bars));
                }
                Done::FxThumb { key, frame } => {
                    let tex = ctx.load_texture(
                        format!("fx-{key}"),
                        to_color_image(&frame),
                        egui::TextureOptions::LINEAR,
                    );
                    self.fx_thumbs.insert(key, tex);
                }
                Done::ProjectThumb { out } => self.home.thumb_ready(&out),
                Done::Skipped(key) => {
                    self.fx_thumbs_requested.remove(&key);
                }
                Done::Error(e) => eprintln!("Decoder: {e}"),
            }
        }
    }

    fn set_frame(&mut self, ctx: &egui::Context, clip: ClipId, key: u64, f: &cl_media::RgbaFrame) {
        if matches!(self.pick, Some(crate::pro::Pick::Chroma(c)) if c == clip) {
            self.frame_cpu = Some((clip, f.clone()));
        }
        let img = to_color_image(f);
        match self.frames.get_mut(&clip) {
            Some((k, t)) if t.size() == [f.width as usize, f.height as usize] => {
                t.set(img, egui::TextureOptions::LINEAR);
                *k = key;
            }
            _ => {
                let t =
                    ctx.load_texture(format!("frame-{clip}"), img, egui::TextureOptions::LINEAR);
                self.frames.insert(clip, (key, t));
                // Texturen gelöschter Clips nicht ewig halten
                if self.frames.len() > 64 {
                    let tl = &self.project.timeline;
                    self.frames.retain(|id, _| tl.tracks.iter().any(|t| t.clips.iter().any(|c| c.id == *id)));
                }
            }
        }
    }

    /// Alle sichtbaren Ebenen von unten nach oben (inkl. Übergängen).
    fn layers_at(&self, t: Time) -> Vec<Layer> {
        let mut out = Vec::new();
        for tr in self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|tr| !tr.hidden && tr.kind != TrackKind::Audio)
        {
            for (i, c0) in tr.clips.iter().enumerate() {
                // erst das Zeitfenster prüfen, dann klonen
                let c = self.clip_ref(c0);
                if c.disabled {
                    continue;
                }
                // Übergang am Ende dieses Clips?
                let next = tr
                    .clips
                    .get(i + 1)
                    .filter(|n| (n.start.0 - c.end().0).abs() < 2000 && !n.disabled);
                let tr_out = c
                    .transition_out
                    .as_ref()
                    .filter(|_| next.is_some())
                    .map(|x| x.duration.as_secs_f64().min(c.duration.as_secs_f64()));
                let prev = if i > 0 { tr.clips.get(i - 1) } else { None };
                let tr_in = prev
                    .filter(|p| (c.start.0 - p.end().0).abs() < 2000 && !p.disabled)
                    .and_then(|p| p.transition_out.as_ref())
                    .map(|x| x.duration.as_secs_f64().min(c.duration.as_secs_f64()));
                let ts = t.as_secs_f64();
                let (s, e) = (c.start.as_secs_f64(), c.end().as_secs_f64());
                let lo = s - tr_in.map(|d| d / 2.0).unwrap_or(0.0);
                let hi = e + tr_out.map(|d| d / 2.0).unwrap_or(0.0);
                if ts < lo || ts >= hi {
                    continue;
                }
                let mut alpha = 1.0;
                if let Some(d) = tr_in {
                    // Einblenden über dem Vorgänger
                    let p = ((ts - (s - d / 2.0)) / d).clamp(0.0, 1.0) as f32;
                    alpha = p;
                }
                let t_rel = Time::from_secs_f64((ts - s).clamp(0.0, (e - s - 1e-3).max(0.0)));
                out.push(Layer {
                    clip: c.clone(),
                    t_rel,
                    alpha,
                });
            }
        }
        out
    }

    fn frame_key(
        id: ClipId,
        c: &Clip,
        src: Time,
        w: u32,
        h: u32,
        t_rel: Time,
        animated_fx: bool,
    ) -> u64 {
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        (id, src, w, h).hash(&mut hs);
        // Timing gehört zum Schlüssel: verschobene/getrimmte Clips brauchen einen neuen Strom
        (c.start, c.duration, c.source_in, c.speed.to_bits()).hash(&mut hs);
        serde_json::to_string(&c.fx)
            .unwrap_or_default()
            .hash(&mut hs);
        if animated_fx {
            t_rel.hash(&mut hs);
        }
        hs.finish() | 1
    }

    /// Effekt-/Filter-Elemente auf Spuren über dem Clip `clip` (eingeschaltet, sichtbar).
    fn fx_elements_over(&self, clip: ClipId) -> Vec<Clip> {
        let tracks = &self.project.timeline.tracks;
        let Some(idx) = tracks
            .iter()
            .position(|tr| tr.clips.iter().any(|c| c.id == clip))
        else {
            return Vec::new();
        };
        tracks[idx + 1..]
            .iter()
            .filter(|tr| !tr.hidden && matches!(tr.kind, TrackKind::Effect | TrackKind::Filter))
            .flat_map(|tr| tr.clips.iter())
            .filter(|c| !c.disabled)
            .cloned()
            .collect()
    }

    /// Bilder für alle sichtbaren Medien-Ebenen anfordern bzw. aus Strömen holen.
    ///
    /// Flüssiges Abspielen: Der Strom eines Clips läuft ununterbrochen. Effekt-/Filter-Elemente
    /// darüber laufen als eigene Fassung des Clips (eigener Strom), die beim Zeichnen mit ihrer
    /// Stärke und ggf. nur innerhalb der Maske darübergelegt wird. Ströme für Clips und
    /// Elemente, die gleich beginnen, werden vorab geöffnet, damit an Schnitten nichts hakt.
    fn update_layers(&mut self, ctx: &egui::Context, layers: &[Layer], canvas_px: egui::Vec2) {
        const AHEAD: f64 = 1.5;
        // Beim Abspielen bleibt die Dekodiergröße fest (Umschalten auf den großen Player oder
        // Fenstergröße ändern startet die Ströme nicht neu); im Stand folgt sie der Ansicht.
        let canvas_px = if self.playing {
            *self.play_canvas_px.get_or_insert(canvas_px)
        } else {
            self.play_canvas_px = None;
            canvas_px
        };
        let mut visible = Vec::new();
        let mut overlays = Vec::new();
        let now = self.playhead;
        for l in layers {
            // Pipette: Bild ohne Chroma-Key zeigen, damit die Farbe sichtbar bleibt
            let mut c = l.clip.clone();
            if matches!(self.pick, Some(crate::pro::Pick::Chroma(id)) if id == c.id) {
                c.fx.chroma = None;
            }
            let ClipContent::Media { media, .. } = &c.content else {
                continue;
            };
            let Some(m) = self.project.media(*media).cloned() else {
                continue;
            };
            visible.push(c.id);
            self.request_frame(ctx, c.id, &c, &m, l, canvas_px, None);
            for e in self.fx_elements_over(c.id) {
                let active = e.contains(now);
                let soon = self.playing
                    && e.start > now
                    && (e.start - now).as_secs_f64() <= AHEAD
                    && e.start < c.end();
                if !active && !soon {
                    continue;
                }
                let (mut r, filter) = match &e.content {
                    ClipContent::Effect(r) => (r.clone(), false),
                    ClipContent::Filter(r) => (r.clone(), true),
                    _ => continue,
                };
                let animated = !e.keyframes.get(Prop::Strength).is_empty();
                if animated {
                    r.intensity = 1.0;
                }
                let mut x = c.clone();
                if filter {
                    x.fx.filter = Some(r);
                } else if !x.fx.effects.iter().any(|f| f.id == r.id) {
                    x.fx.effects.push(r);
                }
                let did = c.id.derived(e.id.0.as_u128());
                visible.push(did);
                if active {
                    self.request_frame(ctx, did, &x, &m, l, canvas_px, None);
                    let k = if animated {
                        e.strength_at(now - e.start)
                    } else {
                        1.0
                    };
                    overlays.push((c.id, did, e.fx.mask, k));
                } else {
                    let at = Layer {
                        clip: x.clone(),
                        t_rel: e.start - c.start,
                        alpha: l.alpha,
                    };
                    self.request_frame(ctx, did, &x, &m, &at, canvas_px, Some(e.start));
                }
            }
        }
        // Gleich beginnende Clips vorladen
        if self.playing {
            let upcoming: Vec<Clip> = self
                .project
                .timeline
                .tracks
                .iter()
                .filter(|tr| !tr.hidden && matches!(tr.kind, TrackKind::Video | TrackKind::Sticker))
                .flat_map(|tr| tr.clips.iter())
                .filter(|c| !c.disabled && c.start > now && (c.start - now).as_secs_f64() <= AHEAD)
                .cloned()
                .collect();
            for c in upcoming {
                let ClipContent::Media { media, .. } = &c.content else {
                    continue;
                };
                let Some(m) = self.project.media(*media).cloned() else {
                    continue;
                };
                visible.push(c.id);
                let at = Layer {
                    clip: c.clone(),
                    t_rel: Time::ZERO,
                    alpha: 1.0,
                };
                self.request_frame(ctx, c.id, &c, &m, &at, canvas_px, Some(c.start));
            }
        }
        self.fx_overlays = overlays;
        self.playbacks.retain(|id, _| visible.contains(id));
        self.playbacks_next.retain(|id, _| visible.contains(id));
    }

    /// Ein Bild für die Ebene anfordern (Strom beim Abspielen, sonst Einzelbild).
    #[allow(clippy::too_many_arguments)]
    fn request_frame(
        &mut self,
        ctx: &egui::Context,
        id: ClipId,
        c: &Clip,
        m: &MediaRef,
        l: &Layer,
        canvas_px: egui::Vec2,
        prefetch_at: Option<Time>,
    ) {
        let div = self.settings.preview_div.max(1) as f32;
        let fps = self.project.settings.fps;
        let ClipContent::Media { kind, .. } = &c.content else {
            return;
        };
        // Vergrößerte Clips (z. B. Querformat-Gameplay im Hochformat auf 200 %) brauchen
        // entsprechend mehr Pixel, sonst wird ein kleines Bild hochgezogen und unscharf.
        let zoom = c
            .keyframes
            .get(Prop::Scale)
            .iter()
            .map(|k| k.v)
            .fold(c.transform.scale, f32::max)
            .clamp(0.25, 6.0);
        let cap = |v: f32, src: u32| (v as u32).clamp(64, 3840).min(src.max(64));
        let max_w = cap(canvas_px.x / div * zoom, m.width);
        let max_h = cap(canvas_px.y / div * zoom, m.height);
        let (w, h) = cl_media::fit_size(m.width, m.height, max_w, max_h);
        let lut =
            c.fx.lut
                .as_deref()
                .filter(|p| std::path::Path::new(p).is_file())
                .map(cl_fx::escape_path);
        let is_video = *kind == MediaKind::Video;
        // Auch während Übergängen: der vorab geöffnete Strom beginnt genau am Clipanfang
        let streamable = is_video && !c.fx.reverse && c.fx.speed_curve.is_none();
        if self.playing && streamable && (self.play_rate - 1.0).abs() < 1e-6 {
            let graph =
                cl_fx::preview_graph(c, w, h, l.t_rel.as_secs_f64(), Some(fps), lut.clone());
            let key = Self::frame_key(id, c, Time::ZERO, w, h, Time::ZERO, false);
            // Hängender oder vorzeitig beendeter Strom: an der aktuellen Stelle neu öffnen
            // (das letzte Bild bleibt stehen, bis der neue Strom liefert).
            if prefetch_at.is_none()
                && self.play_warmup.is_none()
                && (c.end() - self.playhead).as_secs_f64() > 0.25
                && self
                    .playbacks
                    .get(&id)
                    .is_some_and(|p| p.key == key && p.t0 <= self.playhead && p.stalled())
            {
                if crate::app::perf() {
                    eprintln!("perf: Strom hing bei {:.2} s, neu geöffnet", self.playhead.as_secs_f64());
                }
                self.playbacks.remove(&id);
            }
            let current = self.playbacks.get(&id).is_some_and(|p| p.key == key);
            let pending = self.playbacks_next.get(&id).is_some_and(|p| p.key == key);
            if !current && !pending {
                // Vorab: ab dem Startpunkt des Clips/Elements; sonst ab dem Playhead
                let t0 = prefetch_at.unwrap_or(self.playhead);
                let at = c.source_time(c.start + l.t_rel);
                if let Some(pb) = Playback::open(
                    &self.preview_path(m, h),
                    at,
                    t0,
                    w,
                    h,
                    fps,
                    graph.as_deref(),
                    key,
                ) {
                    self.perf_opens += 1;
                    // Läuft schon ein Strom (z. B. Effekt während der Wiedergabe geändert),
                    // zeigt er weiter Bilder, bis der neue bereit ist – kein Stocken.
                    if self.playbacks.contains_key(&id) && prefetch_at.is_none() {
                        self.playbacks_next.insert(id, pb);
                    } else {
                        self.playbacks.insert(id, pb);
                    }
                }
            }
            if prefetch_at.is_some() {
                return;
            }
            // Neuer Strom bereit? Dann umschalten.
            if let Some(f) = self
                .playbacks_next
                .get_mut(&id)
                .filter(|p| p.key == key)
                .and_then(|p| p.catch_up(self.playhead))
            {
                if let Some(pb) = self.playbacks_next.remove(&id) {
                    self.playbacks.insert(id, pb);
                }
                self.perf_frames += 1;
                self.set_frame(ctx, id, key, &f);
                return;
            }
            if let Some(f) = self
                .playbacks
                .get_mut(&id)
                .and_then(|p| p.catch_up(self.playhead))
            {
                self.perf_frames += 1;
                if id == l.clip.id && !self.play_warmup.is_some() {
                    let now = std::time::Instant::now();
                    if let Some(last) = self.perf_last_frame {
                        self.perf_max_gap = self
                            .perf_max_gap
                            .max(now.duration_since(last).as_secs_f64());
                    }
                    self.perf_last_frame = Some(now);
                }
                self.set_frame(ctx, id, key, &f);
            }
            return;
        }
        if prefetch_at.is_some() {
            return;
        }
        let src = if is_video {
            let span_end = c.source_in + c.source_span() - Time::from_millis(20);
            Time(
                c.source_time(c.start + l.t_rel)
                    .0
                    .clamp(c.source_in.0, span_end.0.max(c.source_in.0)),
            )
            .snap_to_frame(m.fps.unwrap_or(fps))
        } else {
            Time::ZERO
        };
        let animated_fx = c.fx.effects.iter().any(|e| {
            matches!(
                e.id.as_str(),
                "shake"
                    | "zoom-pulse"
                    | "glitch"
                    | "rainbow"
                    | "strobe"
                    | "flash"
                    | "old-film"
                    | "warm-leak"
            )
        });
        let key = Self::frame_key(id, c, src, w, h, l.t_rel, animated_fx);
        if self.requested.get(&id) != Some(&key) {
            self.requested.insert(id, key);
            let graph = cl_fx::preview_graph(c, w, h, l.t_rel.as_secs_f64(), None, lut);
            self.decoder.send(Job::Frame {
                clip: id,
                key,
                path: self.preview_path(m, h),
                at: src,
                w,
                h,
                graph,
            });
        }
    }

    pub fn player(&mut self, ui: &mut egui::Ui) {
        let s = self.project.settings.clone();
        let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
        let controls_h = 34.0;
        let avail = ui.available_size() - vec2(0.0, controls_h);
        let fit_scale = ((avail.x - 16.0) / pw).min((avail.y - 16.0) / ph).max(0.01);
        let (area, area_resp) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());
        self.player_zoom_input(ui, &area_resp, area, fit_scale, pw, ph);
        let scale = fit_scale * self.player_zoom;
        let canvas_size = vec2(pw * scale, ph * scale);
        let canvas = Rect::from_center_size(area.center() + self.player_pan, canvas_size);
        let ppp = ui.ctx().pixels_per_point();
        let layers = self.layers_at(self.playhead);
        self.update_layers(ui.ctx(), &layers, canvas_size * ppp);

        let painter = ui.painter_at(area);
        painter.rect_filled(canvas, 0.0, Color32::BLACK);
        let content = painter.with_clip_rect(canvas.intersect(area));
        let regions = self.draw_layers(&content, canvas, scale, &layers);
        // Ablegen aus dem Asset-Bereich direkt im Player
        if let Some(payload) = area_resp.dnd_release_payload::<DragAsset>() {
            let on = area_resp
                .hover_pos()
                .and_then(|p| regions.iter().rev().find(|r| r.contains(p)).map(|r| r.clip));
            self.drop_asset(&payload, self.playhead, None, on);
        } else if area_resp.dnd_hover_payload::<DragAsset>().is_some() {
            painter.rect_stroke(
                canvas,
                0.0,
                egui::Stroke::new(2.0_f32, theme::ACCENT()),
                egui::StrokeKind::Inside,
            );
        }
        if area_resp.clicked() || area_resp.drag_started() {
            crate::app::release_text_focus(ui.ctx());
        }
        if !self.player_pick(ui, &area_resp, &regions) {
            self.player_interact(ui, &area_resp, canvas, scale, &regions);
        }
        if self.voiceover.recorder.is_some() {
            painter.circle_filled(canvas.left_top() + vec2(14.0, 14.0), 5.0, theme::DANGER);
            painter.text(
                canvas.left_top() + vec2(24.0, 14.0),
                egui::Align2::LEFT_CENTER,
                crate::tr("REC"),
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        if self.project.duration() == Time::ZERO {
            painter.text(
                canvas.center() - vec2(0.0, 10.0),
                egui::Align2::CENTER_CENTER,
                icon::FILM_SLATE,
                egui::FontId::proportional(28.0),
                theme::TEXT_FAINT(),
            );
            painter.text(
                canvas.center() + vec2(0.0, 18.0),
                egui::Align2::CENTER_CENTER,
                crate::tr(self.t("drop-hint")),
                egui::FontId::proportional(12.0),
                theme::TEXT_DIM(),
            );
        }
        self.player_controls(ui, controls_h);
    }

    /// Alle Ebenen in `canvas` zeichnen (Player und externes Vorschaufenster).
    fn draw_layers(
        &self,
        content: &egui::Painter,
        canvas: Rect,
        scale: f32,
        layers: &[Layer],
    ) -> Vec<Region> {
        let s = &self.project.settings;
        let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
        let mut regions: Vec<Region> = Vec::new();
        for l in layers {
            let c = &l.clip;
            let mut ev = c.eval_at(l.t_rel, pw, ph);
            if let Some((id, lt)) = self.gizmo.live {
                if id == c.id {
                    ev.x = lt.x;
                    ev.y = lt.y;
                    ev.scale = lt.scale;
                    ev.rotation_deg = lt.rotation_deg;
                }
            }
            let tr = Transform {
                x: ev.x,
                y: ev.y,
                scale: ev.scale,
                rotation_deg: ev.rotation_deg,
            };
            let alpha = (ev.opacity * l.alpha).clamp(0.0, 1.0);
            let center = canvas.center() + vec2(ev.x, ev.y) * scale;
            let angle = ev.rotation_deg.to_radians();
            match &c.content {
                ClipContent::Media { .. } => {
                    let Some((_, tex)) = self.frames.get(&c.id) else {
                        continue;
                    };
                    let ts = tex.size_vec2();
                    let fit = (canvas.width() / ts.x).min(canvas.height() / ts.y) * ev.scale;
                    let region = Region {
                        clip: c.id,
                        center,
                        size: ts * fit,
                        angle,
                        tr,
                    };
                    let mut mesh = egui::Mesh::with_texture(tex.id());
                    let col = Color32::from_white_alpha((alpha * 255.0) as u8);
                    for (sx, sy, u, v) in [
                        (-1.0, -1.0, 0.0, 0.0),
                        (1.0, -1.0, 1.0, 0.0),
                        (1.0, 1.0, 1.0, 1.0),
                        (-1.0, 1.0, 0.0, 1.0),
                    ] {
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: region.corner(sx, sy),
                            uv: pos2(u, v),
                            color: col,
                        });
                    }
                    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                    content.add(egui::Shape::mesh(mesh));
                    // Maskierte Effekt-/Filter-Elemente darüber: zweite Fassung nur in der Maske
                    let corners = [
                        region.corner(-1.0, -1.0),
                        region.corner(1.0, -1.0),
                        region.corner(1.0, 1.0),
                        region.corner(-1.0, 1.0),
                    ];
                    for (base, did, mk, k) in &self.fx_overlays {
                        if *base != c.id || *k <= 0.001 {
                            continue;
                        }
                        let Some((_, fx_tex)) = self.frames.get(did) else {
                            continue;
                        };
                        let alpha = alpha * k;
                        let Some(mk) = mk else {
                            crate::fx_mask::draw_textured(
                                content,
                                fx_tex.id(),
                                corners,
                                &corners,
                                alpha,
                            );
                            continue;
                        };
                        let (polys, inverted) = crate::fx_mask::polygons(mk, canvas);
                        if inverted {
                            // alles außer der Form: Effekt ganz, Original in der Form darüber
                            crate::fx_mask::draw_textured(
                                content,
                                fx_tex.id(),
                                corners,
                                &corners,
                                alpha,
                            );
                            for poly in &polys {
                                crate::fx_mask::draw_textured(
                                    content,
                                    tex.id(),
                                    corners,
                                    poly,
                                    alpha,
                                );
                            }
                        } else {
                            for poly in &polys {
                                crate::fx_mask::draw_textured(
                                    content,
                                    fx_tex.id(),
                                    corners,
                                    poly,
                                    alpha,
                                );
                            }
                        }
                    }
                    regions.push(region);
                }
                ClipContent::Effect(_) | ClipContent::Filter(_) => {
                    // Maske des ausgewählten Elements als Umriss zeigen
                    if self.selected == Some(c.id) {
                        if let Some(mk) = &c.fx.mask {
                            let (polys, _) = crate::fx_mask::polygons(mk, canvas);
                            for poly in polys {
                                content.add(egui::Shape::closed_line(
                                    poly,
                                    egui::Stroke::new(1.5_f32, theme::ACCENT()),
                                ));
                            }
                        }
                    }
                }
                ClipContent::Text(st) => {
                    let size = self.draw_text(
                        content,
                        st,
                        center,
                        ev.scale * scale,
                        angle,
                        alpha,
                        canvas.width(),
                    );
                    regions.push(Region {
                        clip: c.id,
                        center,
                        size,
                        angle,
                        tr,
                    });
                }
            }
        }
        regions
    }

    /// Vorschau in eigenem Fenster (z. B. auf dem zweiten Bildschirm, F11 = Vollbild).
    pub fn external_preview(&mut self, ctx: &egui::Context) {
        let init = egui::Id::new("ext-preview-init");
        if !self.ext_preview {
            ctx.data_mut(|d| d.remove::<u8>(init));
            return;
        }
        let vid = egui::ViewportId::from_hash_of("cutline-preview");
        // In den ersten Bildern nach dem Öffnen ausdrücklich nach vorn holen: eine Angabe im
        // Builder wirkt unter Windows nicht, das Fenster läge hinter dem Hauptfenster
        let n = ctx.data(|d| d.get_temp::<u8>(init)).unwrap_or(0);
        if n < 4 {
            ctx.data_mut(|d| d.insert_temp(init, n + 1));
            ctx.send_viewport_cmd_to(vid, egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd_to(vid, egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(crate::tr("Cutline – Vorschau"))
            .with_inner_size([960.0, 540.0])
            .with_min_inner_size([320.0, 180.0]);
        // Erst auf dem Bildschirm des Hauptfensters öffnen; von dort auf den zweiten ziehen
        // nur beim Öffnen: eine dauerhaft gesetzte Position hält das Fenster fest
        if let Some(r) = ctx.input(|i| i.viewport().outer_rect).filter(|_| n < 4) {
            builder = builder.with_position(r.min + vec2(80.0, 80.0));
        }
        let mut ext_full = self.ext_full;
        let mut toggle_full = false;
        ctx.show_viewport_immediate(vid, builder, |ctx, _| {
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(Color32::BLACK))
                .show(ctx, |ui| {
                    let s = self.project.settings.clone();
                    let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
                    let area = ui.max_rect();
                    let scale = (area.width() / pw).min(area.height() / ph);
                    let canvas =
                        Rect::from_center_size(area.center(), vec2(pw * scale, ph * scale));
                    let layers = self.layers_at(self.playhead);
                    let painter = ui.painter_at(area);
                    self.draw_layers(&painter.with_clip_rect(canvas), canvas, scale, &layers);
                    let r = ui.interact(area, ui.id().with("ext"), egui::Sense::click());
                    if r.double_clicked() {
                        toggle_full = true;
                    }
                });
            if ctx.input(|i| i.key_pressed(egui::Key::F11) || (ext_full && i.key_pressed(egui::Key::Escape))) {
                toggle_full = true;
            }
            // Vollbild als randloses, maximiertes Fenster: echtes Vollbild einer zweiten
            // Ansicht lässt egui/winit unter Windows hängen.
            if toggle_full {
                let full = !ext_full;
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!full));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(full));
                ext_full = full;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                self.toggle_play();
            }
            if ctx.input(|i| i.viewport().close_requested()) {
                self.ext_preview = false;
                ext_full = false;
            }
        });
        self.ext_full = ext_full;
        if self.playing {
            ctx.request_repaint_of(vid);
        }
    }

    /// Text wie im Export: Fläche, Kontur, Schatten, Ausrichtung, Zeilenabstand.
    /// Gibt die Größe des Textblocks (Bildschirm) zurück.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &self,
        p: &egui::Painter,
        st: &TextStyle,
        center: egui::Pos2,
        k: f32,
        angle: f32,
        alpha: f32,
        canvas_w: f32,
    ) -> egui::Vec2 {
        let txt = if st.uppercase {
            st.text.to_uppercase()
        } else {
            st.text.clone()
        };
        let px = (st.size * k).max(2.0);
        let font = egui::FontId::new(px, self.font_family(st.font_family.as_deref()));
        let wrap = if st.auto_wrap {
            canvas_w * 0.9
        } else {
            f32::INFINITY
        };
        let a = |c: [u8; 4]| {
            Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (c[3] as f32 * alpha) as u8)
        };
        let job = |col: Color32| {
            let mut j = egui::text::LayoutJob::default();
            j.append(
                &txt,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: col,
                    line_height: Some(px * 1.25 * st.line_spacing.max(0.5)),
                    italics: st.italic,
                    ..Default::default()
                },
            );
            j.wrap.max_width = wrap;
            j.halign = match st.align {
                x if x < 0 => egui::Align::LEFT,
                x if x > 0 => egui::Align::RIGHT,
                _ => egui::Align::Center,
            };
            j
        };
        let galley = p.layout_job(job(a(st.color)));
        let size = galley.size();
        let rot = |v: egui::Vec2| {
            let (s, c) = angle.sin_cos();
            vec2(v.x * c - v.y * s, v.x * s + v.y * c)
        };
        // Ankerpunkt für TextShape: links oben der Galley (bei Center-Ausrichtung liegt
        // die Galley um x = 0 herum)
        let x_off = match st.align {
            x if x < 0 => -size.x / 2.0,
            x if x > 0 => size.x / 2.0,
            _ => 0.0,
        };
        let anchor = center + rot(vec2(x_off, -size.y / 2.0));
        if let Some(bg) = st.background {
            let pad = (px * 0.25).max(3.0);
            let pts: Vec<egui::Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .iter()
                .map(|(sx, sy)| {
                    center
                        + rot(vec2(
                            sx * (size.x / 2.0 + pad),
                            sy * (size.y / 2.0 + pad * 0.6),
                        ))
                })
                .collect();
            p.add(egui::Shape::convex_polygon(pts, a(bg), egui::Stroke::NONE));
        }
        if st.shadow {
            let sg = p.layout_job(job(Color32::from_black_alpha((150.0 * alpha) as u8)));
            let off = rot(vec2(1.0, 1.0) * (px / 22.0).clamp(1.0, 4.0));
            p.add(egui::epaint::TextShape::new(anchor + off, sg, Color32::BLACK).with_angle(angle));
        }
        if let Some((w, col)) = st.outline {
            let og = p.layout_job(job(a(col)));
            let r = (w * k).clamp(0.6, 12.0);
            for i in 0..12 {
                let ang = i as f32 / 12.0 * std::f32::consts::TAU;
                p.add(
                    egui::epaint::TextShape::new(
                        anchor + rot(vec2(ang.cos(), ang.sin()) * r),
                        og.clone(),
                        Color32::BLACK,
                    )
                    .with_angle(angle),
                );
            }
        }
        p.add(egui::epaint::TextShape::new(anchor, galley, a(st.color)).with_angle(angle));
        if st.bold {
            // Fettdruck nachahmen, wenn die Schrift keinen eigenen Schnitt hat
            let g2 = p.layout_job(job(a(st.color)));
            p.add(
                egui::epaint::TextShape::new(anchor + rot(vec2(px * 0.03, 0.0)), g2, a(st.color))
                    .with_angle(angle),
            );
        }
        size
    }

    /// Steuerleiste: Zeit links, Play mittig, Chips rechts.
    /// Pegelanzeige (Spitze mit Abfall und Haltelinie), rot ab −1 dBFS
    fn level_meter(&mut self, ui: &mut egui::Ui) {
        let (l, r) = self
            .audio
            .as_ref()
            .map(|a| a.take_peaks())
            .unwrap_or((0.0, 0.0));
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        for (i, v) in [l, r].into_iter().enumerate() {
            let cur = &mut self.meter[i];
            cur.0 = v.max(cur.0 - dt * 1.2);
            if v >= cur.1 {
                cur.1 = v;
                cur.2 = 1.0;
            } else {
                cur.2 -= dt;
                if cur.2 <= 0.0 {
                    cur.1 = (cur.1 - dt * 0.6).max(cur.0);
                }
            }
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 22.0), egui::Sense::hover());
        let db = |v: f32| if v <= 1e-5 { -60.0 } else { 20.0 * v.log10() };
        let y_of = |v: f32| rect.bottom() - rect.height() * ((db(v) + 60.0) / 60.0).clamp(0.0, 1.0);
        let p = ui.painter();
        for i in 0..2 {
            let x0 = rect.left() + i as f32 * 7.0;
            let bar = Rect::from_min_max(pos2(x0, rect.top()), pos2(x0 + 5.0, rect.bottom()));
            p.rect_filled(bar, 1.0, theme::BG_HOVER());
            let (v, hold, _) = self.meter[i];
            let col = if db(v) > -1.0 {
                theme::DANGER
            } else if db(v) > -9.0 {
                Color32::from_rgb(0xE8, 0xC0, 0x50)
            } else {
                Color32::from_rgb(0x5C, 0xC8, 0x7A)
            };
            p.rect_filled(
                Rect::from_min_max(pos2(bar.left(), y_of(v)), bar.max),
                1.0,
                col,
            );
            if hold > 1e-4 {
                let y = y_of(hold);
                p.line_segment(
                    [pos2(bar.left(), y), pos2(bar.right(), y)],
                    egui::Stroke::new(
                        1.0_f32,
                        if db(hold) > -1.0 {
                            theme::DANGER
                        } else {
                            theme::TEXT_DIM()
                        },
                    ),
                );
            }
        }
        resp.on_hover_text(crate::tr(format!(
            "Pegel L {:.0} dB · R {:.0} dB",
            db(self.meter[0].1),
            db(self.meter[1].1)
        )));
        if self.playing {
            ui.ctx().request_repaint_after(self.play_repaint_delay());
        }
    }

    fn player_controls(&mut self, ui: &mut egui::Ui, h: f32) {
        let fps = self.project.settings.fps;
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), egui::Sense::hover());
        let left = Rect::from_min_max(bar.min, pos2(bar.center().x - 24.0, bar.max.y));
        let right = Rect::from_min_max(pos2(bar.center().x + 24.0, bar.min.y), bar.max);
        ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(left)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.add_space(4.0);
                self.level_meter(ui);
                ui.label(
                    RichText::new(crate::tr(self.playhead.timecode(fps)))
                        .color(theme::ACCENT())
                        .monospace(),
                );
                let total = format!("/ {}", self.project.duration().timecode(fps));
                let need = ui
                    .painter()
                    .layout_no_wrap(
                        total.clone(),
                        egui::TextStyle::Monospace.resolve(ui.style()),
                        theme::TEXT_DIM(),
                    )
                    .size()
                    .x
                    + 8.0;
                if ui.available_width() > need {
                    ui.label(
                        RichText::new(crate::tr(total))
                            .color(theme::TEXT_DIM())
                            .monospace(),
                    );
                }
            },
        );
        let glyph = if self.playing {
            icon::PAUSE
        } else {
            icon::PLAY
        };
        let pr = ui.put(
            Rect::from_center_size(bar.center(), vec2(36.0, 28.0)),
            egui::Button::new(RichText::new(crate::tr(glyph)).size(19.0)).frame(false),
        );
        if pr
            .on_hover_text(crate::tr(format!(
                "{}  (Leertaste)",
                if self.playing {
                    self.t("pause")
                } else {
                    self.t("play")
                }
            )))
            .clicked()
        {
            self.toggle_play();
        }
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            let compact = ui.available_width() < 200.0;
            let (g, tip) = if self.player_max { (icon::CORNERS_IN, "Normale Ansicht (F)") } else { (icon::CORNERS_OUT, "Großer Player (F)") };
            if theme::icon_button(ui, g, tip, true, self.player_max).clicked() {
                self.player_max = !self.player_max;
            }
            if theme::icon_button(ui, icon::MONITOR_PLAY, "Vorschau in eigenem Fenster (zweiter Bildschirm, Doppelklick oder F11 = Vollbild)", true, self.ext_preview).clicked() {
                self.ext_preview = !self.ext_preview;
            }
            self.player_zoom_chip(ui, compact);
            let q = match self.settings.preview_div {
                1 => "Voll",
                2 => "½",
                _ => "¼",
            };
            let qlabel = if compact { icon::MONITOR.to_string() } else { format!("{} {q}", icon::MONITOR) };
            if theme::chip(ui, &qlabel).on_hover_text(crate::tr(format!("Vorschauqualität: {q}"))).clicked() {
                self.settings.preview_div = match self.settings.preview_div {
                    1 => 2,
                    2 => 4,
                    _ => 1,
                };
                self.invalidate_preview();
            }
            let aspect = AspectPreset::PRESETS.iter().find(|(_, p)| *p == self.project.settings.aspect).map(|(n, _)| *n).unwrap_or("Eigenes");
            let alabel = if compact { icon::FRAME_CORNERS.to_string() } else { format!("{} {aspect}", icon::FRAME_CORNERS) };
            let resp = theme::chip(ui, &alabel).on_hover_text(crate::tr(format!("{}: {aspect}", self.t("aspect"))));
            let popup = ui.make_persistent_id("aspect-popup");
            if resp.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup));
            }
            egui::popup_above_or_below_widget(ui, popup, &resp, egui::AboveOrBelow::Above, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                ui.set_min_width(120.0);
                for (n, p) in AspectPreset::PRESETS {
                    if ui.selectable_label(self.project.settings.aspect == p, crate::tr(n)).clicked() {
                        self.set_aspect(p);
                    }
                }
            });
        });
    }

    pub fn set_aspect(&mut self, p: AspectPreset) {
        let mut s = self.project.settings.clone();
        if p == AspectPreset::Original {
            if let Some(m) = self.project.media.iter().find(|m| m.width > 0) {
                s.width = m.width;
                s.height = m.height;
            }
            s.aspect = p;
        } else {
            s.apply_aspect(p);
        }
        if s.platform
            .as_deref()
            .and_then(crate::export_ui::platform)
            .is_some_and(|pl| !pl.fits(&s))
        {
            s.platform = None;
        }
        if s != self.project.settings {
            self.apply(EditOp::SetSettings { settings: s });
        }
    }
}
