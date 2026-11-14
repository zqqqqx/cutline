//! Rechte Karte: Eigenschaften des ausgewählten Clips (Reiter je nach Clip-Art),
//! Übergang, Mehrfachauswahl oder – ohne Auswahl – Projekt und Verlauf.
//! Änderungen laufen live über `App::live` und landen beim Loslassen als ein Undo-Schritt.

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke};

use crate::app::{App, ClipTab, InspectorTab, LeftTab};
use crate::theme::{self, icon};
use crate::widgets::{self, Edit, Kf, Slider};

fn tabs_for(c: &Clip) -> Vec<(ClipTab, &'static str)> {
    match &c.content {
        ClipContent::Effect(_) | ClipContent::Filter(_) => {
            vec![(ClipTab::Effects, "Einfach"), (ClipTab::Basic, "Maske")]
        }
        ClipContent::Text(_) => vec![
            (ClipTab::Text, "Text"),
            (ClipTab::Basic, "Basis"),
            (ClipTab::Animation, "Animation"),
        ],
        ClipContent::Media {
            kind: MediaKind::Audio,
            ..
        } => vec![(ClipTab::Audio, "Audio"), (ClipTab::Speed, "Tempo")],
        ClipContent::Media {
            kind: MediaKind::Image,
            ..
        } => vec![
            (ClipTab::Basic, "Basis"),
            (ClipTab::Effects, "Effekte"),
            (ClipTab::Adjust, "Anpassen"),
            (ClipTab::Animation, "Animation"),
        ],
        ClipContent::Media { .. } => vec![
            (ClipTab::Basic, "Basis"),
            (ClipTab::Effects, "Effekte"),
            (ClipTab::Adjust, "Anpassen"),
            (ClipTab::Animation, "Animation"),
            (ClipTab::Speed, "Tempo"),
            (ClipTab::Audio, "Audio"),
        ],
    }
}

/// Masken-Auswahl und -Regler (Clip oder Effekt-Element).
fn mask_ui(ui: &mut egui::Ui, mask: &mut Option<Mask>) -> Edit {
    let mut e = Edit::default();
    let shapes = [
        None,
        Some(MaskShape::Linear),
        Some(MaskShape::Mirror),
        Some(MaskShape::Circle),
        Some(MaskShape::Rect),
    ];
    let mut i = shapes
        .iter()
        .position(|s| *s == mask.map(|m| m.shape))
        .unwrap_or(0);
    if widgets::segmented(
        ui,
        &["Keine", "Linear", "Spiegel", "Kreis", "Rechteck"],
        &mut i,
    ) {
        *mask = shapes[i].map(|s| {
            let mut m = mask.unwrap_or(Mask::new(s));
            m.shape = s;
            m
        });
        e.commit = true;
    }
    if let Some(m) = mask.as_mut() {
        ui.add_space(4.0);
        let mut r = Edit::default();
        if matches!(m.shape, MaskShape::Circle | MaskShape::Rect) {
            r |= theme::row(ui, "Breite", |ui| {
                Slider::new(&mut m.w, 0.02..=2.0)
                    .percent()
                    .default(0.6)
                    .show(ui)
            });
        }
        if m.shape != MaskShape::Linear {
            r |= theme::row(ui, "Höhe", |ui| {
                Slider::new(&mut m.h, 0.02..=2.0)
                    .percent()
                    .default(0.6)
                    .show(ui)
            });
        }
        r |= theme::row(ui, "Mitte X", |ui| {
            Slider::new(&mut m.cx, 0.0..=1.0)
                .percent()
                .default(0.5)
                .show(ui)
        });
        r |= theme::row(ui, "Mitte Y", |ui| {
            Slider::new(&mut m.cy, 0.0..=1.0)
                .percent()
                .default(0.5)
                .show(ui)
        });
        r |= theme::row(ui, "Drehen", |ui| {
            Slider::new(&mut m.rotation_deg, -180.0..=180.0)
                .suffix("°")
                .default(0.0)
                .show(ui)
        });
        r |= theme::row(ui, "Weiche Kante", |ui| {
            Slider::new(&mut m.feather, 0.0..=0.5)
                .percent()
                .default(0.05)
                .show(ui)
        });
        if widgets::toggle_row(ui, "Umkehren", &mut m.invert) {
            r.commit = true;
        }
        e |= r;
    }
    e
}

/// Reiterleiste mit Unterstrich.
fn tab_bar<T: Copy + PartialEq>(ui: &mut egui::Ui, tabs: &[(T, &str)], cur: &mut T) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    let font = egui::FontId::proportional(11.5);
    let widths: Vec<f32> = tabs
        .iter()
        .map(|(_, l)| {
            ui.painter()
                .layout_no_wrap(l.to_string(), font.clone(), theme::TEXT())
                .size()
                .x
                + 14.0
        })
        .collect();
    let total: f32 = widths.iter().sum();
    let k = if total > w { w / total } else { 1.0 };
    let mut x = rect.left();
    for (i, (t, l)) in tabs.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(widths[i] * k, rect.height()));
        let resp = ui.interact(r, ui.id().with(("itab", *l)), Sense::click());
        let active = *cur == *t;
        let col = if active {
            theme::ACCENT()
        } else if resp.hovered() {
            theme::TEXT()
        } else {
            theme::TEXT_DIM()
        };
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            crate::tr(*l),
            font.clone(),
            col,
        );
        if active {
            ui.painter().line_segment(
                [
                    pos2(r.left() + 6.0, r.bottom() - 1.0),
                    pos2(r.right() - 6.0, r.bottom() - 1.0),
                ],
                Stroke::new(2.0_f32, theme::ACCENT()),
            );
        }
        if resp.clicked() {
            *cur = *t;
        }
        x += widths[i] * k;
    }
    let r = ui.max_rect();
    ui.painter().line_segment(
        [
            pos2(r.left() - 10.0, rect.bottom()),
            pos2(r.right() + 10.0, rect.bottom()),
        ],
        Stroke::new(1.0_f32, theme::BORDER()),
    );
    ui.add_space(8.0);
}

