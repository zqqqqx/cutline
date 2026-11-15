//! Hintergrundaufgaben (Spracherkennung, Freistellen, Analysen, Modell-Downloads) mit
//! Fortschritt und Abbrechen. Ergebnisse kommen über einen Kanal zurück in die App.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use cl_core::{ClipId, Time};
use eframe::egui;

pub enum TaskDone {
    Captions {
        segments: Vec<cl_analyze::asr::Segment>,
        offset: Time,
        engine: &'static str,
    },
    Matte {
        clip: ClipId,
        out: PathBuf,
    },
    Scenes {
        clip: ClipId,
        times: Vec<f64>,
    },
    Beats {
        clip: ClipId,
        times: Vec<f64>,
        bpm: f64,
    },
    Track {
        clip: ClipId,
        target: ClipId,
        points: Vec<(f64, f32, f32)>,
        from: Time,
    },
    Reframe {
        clip: ClipId,
        points: Vec<(f64, f32, f32)>,
    },
    Model {
        id: &'static str,
    },
    /// Ergebnis direkt auf dem Haupt-Thread anwenden
    Run(Box<dyn FnOnce(&mut crate::app::App) + Send>),
    /// Proxy fertig (oder fehlgeschlagen)
    Proxy {
        media: cl_core::MediaId,
        ok: bool,
    },
    Failed(String),
}

pub struct Task {
    pub id: u64,
    pub label: String,
    /// Art (verhindert doppelte Aufträge), z. B. „matte:<clip>“
    pub key: String,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    /// Projekt beim Start (siehe [`Tasks::next_generation`])
    generation: u64,
}

impl Task {
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct Tasks {
    pub list: Vec<Task>,
    next: u64,
    tx: Sender<(u64, TaskDone)>,
    rx: Receiver<(u64, TaskDone)>,
    generation: u64,
}

impl Default for Tasks {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            list: Vec::new(),
            next: 1,
            tx,
            rx,
            generation: 0,
        }
    }
}

impl Tasks {
    pub fn running(&self, key: &str) -> Option<&Task> {
        self.list.iter().find(|t| t.key == key)
    }

    /// Aufgabe starten; `f` bekommt eine Fortschrittsfunktion (0…1) und das Abbruch-Flag.
    pub fn spawn(
        &mut self,
        ctx: &egui::Context,
        key: impl Into<String>,
        label: impl Into<String>,
        f: impl FnOnce(&dyn Fn(f32), &AtomicBool) -> TaskDone + Send + 'static,
    ) {
        let key = key.into();
        if self.running(&key).is_some() {
            return;
        }
        let id = self.next;
        self.next += 1;
        let progress = Arc::new(AtomicU32::new(0f32.to_bits()));
        let cancel = Arc::new(AtomicBool::new(false));
        self.list.push(Task {
            id,
            label: label.into(),
            key,
            progress: progress.clone(),
            cancel: cancel.clone(),
            generation: self.generation,
        });
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("cl-task".into())
            .spawn(move || {
                let p = progress.clone();
                let c2 = ctx.clone();
                let report = move |v: f32| {
                    p.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                    c2.request_repaint();
                };
                let done = f(&report, &cancel);
                let _ = tx.send((id, done));
                ctx.request_repaint();
            })
            .expect("Task-Thread");
    }

    /// Anderes Projekt im Editor: Ergebnisse älterer Aufgaben gehören nicht mehr dazu.
    pub fn next_generation(&mut self) {
        self.generation += 1;
    }

    /// Fertige Aufgaben abholen. Projektbezogene Ergebnisse aus einem anderen Projekt werden
    /// zur Meldung statt ins offene Projekt eingefügt.
    pub fn poll(&mut self) -> Vec<(String, TaskDone)> {
        let mut out = Vec::new();
        while let Ok((id, done)) = self.rx.try_recv() {
            if let Some(i) = self.list.iter().position(|t| t.id == id) {
                let t = self.list.remove(i);
                let global = matches!(done, TaskDone::Failed(_) | TaskDone::Model { .. } | TaskDone::Proxy { .. });
                let done = if t.generation == self.generation || global {
                    done
                } else {
                    TaskDone::Failed("Ergebnis verworfen, weil inzwischen ein anderes Projekt geöffnet wurde".into())
                };
                out.push((t.key, done));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_from_a_left_project_are_dropped() {
        let ctx = egui::Context::default();
        let mut t = Tasks::default();
        t.spawn(&ctx, "asr", "Untertitel", |_, _| TaskDone::Captions { segments: Vec::new(), offset: Time::ZERO, engine: "x" });
        t.spawn(&ctx, "model", "Modell", |_, _| TaskDone::Model { id: "m" });
        t.next_generation();
        let mut got = Vec::new();
        while got.len() < 2 {
            got.extend(t.poll());
            std::thread::yield_now();
        }
        for (k, d) in got {
            match k.as_str() {
                "asr" => assert!(matches!(d, TaskDone::Failed(_))),
                _ => assert!(matches!(d, TaskDone::Model { .. })),
            }
        }
    }
}
