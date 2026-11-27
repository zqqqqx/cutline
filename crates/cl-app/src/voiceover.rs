//! Voiceover: small window with record button, level, live waveform, microphone choice.
//! While recording, a red clip with waveform also grows in the timeline.

use eframe::egui::{self, pos2, vec2, Rect, RichText, Stroke};

use crate::app::App;
use crate::preview::WAVE_PER_SEC;
use crate::theme::{self, icon};
use crate::widgets::{self, Slider};

impl App {
    pub fn voiceover_window(&mut self, ctx: &egui::Context) {
        if !self.voiceover.open {
            return;
        }
        if self.voiceover.devices.is_empty() {
            self.voiceover.devices = crate::record::input_devices();
        }
        let mut open = true;
        let recording = self.voiceover.recorder.is_some();
        let title = format!("{}  {}", icon::MICROPHONE, self.t("voiceover"));
        crate::dialog::window(ctx, crate::tr(title.as_str()), "voiceover")
        .default_pos(ctx.screen_rect().right_top() + vec2(-360.0, 70.0))
        .fixed_size(vec2(300.0, 0.0))
        .show(ctx, |ui| {
            if crate::dialog::title(ui, &title) {
                open = false;
            }
            ui.horizontal(|ui| {
                let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 48.0), egui::Sense::click());
                let hover = resp.hovered();
                ui.painter().circle_stroke(
                    rect.center(),
                    22.0,
                    Stroke::new(2.0_f32, if hover { theme::TEXT() } else { theme::TEXT_DIM() }),
                );
                let counting = self.voiceover.countdown_until.is_some();
                if recording {
                    ui.painter().rect_filled(
                        Rect::from_center_size(rect.center(), vec2(16.0, 16.0)),
                        theme::R_CTRL,
                        theme::DANGER,
                    );
                } else if counting {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        icon::X,
                        egui::FontId::proportional(20.0), // icon glyph in the round record button
                        theme::TEXT(),
                    );
                } else {
                    ui.painter()
                        .circle_filled(rect.center(), 15.0, theme::DANGER);
                }
                if resp
                    .on_hover_text(crate::tr(if recording {
                        "Stop recording"
                    } else if counting {
                        "Cancel countdown (Esc)"
                    } else {
                        "Start recording"
                    }))
                    .clicked()
                {
                    if recording {
                        self.stop_voiceover();
                    } else if counting {
                        self.voiceover.countdown_until = None;
                    } else {
                        self.start_voiceover();
                    }
                }
                ui.vertical(|ui| {
                    let (time, state) = if let Some(r) = &self.voiceover.recorder {
                        let s = r.elapsed().as_secs();
                        (
                            format!("{:02}:{:02}", s / 60, s % 60),
                            "Recording~running".to_string(),
                        )
                    } else if let Some(u) = self.voiceover.countdown_until {
                        let left = u
                            .saturating_duration_since(std::time::Instant::now())
                            .as_secs()
                            + 1;
                        (format!("{left}"), "Get ready".to_string())
                    } else {
                        ("00:00".into(), "Starts at the playhead".into())
                    };
                    ui.label(RichText::new(crate::tr(time)).size(theme::FS_PAGE).monospace());
                    ui.label(
                        RichText::new(crate::tr(state))
                            .small()
                            .color(theme::TEXT_DIM()),
                    );
                });
            });
            ui.add_space(6.0);
            // Live waveform of the last seconds
            let (bar, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 34.0), egui::Sense::hover());
            ui.painter().rect_filled(bar, theme::R_CTRL, theme::BG_INPUT());
            if let Some(r) = &self.voiceover.recorder {
                let peaks = r.peaks();
                let n = (bar.width() / 2.0) as usize;
                let start = peaks.len().saturating_sub(n);
                for (i, v) in peaks[start..].iter().enumerate() {
                    let x = bar.left() + 2.0 + i as f32 * 2.0;
                    let h = (v.sqrt() * (bar.height() - 6.0)).max(1.0);
                    ui.painter().line_segment(
                        [
                            pos2(x, bar.center().y - h / 2.0),
                            pos2(x, bar.center().y + h / 2.0),
                        ],
                        Stroke::new(
                            1.4_f32,
                            if *v > 0.9 || r.level() > 0.98 {
                                theme::DANGER
                            } else {
                                theme::ACCENT()
                            },
                        ),
                    );
                }
                let _ = WAVE_PER_SEC;
            }
            ui.add_space(8.0);
            let mut tab = self.voiceover.tab;
            widgets::segmented(ui, &["Recording", "Settings"], &mut tab);
            self.voiceover.tab = tab;
            ui.add_space(6.0);
            ui.add_enabled_ui(!recording, |ui| {
                if self.voiceover.tab == 0 {
                    let devs = self.voiceover.devices.clone();
                    egui::ComboBox::from_id_salt("vo-dev")
                        .selected_text(crate::tr(
                            self.voiceover
                                .device
                                .clone()
                                .unwrap_or_else(|| "Default microphone".into()),
                        ))
                        .width(268.0)
                        .show_ui(ui, |ui| {
                            for d in devs {
                                ui.selectable_value(&mut self.voiceover.device, Some(d.clone()), crate::tr(d));
                            }
                        });
                    ui.add_space(4.0);
                    theme::row(ui, "Input", |ui| {
                        Slider::new(&mut self.voiceover.gain_db, -12.0..=24.0)
                            .suffix(" dB")
                            .default(0.0)
                            .show(ui)
                    });
                    widgets::toggle_row(ui, "Mute project", &mut self.voiceover.mute_project);
                    widgets::toggle_row(ui, "3-2-1 countdown", &mut self.voiceover.countdown);
                } else {
                    egui::ScrollArea::vertical().max_height(380.0).min_scrolled_height(380.0).auto_shrink([false, false]).show(ui, |ui| {
                        theme::row(ui, "Volume", |ui| {
                            Slider::new(&mut self.voiceover.volume_db, -24.0..=12.0)
                                .suffix(" dB")
                                .default(0.0)
                                .show(ui)
                        });
                        let _ = crate::pro::audio_fx_ui(ui, &mut self.voiceover.audio);
                    });
                }
            });
        });
        if self.voiceover.countdown_until.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.voiceover.countdown_until = None;
        }
        if self.voiceover.recorder.is_some() {
            ctx.request_repaint();
        }
        if !open {
            if self.voiceover.recorder.is_some() {
                self.stop_voiceover();
            }
            self.voiceover.countdown_until = None;
            self.voiceover.open = false;
        }
    }
}