impl App {
    pub fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        if let Some(t) = self.selected_transition {
            if let Some((_, c)) = self
                .project
                .find_clip(t)
                .map(|(a, b)| (a.clone(), b.clone()))
            {
                if let Some(tr) = c.transition_out.clone() {
                    self.transition_inspector(ui, &c, &tr);
                    return;
                }
            }
        }
        let sel = self.selection();
        if sel.len() > 1 {
            self.multi_inspector(ui, sel.len());
            if self.selected.is_none() {
                self.selected = sel.last().copied();
            }
        }
        match self.selected.and_then(|s| self.clip_view(s)) {
            Some(clip) => self.clip_inspector(ui, clip),
            None => {
                let mut t = self.inspector_tab;
                tab_bar(
                    ui,
                    &[
                        (InspectorTab::Project, "Projekt"),
                        (InspectorTab::History, "Verlauf"),
                    ],
                    &mut t,
                );
                self.inspector_tab = t;
                egui::ScrollArea::vertical()
                    .id_salt("insp-p")
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.inspector_tab {
                        InspectorTab::Project => self.project_inspector(ui),
                        InspectorTab::History => self.history_inspector(ui),
                    });
            }
        }
    }

    fn multi_inspector(&mut self, ui: &mut egui::Ui, n: usize) {
        theme::card_header(ui, &format!("{n} Clips ausgewählt"), |ui| {
            if theme::icon_button(ui, icon::X, "Auswahl aufheben (Esc)", true, false).clicked() {
                self.select_only(None);
            }
        });
        ui.horizontal(|ui| {
            if theme::icon_button(ui, icon::COPY, "Kopieren", true, false).clicked() {
                self.copy_selection();
            }
            if theme::icon_button(ui, icon::COPY_SIMPLE, "Duplizieren", true, false).clicked() {
                self.duplicate_selection();
            }
            if theme::icon_button(
                ui,
                icon::EYE_SLASH,
                "Deaktivieren / Aktivieren",
                true,
                false,
            )
            .clicked()
            {
                self.toggle_disabled();
            }
            if theme::icon_button(ui, icon::TRASH, "Löschen", true, false).clicked() {
                self.delete_selected();
            }
        });
        ui.label(RichText::new(crate::tr("Gilt für alle ausgewählten Clips")).small().color(theme::TEXT_DIM()));
        ui.add_space(6.0);
    }

    fn transition_inspector(&mut self, ui: &mut egui::Ui, c: &Clip, tr: &TransitionRef) {
        theme::card_header(ui, "Übergang", |ui| {
            if theme::icon_button(ui, icon::X, "Schließen", true, false).clicked() {
                self.selected_transition = None;
            }
        });
        theme::row(ui, "Art", |ui| {
            ui.label(RichText::new(crate::tr(cl_fx::transition_name(&tr.id))).strong())
        });
        let next_len = self
            .next_adjacent(c.id)
            .and_then(|n| self.project.find_clip(n))
            .map(|(_, n)| n.duration.as_secs_f64())
            .unwrap_or(1.0);
        let max = (c.duration.as_secs_f64().min(next_len)).clamp(0.1, 5.0) as f32;
        let mut d = tr.duration.as_secs_f64() as f32;
        let e = theme::row(ui, "Dauer", |ui| {
            Slider::new(&mut d, 0.1..=max)
                .suffix(" s")
                .decimals(1)
                .default(self.settings.transition_duration)
                .show(ui)
        });
        if e.commit {
            self.apply(EditOp::SetTransition {
                clip: c.id,
                transition: Some(TransitionRef {
                    id: tr.id.clone(),
                    duration: Time::from_secs_f64(d as f64),
                }),
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add(theme::secondary(format!("{}  Übergänge", icon::INTERSECT)))
                .clicked()
            {
                self.tab = LeftTab::Transitions;
            }
            if ui
                .add(theme::secondary(format!("{}  Entfernen", icon::TRASH)))
                .clicked()
            {
                self.set_transition(c.id, None);
            }
        });
    }

    fn kf_state(&self, c: &Clip, p: Prop) -> Kf {
        if c.keyframes.get(p).is_empty() {
            Kf::None
        } else if c.keyframes.has_at(p, self.rel_time(c)) {
            Kf::Here
        } else {
            Kf::Elsewhere
        }
    }

    /// Regler für eine keyframefähige Eigenschaft.
    #[allow(clippy::too_many_arguments)]
    fn prop_slider(
        &self,
        ui: &mut egui::Ui,
        c: &mut Clip,
        label: &str,
        p: Prop,
        range: std::ops::RangeInclusive<f32>,
        def: f32,
        fmt: (&str, usize, f32),
        kf: &mut Option<Vec<Prop>>,
    ) -> Edit {
        let (w, h) = (
            self.project.settings.width as f32,
            self.project.settings.height as f32,
        );
        let ev = c.eval_at(self.rel_time(c), w, h);
        let mut v = if c.keyframes.get(p).is_empty() {
            match p {
                Prop::X => c.transform.x,
                Prop::Y => c.transform.y,
                Prop::Scale => c.transform.scale,
                Prop::Rotation => c.transform.rotation_deg,
                Prop::Opacity => c.opacity,
                Prop::Volume => c.volume_db,
                Prop::Strength => c.strength_at(Time::ZERO),
            }
        } else {
            match p {
                Prop::X => c.keyframes.eval(p, self.rel_time(c)).unwrap_or(ev.x),
                Prop::Y => c.keyframes.eval(p, self.rel_time(c)).unwrap_or(ev.y),
                _ => c.keyframes.eval(p, self.rel_time(c)).unwrap_or(0.0),
            }
        };
        let state = self.kf_state(c, p);
        let e = theme::row(ui, label, |ui| {
            let r = ui.allocate_ui(
                vec2(
                    ui.available_width() - 18.0 - ui.spacing().item_spacing.x - 1.0,
                    20.0,
                ),
                |ui| {
                    let mut s = Slider::new(&mut v, range).default(def).decimals(fmt.1);
                    s = if fmt.2 == 100.0 {
                        s.percent()
                    } else {
                        s.suffix(fmt.0)
                    };
                    s.show(ui)
                },
            );
            if widgets::kf_button(ui, state).clicked() {
                *kf = Some(vec![p]);
            }
            r.inner
        });
        if e.changed {
            self.set_prop(c, p, v);
        }
        e
    }

    fn clip_inspector(&mut self, ui: &mut egui::Ui, mut c: Clip) {
        let tabs = tabs_for(&c);
        if !tabs.iter().any(|t| t.0 == self.clip_tab) {
            self.clip_tab = tabs[0].0;
        }
        self.preset_bar(ui, &c);
        let mut tab = self.clip_tab;
        tab_bar(ui, &tabs, &mut tab);
        if tab != self.clip_tab {
            self.commit_live();
            self.clip_tab = tab;
        }
        let orig = c.clone();
        let mut e = Edit::default();
        let mut kf: Option<Vec<Prop>> = None;
        egui::ScrollArea::vertical()
            .id_salt(("insp", c.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                match self.clip_tab {
                    ClipTab::Basic
                        if matches!(c.content, ClipContent::Effect(_) | ClipContent::Filter(_)) =>
                    {
                        theme::section(ui, "Maske");
                        e |= mask_ui(ui, &mut c.fx.mask);
                    }
                    ClipTab::Basic => {
                        self.basic_tab(ui, &mut c, &mut e, &mut kf);
                        // Bilder und Texte haben keinen Tempo-Reiter: Zeit hier
                        if !matches!(
                            c.content,
                            ClipContent::Media {
                                kind: MediaKind::Video | MediaKind::Audio,
                                ..
                            }
                        ) {
                            let now = c.clone();
                            self.timing_section(ui, &now);
                        }
                    }
                    ClipTab::Text => self.text_props(ui, &mut c, &mut e),
                    ClipTab::Effects
                        if matches!(c.content, ClipContent::Effect(_) | ClipContent::Filter(_)) =>
                    {
                        self.fx_clip_props(ui, &mut c, &mut e, &mut kf);
                        let now = c.clone();
                        self.timing_section(ui, &now);
                    }
                    ClipTab::Effects => self.effects_props(ui, &mut c, &mut e),
                    ClipTab::Adjust => self.adjust_props(ui, &mut c, &mut e),
                    ClipTab::Animation => self.anim_props(ui, &mut c, &mut e),
                    ClipTab::Speed => {
                        self.speed_props(ui, &mut c, &mut e);
                        let now = c.clone();
                        self.timing_section(ui, &now);
                    }
                    ClipTab::Audio => self.audio_props(ui, &mut c, &mut e, &mut kf),
                }
                if matches!(self.clip_tab, ClipTab::Basic | ClipTab::Audio | ClipTab::Effects | ClipTab::Animation) {
                    e |= self.curve_ui(ui, &mut c);
                }
                ui.add_space(10.0);
                let fps = self.project.settings.fps;
                ui.label(
                    RichText::new(format!(
                        "{} · {:.2} s · {}",
                        crate::tr(match &c.content {
                            ClipContent::Effect(r) | ClipContent::Filter(r) => cl_fx::fx_name(&r.id),
                            _ => c.label(&self.project),
                        }),
                        c.duration.as_secs_f64(),
                        crate::tr(format!("ab {}", c.start.timecode(fps)))
                    ))
                    .small()
                    .color(theme::TEXT_FAINT()),
                );
            });
        if let Some(props) = kf {
            self.live = Some(c);
            self.toggle_keyframe(orig.id, &props);
            return;
        }
        if e.changed || e.commit {
            if c != orig || self.live.is_some() {
                self.live = Some(c);
            }
        }
        if e.commit {
            self.commit_live();
        }
    }

    /// Effekt- oder Filter-Element auf einer eigenen Spur: Art und Stärke.
    fn fx_clip_props(
        &mut self,
        ui: &mut egui::Ui,
        c: &mut Clip,
        e: &mut Edit,
        kf: &mut Option<Vec<Prop>>,
    ) {
        let id = match &c.content {
            ClipContent::Effect(r) | ClipContent::Filter(r) => r.id.clone(),
            _ => return,
        };
        theme::section(ui, "Einzelheiten");
        theme::row(ui, "Name", |ui| {
            ui.label(RichText::new(crate::tr(cl_fx::fx_name(&id))).strong())
        });
        *e |= self.prop_slider(
            ui,
            c,
            "Stärke",
            Prop::Strength,
            0.0..=1.0,
            1.0,
            ("", 0, 100.0),
            kf,
        );
    }

    fn basic_tab(
        &mut self,
        ui: &mut egui::Ui,
        c: &mut Clip,
        e: &mut Edit,
        kf: &mut Option<Vec<Prop>>,
    ) {
        let visual = c.kind() == TrackKind::Video || matches!(c.content, ClipContent::Text(_));
        if !visual {
            return;
        }
        theme::section(ui, "Transformieren");
        *e |= self.prop_slider(
            ui,
            c,
            "Skalierung",
            Prop::Scale,
            0.05..=5.0,
            1.0,
            ("", 0, 100.0),
            kf,
        );
        let (w, h) = (
            self.project.settings.width as f32,
            self.project.settings.height as f32,
        );
        *e |= self.prop_slider(ui, c, "Position X", Prop::X, -w..=w, 0.0, ("", 0, 1.0), kf);
        *e |= self.prop_slider(ui, c, "Position Y", Prop::Y, -h..=h, 0.0, ("", 0, 1.0), kf);
        *e |= self.prop_slider(
            ui,
            c,
            "Drehen",
            Prop::Rotation,
            -180.0..=180.0,
            0.0,
            ("°", 0, 1.0),
            kf,
        );
        ui.horizontal(|ui| {
            ui.add_space(theme::LABEL_W + 6.0);
            if ui
                .add(theme::ghost(format!(
                    "{}  Zurücksetzen",
                    icon::ARROW_COUNTER_CLOCKWISE
                )))
                .clicked()
            {
                c.transform = Transform::default();
                for p in [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation] {
                    c.keyframes.0.remove(&p);
                }
                e.commit = true;
            }
            let any = [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation]
                .iter()
                .any(|p| !c.keyframes.get(*p).is_empty());
            if ui
                .add(theme::ghost(format!(
                    "{}  Keyframe alle",
                    if any { "◆" } else { "◇" }
                )))
                .on_hover_text(crate::tr(
                    "Position, Größe und Drehung am Playhead festhalten",
                ))
                .clicked()
            {
                *kf = Some(vec![Prop::X, Prop::Y, Prop::Scale, Prop::Rotation]);
            }
        });
        theme::section(ui, "Mischen");
        *e |= self.prop_slider(
            ui,
            c,
            "Deckkraft",
            Prop::Opacity,
            0.0..=1.0,
            1.0,
            ("", 0, 100.0),
            kf,
        );
        if matches!(c.content, ClipContent::Media { .. }) {
            theme::row(ui, "Spiegeln", |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new(icon::FLIP_HORIZONTAL).size(14.0))
                            .selected(c.fx.flip_h),
                    )
                    .on_hover_text(crate::tr("Horizontal"))
                    .clicked()
                {
                    c.fx.flip_h = !c.fx.flip_h;
                    e.commit = true;
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new(icon::FLIP_VERTICAL).size(14.0))
                            .selected(c.fx.flip_v),
                    )
                    .on_hover_text(crate::tr("Vertikal"))
                    .clicked()
                {
                    c.fx.flip_v = !c.fx.flip_v;
                    e.commit = true;
                }
            });
            theme::section(ui, "Maske");
            *e |= mask_ui(ui, &mut c.fx.mask);
        }
        self.pro_visual(ui, c, e);
        theme::section(ui, "Clip");
        let mut dis = c.disabled;
        if widgets::toggle_row(ui, "Deaktiviert (V)", &mut dis) {
            c.disabled = dis;
            e.commit = true;
        }
    }

    fn text_props(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        let fonts: Vec<String> = {
            let mut v: Vec<String> = self.font_files().into_keys().collect();
            v.sort();
            v
        };
        let ClipContent::Text(st) = &mut c.content else {
            return;
        };
        let r = ui.add(
            egui::TextEdit::multiline(&mut st.text)
                .id_salt(("clip-text", c.id))
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text(crate::tr("Text eingeben")),
        );
        if self.focus_text {
            r.request_focus();
            self.focus_text = false;
        }
        if r.changed() {
            e.changed = true;
        }
        if r.lost_focus() {
            e.commit = true;
        }
        theme::section(ui, "Schrift");
        theme::row(ui, "Schriftart", |ui| {
            let current = st.font_family.clone().unwrap_or_else(|| "System".into());
            egui::ComboBox::from_id_salt("font")
                .selected_text(current)
                .width((ui.available_width() - 12.0).max(60.0))
                .height(320.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(st.font_family.is_none(), crate::tr("System"))
                        .clicked()
                    {
                        st.font_family = None;
                        e.commit = true;
                    }
                    // Favoriten zuerst (mit Stern)
                    let mut fonts = fonts.clone();
                    fonts.sort_by_key(|f| !self.library.is_favorite(&format!("font:{f}")));
                    for f in &fonts {
                        let fav = self.library.is_favorite(&format!("font:{f}"));
                        let txt = RichText::new(if fav { format!("★ {f}") } else { f.clone() })
                            .font(egui::FontId::new(15.0, self.font_family(Some(f))));
                        if ui
                            .selectable_label(st.font_family.as_deref() == Some(f), txt)
                            .clicked()
                        {
                            st.font_family = Some(f.clone());
                            e.commit = true;
                        }
                    }
                    ui.separator();
                    if ui
                        .button(crate::tr(format!(
                            "{}  Schrift importieren …",
                            icon::UPLOAD_SIMPLE
                        )))
                        .clicked()
                    {
                        let ctx = ui.ctx().clone();
                        self.pick_fonts(&ctx);
                    }
                });
        });
        *e |= theme::row(ui, "Größe", |ui| {
            Slider::new(&mut st.size, 8.0..=400.0)
                .default(64.0)
                .show(ui)
        });
        theme::row(ui, "Stil", |ui| {
            for (on, g, tip) in [
                (&mut st.bold, icon::TEXT_B, "Fett"),
                (&mut st.italic, icon::TEXT_ITALIC, "Kursiv"),
                (&mut st.uppercase, icon::TEXT_AA, "Großbuchstaben"),
                (
                    &mut st.auto_wrap,
                    icon::ARROW_ELBOW_DOWN_LEFT,
                    "Automatischer Umbruch",
                ),
            ] {
                if ui
                    .add(
                        egui::Button::new(RichText::new(crate::tr(g)).size(14.0))
                            .selected(*on)
                            .min_size(vec2(26.0, 22.0)),
                    )
                    .on_hover_text(crate::tr(tip))
                    .clicked()
                {
                    *on = !*on;
                    e.commit = true;
                }
            }
        });
        theme::row(ui, "Ausrichtung", |ui| {
            for (v, g) in [
                (-1i8, icon::TEXT_ALIGN_LEFT),
                (0, icon::TEXT_ALIGN_CENTER),
                (1, icon::TEXT_ALIGN_RIGHT),
            ] {
                if ui
                    .add(
                        egui::Button::new(RichText::new(crate::tr(g)).size(14.0))
                            .selected(st.align == v)
                            .min_size(vec2(26.0, 22.0)),
                    )
                    .clicked()
                {
                    st.align = v;
                    e.commit = true;
                }
            }
        });
        *e |= theme::row(ui, "Zeilenabstand", |ui| {
            Slider::new(&mut st.line_spacing, 0.6..=3.0)
                .decimals(1)
                .suffix("×")
                .default(1.0)
                .show(ui)
        });
        theme::row(ui, "Farbe", |ui| {
            if widgets::color_button(ui, &mut st.color) {
                e.commit = true;
            }
        });
        theme::section(ui, "Kontur");
        let mut on = st.outline.is_some();
        if widgets::toggle_row(ui, "Kontur", &mut on) {
            st.outline = if on {
                Some((4.0, [0, 0, 0, 255]))
            } else {
                None
            };
            e.commit = true;
        }
        if let Some((w, col)) = &mut st.outline {
            *e |= theme::row(ui, "Stärke", |ui| {
                Slider::new(w, 0.5..=20.0).decimals(1).default(4.0).show(ui)
            });
            theme::row(ui, "Farbe", |ui| {
                if widgets::color_button(ui, col) {
                    e.commit = true;
                }
            });
        }
        theme::section(ui, "Fläche und Schatten");
        let mut on = st.background.is_some();
        if widgets::toggle_row(ui, "Hintergrundfläche", &mut on) {
            st.background = if on { Some([0, 0, 0, 170]) } else { None };
            e.commit = true;
        }
        if let Some(bg) = &mut st.background {
            theme::row(ui, "Flächenfarbe", |ui| {
                if widgets::color_button(ui, bg) {
                    e.commit = true;
                }
            });
        }
        if widgets::toggle_row(ui, "Schatten", &mut st.shadow) {
            e.commit = true;
        }
        theme::section(ui, "Texteffekte");
        let mut chosen = None;
        widgets::grid(
            ui,
            cl_fx::text::TEXT_EFFECTS,
            52.0,
            1.0,
            0.0,
            |ui, r, fx| {
                let mut s2 = TextStyle {
                    text: "Aa".into(),
                    size: 64.0,
                    bold: true,
                    ..Default::default()
                };
                (fx.apply)(&mut s2);
                let resp = widgets::tile(
                    ui,
                    egui::Id::new(("teff-i", fx.id)),
                    r,
                    "",
                    false,
                    |p, a| {
                        p.rect_filled(a, 6.0, Color32::from_rgb(0x2A, 0x2D, 0x36));
                        let k = (a.height() * 0.5) / 64.0;
                        self.draw_text(p, &s2, a.center(), k, 0.0, 1.0, a.width());
                    },
                );
                if resp.on_hover_text(crate::tr(fx.name)).clicked() {
                    chosen = Some(fx.apply);
                }
            },
        );
        if let Some(apply) = chosen {
            apply(st);
            e.commit = true;
        }
    }

    fn effects_props(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        theme::section(ui, "Filter");
        match &mut c.fx.filter {
            Some(f) => {
                let name = cl_fx::fx_name(&f.id);
                let mut remove = false;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::tr(format!("{}  {name}", icon::CIRCLE_HALF))).strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        remove = theme::icon_button(ui, icon::X, "Filter entfernen", true, false)
                            .clicked();
                    });
                });
                *e |= theme::row(ui, "Stärke", |ui| {
                    Slider::new(&mut f.intensity, 0.0..=1.0)
                        .percent()
                        .default(1.0)
                        .show(ui)
                });
                if remove {
                    c.fx.filter = None;
                    e.commit = true;
                }
            }
            None => {
                if ui
                    .add(theme::secondary(format!(
                        "{}  Filter wählen",
                        icon::CIRCLE_HALF
                    )))
                    .clicked()
                {
                    self.tab = LeftTab::Filters;
                }
            }
        }
        theme::section(ui, "Effekte");
        let mut remove = None;
        for (i, fx) in c.fx.effects.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::tr(format!(
                        "{}  {}",
                        icon::SPARKLE,
                        cl_fx::fx_name(&fx.id)
                    )))
                    .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::X, "Effekt entfernen", true, false).clicked() {
                        remove = Some(i);
                    }
                });
            });
            *e |= theme::row(ui, "Stärke", |ui| {
                Slider::new(&mut fx.intensity, 0.0..=1.0)
                    .percent()
                    .default(1.0)
                    .show(ui)
            });
            ui.add_space(4.0);
        }
        if let Some(i) = remove {
            c.fx.effects.remove(i);
            e.commit = true;
        }
        if ui
            .add(theme::secondary(format!(
                "{}  Effekt hinzufügen",
                icon::PLUS
            )))
            .clicked()
        {
            self.tab = LeftTab::Effects;
        }
        theme::section(ui, "LUT");
        let mut clear = false;
        match &c.fx.lut {
            Some(l) => {
                ui.horizontal(|ui| {
                    let name = std::path::Path::new(l)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.add(
                        egui::Label::new(RichText::new(crate::tr(name)).color(theme::TEXT()))
                            .truncate(),
                    )
                    .on_hover_text(crate::tr(l));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clear =
                            theme::icon_button(ui, icon::X, "LUT entfernen", true, false).clicked()
                    });
                });
            }
            None => {
                if ui
                    .add(theme::secondary(format!(
                        "{}  LUT laden (.cube)",
                        icon::UPLOAD_SIMPLE
                    )))
                    .clicked()
                {
                    if let Some(f) = crate::dialog().add_filter("LUT", &["cube"]).pick_file() {
                        c.fx.lut = Some(f.to_string_lossy().into_owned());
                        e.commit = true;
                    }
                }
            }
        }
        if clear {
            c.fx.lut = None;
            e.commit = true;
        }
    }

    fn adjust_props(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        let a = &mut c.fx.adjust;
        theme::section(ui, "Licht");
        for (label, v) in [
            ("Helligkeit", &mut a.brightness),
            ("Kontrast", &mut a.contrast),
            ("Lichter", &mut a.highlights),
            ("Schatten", &mut a.shadows),
            ("Verblassen", &mut a.fade),
        ] {
            *e |= theme::row(ui, label, |ui| {
                Slider::new(v, -1.0..=1.0).percent().default(0.0).show(ui)
            });
        }
        theme::section(ui, "Farbe");
        for (label, v) in [
            ("Sättigung", &mut a.saturation),
            ("Temperatur", &mut a.temperature),
            ("Tönung", &mut a.tint),
        ] {
            *e |= theme::row(ui, label, |ui| {
                Slider::new(v, -1.0..=1.0).percent().default(0.0).show(ui)
            });
        }
        *e |= theme::row(ui, "Farbton", |ui| {
            Slider::new(&mut a.hue, -180.0..=180.0)
                .suffix("°")
                .default(0.0)
                .show(ui)
        });
        theme::section(ui, "HSL");
        let mut k = self
            .asset_cat
            .get("hsl")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0)
            .min(5);
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            for (i, (_, name, rgb)) in HSL_COLORS.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                ui.painter().circle_filled(
                    r.center(),
                    8.0,
                    Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                );
                if i == k {
                    ui.painter().circle_stroke(
                        r.center(),
                        10.5,
                        Stroke::new(1.5_f32, theme::ACCENT()),
                    );
                }
                if a.hsl[i] != [0.0; 3] {
                    ui.painter()
                        .circle_filled(r.center() + vec2(7.0, -7.0), 2.5, theme::ACCENT());
                }
                if resp.on_hover_text(crate::tr(*name)).clicked() {
                    k = i;
                }
            }
        });
        self.asset_cat.insert("hsl", k.to_string());
        let [h, s, l] = &mut a.hsl[k];
        *e |= theme::row(ui, "Farbton", |ui| {
            Slider::new(h, -1.0..=1.0).percent().default(0.0).show(ui)
        });
        *e |= theme::row(ui, "Sättigung", |ui| {
            Slider::new(s, -1.0..=1.0).percent().default(0.0).show(ui)
        });
        *e |= theme::row(ui, "Helligkeit", |ui| {
            Slider::new(l, -1.0..=1.0).percent().default(0.0).show(ui)
        });
        theme::section(ui, "Details");
        for (label, v, lo) in [
            ("Schärfe", &mut a.sharpen, -1.0),
            ("Vignette", &mut a.vignette, 0.0),
            ("Körnung", &mut a.grain, 0.0),
        ] {
            *e |= theme::row(ui, label, |ui| {
                Slider::new(v, lo..=1.0).percent().default(0.0).show(ui)
            });
        }
        ui.add_space(6.0);
        if ui
            .add(theme::secondary(format!(
                "{}  Alles zurücksetzen",
                icon::ARROW_COUNTER_CLOCKWISE
            )))
            .clicked()
        {
            c.fx.adjust = Adjust::default();
            e.commit = true;
        }
    }

    fn anim_props(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        let mut slot = self.anim_slot;
        widgets::segmented(ui, &["Ein", "Aus", "Kombi"], &mut slot);
        self.anim_slot = slot;
        ui.add_space(8.0);
        let list: Vec<(&str, &str)> = if slot == 2 {
            cl_core::COMBO_ANIMATIONS.to_vec()
        } else {
            cl_core::ANIMATIONS.to_vec()
        };
        let mut items = vec![("", "Keine")];
        items.extend(list);
        let cur = match slot {
            0 => c.anim.intro.as_ref(),
            1 => c.anim.outro.as_ref(),
            _ => c.anim.combo.as_ref(),
        }
        .map(|a| a.id.clone())
        .unwrap_or_default();
        let time = ui.input(|i| i.time);
        let mut chosen: Option<String> = None;
        widgets::grid(ui, &items, 66.0, 1.0, 16.0, |ui, r, (id, name)| {
            let hovered = ui.rect_contains_pointer(r);
            let resp = widgets::tile(
                ui,
                egui::Id::new(("anim", slot, *id)),
                r,
                name,
                cur == *id,
                |p, a| {
                    if id.is_empty() {
                        p.text(
                            a.center(),
                            egui::Align2::CENTER_CENTER,
                            icon::PROHIBIT,
                            egui::FontId::proportional(18.0),
                            theme::TEXT_FAINT(),
                        );
                        return;
                    }
                    // Mini-Vorschau: Quadrat, animiert beim Überfahren
                    let phase = if hovered {
                        ((time * 0.8).fract()) as f32
                    } else {
                        0.62
                    };
                    let d = if slot == 2 {
                        cl_core::combo_delta(id, phase * 1.2, 1.2)
                    } else {
                        cl_core::anim_delta(id, if slot == 1 { 1.0 - phase } else { phase })
                    };
                    let base = a.width().min(a.height()) * 0.42;
                    let ctr = a.center() + vec2(d.dx * a.width() * 0.5, d.dy * a.height() * 0.5);
                    let size = base * d.scale_mul;
                    let col =
                        Color32::from_white_alpha((d.opacity_mul.clamp(0.0, 1.0) * 230.0) as u8);
                    let rot = d.rot_add.to_radians();
                    let pts: Vec<egui::Pos2> =
                        [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                            .iter()
                            .map(|(x, y)| {
                                let v = vec2(x * size / 2.0, y * size / 2.0);
                                let (s, cc) = rot.sin_cos();
                                ctr + vec2(v.x * cc - v.y * s, v.x * s + v.y * cc)
                            })
                            .collect();
                    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
                },
            );
            if hovered {
                ui.ctx().request_repaint();
            }
            if resp.clicked() {
                chosen = Some(id.to_string());
            }
        });
        let def = Time::from_secs_f64(self.settings.anim_duration as f64);
        if let Some(id) = chosen {
            let combo_def = Time::from_secs_f64(1.2);
            let slot_ref = match slot {
                0 => &mut c.anim.intro,
                1 => &mut c.anim.outro,
                _ => &mut c.anim.combo,
            };
            *slot_ref = if id.is_empty() {
                None
            } else {
                Some(AnimRef {
                    id,
                    duration: slot_ref
                        .as_ref()
                        .map(|a| a.duration)
                        .unwrap_or(if slot == 2 { combo_def } else { def }),
                })
            };
            e.commit = true;
        }
        let max = if slot == 2 {
            5.0
        } else {
            (c.duration.as_secs_f64() as f32).min(5.0).max(0.1)
        };
        let slot_ref = match slot {
            0 => &mut c.anim.intro,
            1 => &mut c.anim.outro,
            _ => &mut c.anim.combo,
        };
        if let Some(a) = slot_ref {
            ui.add_space(6.0);
            let mut d = a.duration.as_secs_f64() as f32;
            let r = theme::row(ui, if slot == 2 { "Tempo" } else { "Dauer" }, |ui| {
                Slider::new(&mut d, 0.1..=max)
                    .suffix(" s")
                    .decimals(1)
                    .default(0.5)
                    .show(ui)
            });
            if r.changed {
                a.duration = Time::from_secs_f64(d as f64);
            }
            *e |= r;
        }
    }

    fn speed_props(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        let mut mode = if c.fx.speed_curve.is_some() { 1 } else { 0 };
        if widgets::segmented(ui, &["Normal", "Kurve"], &mut mode) {
            if mode == 0 {
                c.fx.speed_curve = None;
                e.commit = true;
            }
            self.asset_cat.insert("speed-mode", mode.to_string());
        }
        let mode = if c.fx.speed_curve.is_some() {
            1
        } else {
            self.asset_cat
                .get("speed-mode")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        };
        ui.add_space(8.0);
        if mode == 0 {
            let mut s = c.speed;
            let r = theme::row(ui, "Tempo", |ui| {
                Slider::new(&mut s, 0.1..=10.0)
                    .log()
                    .decimals(2)
                    .suffix("×")
                    .default(1.0)
                    .show(ui)
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_space(theme::LABEL_W + 6.0);
                for v in [0.5f32, 1.0, 1.5, 2.0, 3.0] {
                    if theme::cat_chip(ui, &format!("{v}×"), (c.speed - v).abs() < 1e-3).clicked()
                    {
                        self.commit_live();
                        self.apply(EditOp::SetSpeed {
                            clip: c.id,
                            speed: v,
                        });
                        return;
                    }
                }
            });
            let new_dur = c.source_span().as_secs_f64() / s.max(0.01) as f64;
            theme::row(ui, "Neue Dauer", |ui| {
                ui.label(
                    RichText::new(crate::tr(format!("{:.2} s", new_dur))).color(theme::TEXT_DIM()),
                )
            });
            if r.commit && (s - c.speed).abs() > 1e-4 {
                self.commit_live();
                self.apply(EditOp::SetSpeed {
                    clip: c.id,
                    speed: s,
                });
                return;
            }
        } else {
            ui.label(RichText::new(crate::tr("Ton ist bei Kurven stumm")).small().color(theme::TEXT_DIM()));
            ui.add_space(6.0);
            let mut items: Vec<(&str, &str)> = vec![("", "Keine")];
            items.extend(cl_core::SPEED_CURVES.iter().map(|c| (c.0, c.1)));
            let cur = c.fx.speed_curve.clone().unwrap_or_default();
            let mut chosen = None;
            widgets::grid(ui, &items, 80.0, 1.6, 16.0, |ui, r, (id, name)| {
                let resp = widgets::tile(
                    ui,
                    egui::Id::new(("curve", *id)),
                    r,
                    name,
                    cur == *id,
                    |p, a| {
                        if id.is_empty() {
                            p.line_segment(
                                [
                                    pos2(a.left() + 8.0, a.center().y),
                                    pos2(a.right() - 8.0, a.center().y),
                                ],
                                Stroke::new(1.5_f32, theme::TEXT_DIM()),
                            );
                            return;
                        }
                        let g = a.shrink(8.0);
                        let pts: Vec<egui::Pos2> = (0..=40)
                            .map(|k| {
                                let u = k as f32 / 40.0;
                                let v = (cl_core::curve_speed_at(id, u).ln() / 6f32.ln())
                                    .clamp(-1.0, 1.0);
                                pos2(
                                    g.left() + u * g.width(),
                                    g.center().y - v * g.height() / 2.0,
                                )
                            })
                            .collect();
                        p.line_segment(
                            [pos2(g.left(), g.center().y), pos2(g.right(), g.center().y)],
                            Stroke::new(1.0_f32, theme::BORDER()),
                        );
                        p.add(egui::Shape::line(pts, Stroke::new(1.6_f32, theme::ACCENT())));
                    },
                );
                if resp.clicked() {
                    chosen = Some(id.to_string());
                }
            });
            if let Some(id) = chosen {
                c.fx.speed_curve = if id.is_empty() { None } else { Some(id) };
                e.commit = true;
            }
        }
        if matches!(
            c.content,
            ClipContent::Media {
                kind: MediaKind::Video,
                ..
            }
        ) {
            if c.speed < 0.999 || c.fx.speed_curve.is_some() {
                theme::section(ui, "Zeitlupe");
                if widgets::toggle_row(ui, "Flüssige Zeitlupe", &mut c.fx.smooth_slowmo) {
                    e.commit = true;
                }
                ui.label(RichText::new(crate::tr("Vorschau angenähert, Export exakt")).small().color(theme::TEXT_DIM()));
            }
            theme::section(ui, "Richtung");
            if widgets::toggle_row(ui, "Rückwärts abspielen", &mut c.fx.reverse) {
                e.commit = true;
            }
        }
    }

    fn audio_props(
        &mut self,
        ui: &mut egui::Ui,
        c: &mut Clip,
        e: &mut Edit,
        kf: &mut Option<Vec<Prop>>,
    ) {
        theme::section(ui, "Lautstärke");
        *e |= self.prop_slider(
            ui,
            c,
            "Lautstärke",
            Prop::Volume,
            -60.0..=20.0,
            0.0,
            (" dB", 1, 1.0),
            kf,
        );
        let half = (c.duration.as_secs_f64() / 2.0) as f32;
        let mut fi = c.fade_in.as_secs_f64() as f32;
        let mut fo = c.fade_out.as_secs_f64() as f32;
        let r1 = theme::row(ui, "Einblenden", |ui| {
            Slider::new(&mut fi, 0.0..=half.max(0.01))
                .suffix(" s")
                .decimals(1)
                .default(0.0)
                .show(ui)
        });
        let r2 = theme::row(ui, "Ausblenden", |ui| {
            Slider::new(&mut fo, 0.0..=half.max(0.01))
                .suffix(" s")
                .decimals(1)
                .default(0.0)
                .show(ui)
        });
        c.fade_in = Time::from_secs_f64(fi as f64);
        c.fade_out = Time::from_secs_f64(fo as f64);
        *e |= r1;
        *e |= r2;
        if let ClipContent::Media {
            kind: MediaKind::Video,
            ..
        } = c.content
        {
            ui.add_space(8.0);
            if ui
                .add(theme::secondary(format!(
                    "{}  Ton extrahieren",
                    icon::WAVEFORM
                )))
                .on_hover_text(crate::tr("Ton als eigenen Clip auf eine Audiospur legen"))
                .clicked()
            {
                let id = c.id;
                self.commit_live();
                self.extract_audio(id);
            }
        }
        if let ClipContent::Media {
            kind: MediaKind::Audio,
            media,
        } = c.content
        {
            if let Some(m) = self.project.media(media) {
                let src = (m.duration - c.source_in).as_secs_f64() / c.speed.max(0.01) as f64;
                if c.duration.as_secs_f64() > src + 0.01 {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(crate::tr(format!(
                            "{}  Verlängert: {:.1} s Stille am Ende",
                            icon::INFO,
                            c.duration.as_secs_f64() - src
                        )))
                        .small()
                        .color(theme::TEXT_DIM()),
                    );
                }
            }
        }
        self.pro_audio(ui, c, e);
    }

    fn project_inspector(&mut self, ui: &mut egui::Ui) {
        let mut settings = self.project.settings.clone();
        let mut changed = false;
        let mut name = self.project.name.clone();
        let r = theme::row(ui, &self.t("project-name"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY))
        });
        if r.lost_focus() && name != self.project.name && !name.trim().is_empty() {
            self.apply(EditOp::Rename { name });
        }
        if let Some(p) = &self.project_path {
            theme::row(ui, "Speicherort", |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(crate::tr(p.display().to_string()))
                            .small()
                            .color(theme::TEXT_DIM()),
                    )
                    .truncate(),
                )
                .on_hover_text(crate::tr(p.display().to_string()));
            });
        }
        theme::section(ui, "Format");
        let mut picked: Option<Option<&'static crate::export_ui::Platform>> = None;
        theme::row(ui, "Plattform", |ui| {
            let cur = settings
                .platform
                .as_deref()
                .and_then(crate::export_ui::platform);
            egui::ComboBox::from_id_salt("proj-platform")
                .selected_text(crate::tr(cur.map(|p| p.name).unwrap_or("Keine")))
                .width((ui.available_width() - 12.0).max(60.0))
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(cur.is_none(), crate::tr("Keine"))
                        .clicked()
                    {
                        picked = Some(None);
                    }
                    for pl in crate::export_ui::PLATFORMS {
                        if ui
                            .selectable_label(
                                cur.is_some_and(|c| c.id == pl.id),
                                crate::tr(pl.name),
                            )
                            .clicked()
                        {
                            picked = Some(Some(pl));
                        }
                    }
                })
                .response
                .on_hover_text(crate::tr(
                    "Setzt Größe und Bildrate; der Export übernimmt die passende Vorlage",
                ));
        });
        if let Some(pl) = picked {
            match pl {
                Some(pl) => pl.apply_to(&mut settings),
                None => settings.platform = None,
            }
            if settings != self.project.settings {
                self.apply(EditOp::SetSettings { settings });
            }
            return;
        }
        let aspect_name = AspectPreset::PRESETS
            .iter()
            .find(|(_, p)| *p == settings.aspect)
            .map(|(n, _)| n.to_string())
            .unwrap_or_else(|| "Benutzerdefiniert".into());
        let mut new_aspect = None;
        theme::row(ui, &self.t("aspect"), |ui| {
            egui::ComboBox::from_id_salt("aspect")
                .selected_text(crate::tr(aspect_name))
                .width((ui.available_width() - 12.0).max(60.0))
                .show_ui(ui, |ui| {
                    for (n, p) in AspectPreset::PRESETS {
                        if ui
                            .selectable_label(settings.aspect == p, crate::tr(n))
                            .clicked()
                        {
                            new_aspect = Some(p);
                        }
                    }
                });
        });
        if let Some(p) = new_aspect {
            self.set_aspect(p);
            return;
        }
        theme::row(ui, &self.t("resolution"), |ui| {
            let (mut w, mut h) = (settings.width, settings.height);
            let a = ui.add(egui::DragValue::new(&mut w).range(16..=16384));
            ui.label(RichText::new("×").color(theme::TEXT_DIM()));
            let b = ui.add(egui::DragValue::new(&mut h).range(16..=16384));
            if (a.drag_stopped() || a.lost_focus() || b.drag_stopped() || b.lost_focus())
                && (w, h) != (settings.width, settings.height)
            {
                settings.width = w & !1;
                settings.height = h & !1;
                settings.aspect = AspectPreset::Custom;
                settings.platform = None;
                changed = true;
            }
        });
        theme::row(ui, &self.t("frame-rate"), |ui| {
            egui::ComboBox::from_id_salt("fps")
                .selected_text(crate::tr(format!("{:.3} fps", settings.fps.as_f64())))
                .width((ui.available_width() - 12.0).max(60.0))
                .show_ui(ui, |ui| {
                    for f in [
                        "23.976", "24", "25", "29.97", "30", "50", "59.94", "60", "120",
                    ] {
                        let r = Rational::parse(f).unwrap();
                        if ui
                            .selectable_label(settings.fps == r, crate::tr(format!("{f} fps")))
                            .clicked()
                        {
                            settings.fps = r;
                            changed = true;
                        }
                    }
                });
        });
        theme::row(ui, &self.t("color-space"), |ui| {
            let name = |c: ColorSpace| match c {
                ColorSpace::Rec709Sdr => "Rec. 709 SDR",
                ColorSpace::Rec2020Sdr => "Rec. 2020 SDR",
                ColorSpace::Rec2100Hlg => "Rec. 2100 HLG",
                ColorSpace::Rec2100Pq => "Rec. 2100 PQ",
            };
            egui::ComboBox::from_id_salt("cs")
                .selected_text(crate::tr(name(settings.color_space)))
                .width((ui.available_width() - 12.0).max(60.0))
                .show_ui(ui, |ui| {
                    for c in [
                        ColorSpace::Rec709Sdr,
                        ColorSpace::Rec2020Sdr,
                        ColorSpace::Rec2100Hlg,
                        ColorSpace::Rec2100Pq,
                    ] {
                        if ui
                            .selectable_label(settings.color_space == c, crate::tr(name(c)))
                            .clicked()
                        {
                            settings.color_space = c;
                            changed = true;
                        }
                    }
                });
        });
        theme::section(ui, "Bearbeiten");
        changed |= widgets::toggle_row(
            ui,
            &format!("{}  (P)", self.t("magnet")),
            &mut settings.main_track_magnet,
        );
        changed |= widgets::toggle_row(
            ui,
            &format!("{}  (N)", self.t("snapping")),
            &mut settings.snapping,
        );
        if changed && settings != self.project.settings {
            self.apply(EditOp::SetSettings { settings });
        }
        theme::section(ui, "Übersicht");
        let clips: usize = self
            .project
            .timeline
            .tracks
            .iter()
            .map(|t| t.clips.len())
            .sum();
        theme::row(ui, "Inhalt", |ui| {
            ui.label(
                RichText::new(crate::tr(format!(
                    "{} Spuren · {clips} Clips · {} Medien",
                    self.project.timeline.tracks.len(),
                    self.project.media.len()
                )))
                .color(theme::TEXT_DIM()),
            )
        });
        theme::row(ui, "Länge", |ui| {
            ui.label(
                RichText::new(crate::tr(
                    self.project.duration().timecode(self.project.settings.fps),
                ))
                .monospace()
                .color(theme::TEXT_DIM()),
            )
        });
        self.marker_list(ui);
    }

    /// Verlauf: oben die rückgängig gemachten Schritte (blass), dann „Jetzt“, darunter die
    /// bisherigen Änderungen. Klick auf einen Eintrag springt zu dem Stand nach diesem Schritt.
    fn history_inspector(&mut self, ui: &mut egui::Ui) {
        type Row = (&'static str, OpSource, std::time::SystemTime);
        let (done, undone): (Vec<Row>, Vec<Row>) = {
            let d = self.doc.lock().unwrap();
            let map = |e: &cl_core::HistoryEntry| (e.label, e.source.clone(), e.at);
            (d.undo_entries().iter().map(map).collect(), d.redo_entries().iter().map(map).collect())
        };
        // Kopf: Rückgängig/Wiederholen und Anzahl
        ui.horizontal(|ui| {
            if theme::icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, format!("{}  (Strg+Z)", self.t("undo")), !done.is_empty(), false).clicked() {
                self.history_step(-1);
            }
            if theme::icon_button(ui, icon::ARROW_CLOCKWISE, format!("{}  (Strg+Umschalt+Z)", self.t("redo")), !undone.is_empty(), false).clicked() {
                self.history_step(1);
            }
            ui.label(
                RichText::new(crate::tr(if done.len() == 1 { "1 Schritt".to_string() } else { format!("{} Schritte", done.len()) }))
                    .small()
                    .color(theme::TEXT_DIM()),
            );
        });
        if done.is_empty() && undone.is_empty() {
            theme::empty_state(
                ui,
                icon::CLOCK_COUNTER_CLOCKWISE,
                &self.t("history-empty"),
                "Jede Änderung erscheint hier und lässt sich mit einem Klick zurückholen",
            );
            return;
        }
        let mut q = self.asset_cat.get("hist-q").cloned().unwrap_or_default();
        ui.add(
            egui::TextEdit::singleline(&mut q)
                .hint_text(crate::tr(format!("{}  Suchen", icon::MAGNIFYING_GLASS)))
                .desired_width(f32::INFINITY),
        );
        self.asset_cat.insert("hist-q", q.clone());
        let ql = q.to_lowercase();
        let matches = |l: &str| ql.is_empty() || crate::tr(l).to_lowercase().contains(&ql);
        let mut jump: Option<isize> = None;
        egui::ScrollArea::vertical().id_salt("history").auto_shrink([false, false]).show(ui, |ui| {
            // Zukunft (rückgängig gemacht): der weiteste Schritt oben
            for (i, (label, src, at)) in undone.iter().enumerate() {
                if !matches(label) {
                    continue;
                }
                // Abstand vom jetzigen Stand: der nächste Wiederholen-Schritt ist der letzte
                let dist = (undone.len() - i) as isize;
                if history_row(ui, label, src, *at, false).on_hover_text(crate::tr("Bis hier wiederholen")).clicked() {
                    jump = Some(dist);
                }
            }
            // Jetzt
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::CARET_RIGHT).color(theme::ACCENT()));
                ui.label(RichText::new(crate::tr("Jetzt")).strong().color(theme::ACCENT()));
            });
            for (k, (label, src, at)) in done.iter().rev().enumerate() {
                if !matches(label) {
                    continue;
                }
                if k > 0
                    && history_row(ui, label, src, *at, true)
                        .on_hover_text(crate::tr("Zu diesem Stand zurück"))
                        .clicked()
                {
                    jump = Some(-(k as isize));
                } else if k == 0 {
                    history_row(ui, label, src, *at, true);
                }
            }
            // Ursprung
            if !done.is_empty()
                && ui
                    .add(theme::menu_row(crate::tr(format!("{}   Projektbeginn", icon::FLAG))))
                    .on_hover_text(crate::tr("Alle Schritte rückgängig"))
                    .clicked()
            {
                jump = Some(-(done.len() as isize));
            }
        });
        if let Some(n) = jump {
            self.history_step(n);
        }
    }
}

/// Symbol je Art der Änderung
fn history_icon(label: &str) -> &'static str {
    let l = label.to_lowercase();
    let has = |w: &str| l.contains(w);
    if has("lösch") || has("entfern") {
        icon::TRASH
    } else if has("teilen") {
        icon::SCISSORS
    } else if has("verschieb") {
        icon::ARROWS_OUT_CARDINAL
    } else if has("trimm") || has("clip-zeit") || has("kürzen") {
        icon::ARROWS_IN_LINE_HORIZONTAL
    } else if has("text") {
        icon::TEXT_T
    } else if has("lautstärke") || has("blenden") || has("ton") {
        icon::SPEAKER_HIGH
    } else if has("effekt") || has("filter") {
        icon::SPARKLE
    } else if has("übergang") {
        icon::INTERSECT
    } else if has("keyframe") || has("animation") {
        icon::DIAMOND
    } else if has("spur") {
        icon::ROWS
    } else if has("import") || has("einfügen") || has("hinzufügen") {
        icon::PLUS
    } else if has("geschwindigkeit") || has("tempo") {
        icon::GAUGE
    } else if has("marker") {
        icon::BOOKMARK_SIMPLE
    } else if has("projekt") {
        icon::GEAR_SIX
    } else {
        icon::PENCIL_SIMPLE
    }
}

/// Eine Zeile des Verlaufs; `done = false` → rückgängig gemacht (blass)
fn history_row(
    ui: &mut egui::Ui,
    label: &str,
    src: &OpSource,
    at: std::time::SystemTime,
    done: bool,
) -> egui::Response {
    let who = match src {
        OpSource::Ui => String::new(),
        OpSource::Mcp(c) => format!("  · {} {c}", icon::PLUGS_CONNECTED),
        OpSource::Plugin(n) => format!("  · {} {n}", icon::PUZZLE_PIECE),
    };
    let when = crate::home::ago(at);
    let text = format!("{}   {}{who}", history_icon(label), crate::tr(label));
    let r = ui.add(theme::menu_row(text).hint(&when).min_width(ui.available_width()));
    if !done {
        // rückgängig gemachte Schritte blass überdecken
        ui.painter().rect_filled(r.rect, 5.0, theme::BG_CARD().linear_multiply(0.55));
    }
    r
}
